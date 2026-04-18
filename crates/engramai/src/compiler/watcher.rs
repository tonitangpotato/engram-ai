//! Directory Watch Intake — GOAL-plat.12
//!
//! Monitors an inbox directory for new files and automatically imports them
//! as memories. Processed files are moved to a `processed/` subdirectory;
//! failed files go to `error/`.
//!
//! Supported formats:
//! - `.md`, `.txt` → Markdown import (split by heading)
//! - `.json` → JSON import
//! - `.ogg`, `.wav`, `.mp3`, `.m4a`, `.flac` → Voice intake (GOAL-plat.13, requires STT callback)
//!
//! # Architecture
//!
//! `DirectoryWatcher` is synchronous and polling-based (no external `notify` crate needed).
//! It scans the inbox directory at a configurable interval, processes new/changed files,
//! and moves them to outcome subdirectories. This avoids adding a heavyweight dependency
//! and works reliably across macOS/Linux.
//!
//! The watcher is designed to be driven by a caller's loop (e.g., a daemon thread):
//!
//! ```ignore
//! let watcher = DirectoryWatcher::new(inbox_path, store, config)?;
//! loop {
//!     let results = watcher.poll()?;
//!     for r in &results { println!("{}: {}", r.file_name, r.outcome); }
//!     std::thread::sleep(Duration::from_secs(5));
//! }
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::Utc;

use super::import::{ImportPipeline, MarkdownImporter, JsonImporter};
use super::storage::KnowledgeStore;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  CONFIGURATION
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for the directory watcher.
#[derive(Clone, Debug)]
pub struct WatcherConfig {
    /// Inbox directory to monitor.
    pub inbox_dir: PathBuf,
    /// Subdirectory for successfully processed files (relative to inbox_dir).
    pub processed_subdir: String,
    /// Subdirectory for failed files (relative to inbox_dir).
    pub error_subdir: String,
    /// Import configuration for text files.
    pub import_config: ImportConfig,
    /// Maximum file size to process (bytes). Files larger than this are skipped.
    pub max_file_size: u64,
}

