//! `SqliteMemoryReader` — production [`MemoryReader`] backed by its own
//! SQLite [`Connection`].
//!
//! ## Why a separate connection?
//!
//! The pipeline runs on a [`super::worker::WorkerPool`], whose processor must be
//! `Send + Sync` (held inside `Arc<dyn JobProcessor>`). [`crate::memory::Memory`]
//! and [`crate::storage::Storage`] both own a `Connection`, which is `Send`
//! but not `Sync`, so they can't be shared as-is.
//!
//! Rather than wrap the entire `Memory` in a `Mutex` (which would serialize
//! all foreground store/recall traffic on top of the worker traffic), the
//! resolution pipeline opens its **own** connection to the same database
//! file. SQLite's WAL mode (set up in `Storage::open`) handles cross-
//! connection concurrency cleanly: the writer (pipeline) and the reader
//! (foreground) don't block each other for ordinary work, and writes are
//! serialized at the file lock level.
//!
//! ## Why `Mutex<Connection>`?
//!
//! `MemoryReader: Send + Sync`. We hold the connection inside a `Mutex` to
//! upgrade `Send`-only `Connection` to `Send + Sync` for the trait object.
//! Reads inside `fetch()` are short — a single `query_row` plus an
//! `access_log` lookup — so contention is negligible in practice.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use super::pipeline::{MemoryReadError, MemoryReader};
use crate::storage::fetch_memory_record;
use crate::types::MemoryRecord;

/// SQLite-backed [`MemoryReader`] implementation.
///
/// Owns a dedicated connection (separate from `Memory`'s own connection)
/// so the resolution worker pool can read memory rows without taking a
/// lock on the foreground `Memory` instance. See module docs for the
/// rationale.
pub struct SqliteMemoryReader {
    conn: Mutex<Connection>,
    /// Database path, kept for diagnostics.
    db_path: PathBuf,
}

impl SqliteMemoryReader {
    /// Open a fresh connection at `db_path`. The file must already exist
    /// (it is created by `Storage::open` before the pipeline pool is
    /// started).
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let path = db_path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        // Match the foreground connection's pragmas. WAL is critical:
        // without it, our reads would block the foreground writer.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // Foreign keys mirror Storage::open. Harmless for read-only usage.
        conn.pragma_update(None, "foreign_keys", "ON")?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: path,
        })
    }

    /// Path of the database this reader is attached to. Diagnostics only.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

impl MemoryReader for SqliteMemoryReader {
    fn fetch(&self, memory_id: &str) -> Result<Option<MemoryRecord>, MemoryReadError> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| MemoryReadError::Storage(format!("mutex poisoned: {e}")))?;
        fetch_memory_record(&guard, memory_id)
            .map_err(|e| MemoryReadError::Storage(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::Utc;
    use tempfile::tempdir;

    fn make_record(id: &str, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    #[test]
    fn fetch_returns_stored_record() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("mem.db");

        // Write a row through the foreground Storage.
        let mut storage = Storage::new(&db_path).unwrap();
        let rec = make_record("mem-1", "alice met bob in paris");
        storage.add(&rec, "test").unwrap();
        drop(storage);

        let reader = SqliteMemoryReader::open(&db_path).unwrap();
        let fetched = reader.fetch("mem-1").unwrap();
        assert!(fetched.is_some(), "row written via Storage should be readable via reader");
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, "mem-1");
        assert_eq!(fetched.content, "alice met bob in paris");
    }

    #[test]
    fn fetch_missing_returns_none() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("mem.db");
        // Need to initialize schema even though no rows are inserted.
        let _storage = Storage::new(&db_path).unwrap();

        let reader = SqliteMemoryReader::open(&db_path).unwrap();
        let fetched = reader.fetch("nonexistent").unwrap();
        assert!(fetched.is_none());
    }

    #[test]
    fn reader_is_send_and_sync() {
        // Compile-time assertion via trait bounds.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SqliteMemoryReader>();
    }
}