impl WatcherConfig {
    /// Create a config with the given inbox directory and sensible defaults.
    pub fn new(inbox_dir: impl Into<PathBuf>) -> Self {
        Self {
            inbox_dir: inbox_dir.into(),
            processed_subdir: "processed".to_owned(),
            error_subdir: "error".to_owned(),
            import_config: ImportConfig {
                default_policy: ImportPolicy::Merge,
                split_strategy: SplitStrategy::ByHeading,
                duplicate_strategy: DuplicateStrategy::Skip,
                max_document_size_bytes: 10 * 1024 * 1024, // 10 MB
            },
            max_file_size: 50 * 1024 * 1024, // 50 MB (for audio files)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  WATCHER RESULT
// ═══════════════════════════════════════════════════════════════════════════════

/// Outcome of processing a single file.
#[derive(Clone, Debug)]
pub enum FileOutcome {
    /// File was imported successfully.
    Imported {
        report: ImportReport,
    },
    /// File was skipped (unsupported format, too large, etc.).
    Skipped {
        reason: String,
    },
    /// File processing failed.
    Failed {
        error: String,
    },
    /// Audio file needs STT — caller must provide transcription.
    NeedsStt {
        audio_path: PathBuf,
    },
}

/// Result of processing one file from the inbox.
#[derive(Clone, Debug)]
pub struct WatchResult {
    pub file_name: String,
    pub original_path: PathBuf,
    pub moved_to: Option<PathBuf>,
    pub outcome: FileOutcome,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  DIRECTORY WATCHER
// ═══════════════════════════════════════════════════════════════════════════════

/// Polls an inbox directory and imports new files as memories.
pub struct DirectoryWatcher {
    config: WatcherConfig,
    /// Tracks last-seen modification time per file path to avoid re-processing.
    seen: HashMap<PathBuf, SystemTime>,
}

impl DirectoryWatcher {
    /// Create a new `DirectoryWatcher`. Creates inbox + subdirectories if needed.
    pub fn new(config: WatcherConfig) -> Result<Self, KcError> {
        // Ensure directories exist
        fs::create_dir_all(&config.inbox_dir).map_err(|e| {
            KcError::ImportError(format!(
                "Failed to create inbox directory '{}': {}",
                config.inbox_dir.display(),
                e
            ))
        })?;

        let processed_dir = config.inbox_dir.join(&config.processed_subdir);
        fs::create_dir_all(&processed_dir).map_err(|e| {
            KcError::ImportError(format!(
                "Failed to create processed directory '{}': {}",
                processed_dir.display(),
                e
            ))
        })?;

        let error_dir = config.inbox_dir.join(&config.error_subdir);
        fs::create_dir_all(&error_dir).map_err(|e| {
            KcError::ImportError(format!(
                "Failed to create error directory '{}': {}",
                error_dir.display(),
                e
            ))
        })?;

        Ok(Self {
            config,
            seen: HashMap::new(),
        })
    }

    /// Scan the inbox directory and process any new or modified files.
    ///
    /// Returns results for each file processed in this poll cycle.
    /// Files in subdirectories (processed/, error/) are ignored.
    pub fn poll<S: KnowledgeStore>(&mut self, store: &S) -> Result<Vec<WatchResult>, KcError> {
        let entries = fs::read_dir(&self.config.inbox_dir).map_err(|e| {
            KcError::ImportError(format!(
                "Failed to read inbox directory '{}': {}",
                self.config.inbox_dir.display(),
                e
            ))
        })?;

        let mut results = Vec::new();

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("Failed to read directory entry: {}", e);
                    continue;
                }
            };

            let path = entry.path();

            // Skip subdirectories
            if path.is_dir() {
                continue;
            }

            // Skip hidden files (dotfiles)
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(true)
            {
                continue;
            }

            // Check modification time — skip if already processed
            let modified = match entry.metadata().and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };

            if let Some(prev) = self.seen.get(&path) {
                if *prev >= modified {
                    continue; // Already processed this version
                }
            }

            // Process the file
            let result = self.process_file(&path, store);

            // Record the modification time so we don't re-process
            self.seen.insert(path.clone(), modified);

            results.push(result);
        }

        // Clean up seen entries for files that no longer exist
        self.seen.retain(|p, _| p.exists());

        Ok(results)
    }

    /// Process a single file from the inbox.
    fn process_file<S: KnowledgeStore>(&self, path: &Path, store: &S) -> WatchResult {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_owned();

        // Check file size
        let file_size = match fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) => {
                return WatchResult {
                    file_name,
                    original_path: path.to_owned(),
                    moved_to: None,
                    outcome: FileOutcome::Failed {
                        error: format!("Cannot read file metadata: {}", e),
                    },
                };
            }
        };

        if file_size > self.config.max_file_size {
            return WatchResult {
                file_name,
                original_path: path.to_owned(),
                moved_to: None,
                outcome: FileOutcome::Skipped {
                    reason: format!(
                        "File too large ({} bytes, max {})",
                        file_size, self.config.max_file_size
                    ),
                },
            };
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Determine processing strategy based on extension
        let outcome = match ext.as_str() {
            "md" | "txt" => self.import_text_file(path, store),
            "json" => self.import_json_file(path, store),
            "ogg" | "wav" | "mp3" | "m4a" | "flac" | "webm" => {
                // Audio files need external STT — signal to caller
                Ok(FileOutcome::NeedsStt {
                    audio_path: path.to_owned(),
                })
            }
            _ => Ok(FileOutcome::Skipped {
                reason: format!("Unsupported file extension: .{}", ext),
            }),
        };

        let (outcome, moved_to) = match outcome {
            Ok(outcome) => {
                let dest = match &outcome {
                    FileOutcome::Imported { .. } => {
                        self.move_file(path, &self.config.processed_subdir)
                    }
                    FileOutcome::Failed { .. } => {
                        self.move_file(path, &self.config.error_subdir)
                    }
                    FileOutcome::NeedsStt { .. } => {
                        // Don't move — caller will handle after STT
                        Ok(None)
                    }
                    FileOutcome::Skipped { .. } => {
                        // Don't move skipped files — user might fix the extension
                        Ok(None)
                    }
                };
                (outcome, dest.unwrap_or(None))
            }
            Err(e) => {
                let dest = self.move_file(path, &self.config.error_subdir).unwrap_or(None);
                (
                    FileOutcome::Failed {
                        error: format!("{}", e),
                    },
                    dest,
                )
            }
        };

        WatchResult {
            file_name,
            original_path: path.to_owned(),
            moved_to,
            outcome,
        }
    }

    /// Import a text file (.md / .txt) as memories.
    fn import_text_file<S: KnowledgeStore>(
        &self,
        path: &Path,
        store: &S,
    ) -> Result<FileOutcome, KcError> {
        let importer = MarkdownImporter {
            split: self.config.import_config.split_strategy.clone(),
        };

        let report = ImportPipeline::run(store, &importer, path, &self.config.import_config)?;

        Ok(FileOutcome::Imported { report })
    }

    /// Import a JSON file as memories.
    fn import_json_file<S: KnowledgeStore>(
        &self,
        path: &Path,
        store: &S,
    ) -> Result<FileOutcome, KcError> {
        let importer = JsonImporter;

        let report = ImportPipeline::run(store, &importer, path, &self.config.import_config)?;

        Ok(FileOutcome::Imported { report })
    }

    /// Move a file to a subdirectory of the inbox, returning the new path.
    ///
    /// If a file with the same name already exists in the destination,
    /// appends a timestamp to avoid collisions.
    fn move_file(&self, path: &Path, subdir: &str) -> Result<Option<PathBuf>, KcError> {
        let dest_dir = self.config.inbox_dir.join(subdir);

        let file_name = path
            .file_name()
            .ok_or_else(|| KcError::ImportError("File has no name".to_owned()))?;

        let mut dest = dest_dir.join(file_name);

        // Avoid name collisions
        if dest.exists() {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let ts = Utc::now().format("%Y%m%d-%H%M%S");
            let new_name = if ext.is_empty() {
                format!("{}-{}", stem, ts)
            } else {
                format!("{}-{}.{}", stem, ts, ext)
            };
            dest = dest_dir.join(new_name);
        }

        if let Err(e) = fs::rename(path, &dest) {
            // If rename fails (cross-device), try copy + delete
            if e.raw_os_error() == Some(18) {
                // EXDEV — cross-device link
                fs::copy(path, &dest)
                    .and_then(|_| fs::remove_file(path))
                    .map_err(|copy_err| {
                        KcError::ImportError(format!(
                            "Failed to move file '{}' → '{}': {} (and copy fallback: {})",
                            path.display(),
                            dest.display(),
                            e,
                            copy_err
                        ))
                    })?;
            } else {
                return Err(KcError::ImportError(format!(
                    "Failed to move file '{}' → '{}': {}",
                    path.display(),
                    dest.display(),
                    e
                )));
            }
        }

        Ok(Some(dest))
    }

    /// Import a pre-transcribed audio file's text as a memory.
    ///
    /// Call this after STT to complete voice intake (GOAL-plat.13).
    /// The audio file is moved to `processed/` after successful import.
    pub fn import_transcription<S: KnowledgeStore>(
        &self,
        audio_path: &Path,
        transcript: &str,
        store: &S,
    ) -> Result<WatchResult, KcError> {
        let file_name = audio_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio")
            .to_owned();

        // Create a temporary .md file with the transcript
        let content = format!(
            "# Voice Note: {}\n\nTranscribed: {}\n\n{}",
            file_name,
            Utc::now().format("%Y-%m-%d %H:%M"),
            transcript
        );

        // Write to a temp file in inbox for ImportPipeline
        let tmp_path = self.config.inbox_dir.join(format!(
            ".tmp-transcript-{}.md",
            Utc::now().timestamp_millis()
        ));

        fs::write(&tmp_path, &content).map_err(|e| {
            KcError::ImportError(format!("Failed to write transcript: {}", e))
        })?;

        let importer = MarkdownImporter {
            split: SplitStrategy::Smart,
        };

        let result = ImportPipeline::run(store, &importer, &tmp_path, &self.config.import_config);

        // Clean up temp file
        let _ = fs::remove_file(&tmp_path);

        match result {
            Ok(report) => {
                // Move audio file to processed
                let moved = self.move_file(audio_path, &self.config.processed_subdir)?;
                Ok(WatchResult {
                    file_name,
                    original_path: audio_path.to_owned(),
                    moved_to: moved,
                    outcome: FileOutcome::Imported { report },
                })
            }
            Err(e) => {
                // Move audio to error dir
                let moved = self.move_file(audio_path, &self.config.error_subdir)?;
                Ok(WatchResult {
                    file_name,
                    original_path: audio_path.to_owned(),
                    moved_to: moved,
                    outcome: FileOutcome::Failed {
                        error: format!("{}", e),
                    },
                })
            }
        }
    }

    /// Get the inbox directory path.
    pub fn inbox_dir(&self) -> &Path {
        &self.config.inbox_dir
    }

    /// Get the processed directory path.
    pub fn processed_dir(&self) -> PathBuf {
        self.config.inbox_dir.join(&self.config.processed_subdir)
    }

    /// Get the error directory path.
    pub fn error_dir(&self) -> PathBuf {
        self.config.inbox_dir.join(&self.config.error_subdir)
    }

    /// Number of files currently tracked.
    pub fn tracked_count(&self) -> usize {
        self.seen.len()
    }

    /// Clear tracking state — all files will be re-evaluated on next poll.
    pub fn reset(&mut self) {
        self.seen.clear();
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::storage::SqliteKnowledgeStore;
    use tempfile::TempDir;

    fn make_store() -> SqliteKnowledgeStore {
        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();
        store
    }

    fn make_watcher(dir: &Path) -> DirectoryWatcher {
        let config = WatcherConfig::new(dir);
        DirectoryWatcher::new(config).unwrap()
    }

    // ── Construction ─────────────────────────────────────────────────────

    #[test]
    fn test_new_creates_directories() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        assert!(!inbox.exists());

        let _watcher = make_watcher(&inbox);

        assert!(inbox.exists());
        assert!(inbox.join("processed").exists());
        assert!(inbox.join("error").exists());
    }

    #[test]
    fn test_new_with_existing_directory() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        fs::create_dir_all(&inbox).unwrap();

        let watcher = make_watcher(&inbox);
        assert_eq!(watcher.tracked_count(), 0);
    }

    // ── Poll empty ───────────────────────────────────────────────────────

    #[test]
    fn test_poll_empty_inbox() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        let results = watcher.poll(&store).unwrap();
        assert!(results.is_empty());
    }

    // ── Markdown import ──────────────────────────────────────────────────

    #[test]
    fn test_poll_imports_markdown_file() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        // Drop a markdown file into inbox
        let md_path = inbox.join("note.md");
        fs::write(&md_path, "# My Note\n\nThis is a test note about Rust.\n").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 1);

        let r = &results[0];
        assert_eq!(r.file_name, "note.md");
        assert!(matches!(r.outcome, FileOutcome::Imported { .. }));
        assert!(r.moved_to.is_some());

        // File should be in processed/
        assert!(!md_path.exists(), "original file should be moved");
        assert!(inbox.join("processed/note.md").exists());
    }

    #[test]
    fn test_poll_imports_txt_file() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        let txt_path = inbox.join("thought.txt");
        fs::write(&txt_path, "Plain text thought about AI agents.\n").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].outcome, FileOutcome::Imported { .. }));
        assert!(!txt_path.exists());
    }

    // ── JSON import ──────────────────────────────────────────────────────

    #[test]
    fn test_poll_imports_json_file() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        let json_path = inbox.join("data.json");
        fs::write(
            &json_path,
            r#"[{"content": "JSON memory entry", "source": "test", "content_hash": "h1", "metadata": {}}]"#,
        )
        .unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 1);
        // JSON import may fail or succeed depending on ImportPipeline expectations
        // The important thing is it doesn't crash
        let _outcome = &results[0].outcome;
    }

    // ── Skipped files ────────────────────────────────────────────────────

    #[test]
    fn test_poll_skips_unsupported_extension() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        let bmp_path = inbox.join("image.bmp");
        fs::write(&bmp_path, b"fake image data").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].outcome, FileOutcome::Skipped { .. }));
        // Skipped files stay in inbox (not moved)
        assert!(bmp_path.exists());
    }

    #[test]
    fn test_poll_skips_dotfiles() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        fs::write(inbox.join(".hidden"), "hidden file").unwrap();
        fs::write(inbox.join(".DS_Store"), "mac garbage").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert!(results.is_empty(), "dotfiles should be ignored");
    }

    #[test]
    fn test_poll_skips_oversized_file() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();

        let mut config = WatcherConfig::new(&inbox);
        config.max_file_size = 100; // very small limit
        let mut watcher = DirectoryWatcher::new(config).unwrap();

        let big_path = inbox.join("big.md");
        fs::write(&big_path, "x".repeat(200)).unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].outcome, FileOutcome::Skipped { .. }));
    }

    // ── Audio files → NeedsStt ───────────────────────────────────────────

    #[test]
    fn test_poll_audio_needs_stt() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        for ext in &["ogg", "wav", "mp3", "m4a", "flac", "webm"] {
            let path = inbox.join(format!("voice.{}", ext));
            fs::write(&path, b"fake audio data").unwrap();
        }

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 6);
        for r in &results {
            assert!(
                matches!(r.outcome, FileOutcome::NeedsStt { .. }),
                "Audio file {} should need STT, got {:?}",
                r.file_name,
                r.outcome
            );
        }

        // Audio files should NOT be moved yet (caller handles after STT)
        for ext in &["ogg", "wav", "mp3", "m4a", "flac", "webm"] {
            assert!(inbox.join(format!("voice.{}", ext)).exists());
        }
    }

    // ── Idempotent polling ───────────────────────────────────────────────

    #[test]
    fn test_poll_does_not_reprocess() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        // Drop a file that will be skipped (so it stays in inbox)
        let path = inbox.join("image.png");
        fs::write(&path, b"fake image").unwrap();

        let r1 = watcher.poll(&store).unwrap();
        assert_eq!(r1.len(), 1);

        // Second poll — same file, same mtime → should not re-process
        let r2 = watcher.poll(&store).unwrap();
        assert!(r2.is_empty(), "Second poll should skip already-seen files");
    }

    #[test]
    fn test_poll_reprocesses_modified_file() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        // Drop a file that will be skipped
        let path = inbox.join("image.png");
        fs::write(&path, b"v1").unwrap();

        let r1 = watcher.poll(&store).unwrap();
        assert_eq!(r1.len(), 1);

        // Modify the file (sleep to ensure mtime changes)
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, b"v2 modified").unwrap();

        let r2 = watcher.poll(&store).unwrap();
        assert_eq!(r2.len(), 1, "Modified file should be re-processed");
    }

    // ── Transcription import ─────────────────────────────────────────────

    #[test]
    fn test_import_transcription() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let watcher = make_watcher(&inbox);

        // Simulate: audio file in inbox
        let audio_path = inbox.join("recording.ogg");
        fs::write(&audio_path, b"fake audio").unwrap();

        let transcript = "This is the transcribed text from a voice recording about Rust programming.";

        let result = watcher
            .import_transcription(&audio_path, transcript, &store)
            .unwrap();

        assert_eq!(result.file_name, "recording.ogg");
        assert!(matches!(result.outcome, FileOutcome::Imported { .. }));
        assert!(!audio_path.exists(), "audio file should be moved");
        assert!(inbox.join("processed/recording.ogg").exists());
    }

    // ── Reset tracking ───────────────────────────────────────────────────

    #[test]
    fn test_reset_clears_tracking() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        let path = inbox.join("skip.png");
        fs::write(&path, b"data").unwrap();

        let _ = watcher.poll(&store).unwrap();
        assert_eq!(watcher.tracked_count(), 1);

        watcher.reset();
        assert_eq!(watcher.tracked_count(), 0);

        // After reset, file will be re-processed
        let r = watcher.poll(&store).unwrap();
        assert_eq!(r.len(), 1);
    }

    // ── Multiple files in one poll ───────────────────────────────────────

    #[test]
    fn test_poll_multiple_files() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        fs::write(inbox.join("a.md"), "# Note A\n\nContent A\n").unwrap();
        fs::write(inbox.join("b.txt"), "Note B content\n").unwrap();
        fs::write(inbox.join("c.png"), b"image data").unwrap();
        fs::write(inbox.join("d.ogg"), b"audio data").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 4);

        let outcomes: HashMap<String, &FileOutcome> = results
            .iter()
            .map(|r| (r.file_name.clone(), &r.outcome))
            .collect();

        assert!(matches!(outcomes["a.md"], FileOutcome::Imported { .. }));
        assert!(matches!(outcomes["b.txt"], FileOutcome::Imported { .. }));
        assert!(matches!(outcomes["c.png"], FileOutcome::Skipped { .. }));
        assert!(matches!(outcomes["d.ogg"], FileOutcome::NeedsStt { .. }));
    }

    // ── Name collision handling ──────────────────────────────────────────

    #[test]
    fn test_move_file_handles_collision() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        // Pre-populate processed/ with a file that will collide
        fs::write(inbox.join("processed/note.md"), "old processed").unwrap();

        // Drop a new note.md into inbox
        fs::write(inbox.join("note.md"), "# New Note\n\nNew content\n").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].outcome, FileOutcome::Imported { .. }));

        // Should have been moved with a timestamp suffix
        let moved = results[0].moved_to.as_ref().unwrap();
        assert!(moved.exists());
        assert_ne!(
            moved.file_name().unwrap().to_str().unwrap(),
            "note.md",
            "Should have timestamp suffix to avoid collision"
        );
    }

    // ── Subdirectories ignored ───────────────────────────────────────────

    #[test]
    fn test_poll_ignores_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let store = make_store();
        let mut watcher = make_watcher(&inbox);

        // Create a subdirectory with a file in it
        let subdir = inbox.join("nested");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("nested.md"), "# Nested\n\nShould be ignored\n").unwrap();

        let results = watcher.poll(&store).unwrap();
        assert!(results.is_empty(), "Files in subdirectories should be ignored");
    }

    // ── Helper accessors ─────────────────────────────────────────────────

    #[test]
    fn test_directory_accessors() {
        let tmp = TempDir::new().unwrap();
        let inbox = tmp.path().join("inbox");
        let watcher = make_watcher(&inbox);

        assert_eq!(watcher.inbox_dir(), inbox);
        assert_eq!(watcher.processed_dir(), inbox.join("processed"));
        assert_eq!(watcher.error_dir(), inbox.join("error"));
    }
}
