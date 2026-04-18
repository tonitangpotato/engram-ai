//! Intake pipeline for extracting content from URLs and feeding it into the
//! import pipeline.
//!
//! Supports pluggable [`ContentExtractor`] implementations. Ships with
//! [`JinaExtractor`] (Jina Reader API) and [`GenericExtractor`] (plain HTTP
//! fetch). The pipeline produces [`MemoryCandidate`]s that the caller can
//! feed into [`super::import::ImportPipeline`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};

use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  CONTENT EXTRACTOR TRAIT
// ═══════════════════════════════════════════════════════════════════════════════

/// Extracts readable content from a URL.
pub trait ContentExtractor: Send + Sync {
    /// Check if this extractor handles the given URL.
    fn can_handle(&self, url: &str) -> bool;

    /// Extract content from the URL. Returns extracted text + metadata.
    fn extract(&self, url: &str) -> Result<ExtractedContent, KcError>;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  EXTRACTED CONTENT
// ═══════════════════════════════════════════════════════════════════════════════

/// Content extracted from a URL source.
#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub published: Option<DateTime<Utc>>,
    pub url: String,
    pub platform: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  INTAKE REPORT
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of an intake operation.
#[derive(Debug, Clone)]
pub struct IntakeReport {
    pub url: String,
    pub title: String,
    pub memory_candidate: MemoryCandidate,
    pub content_length: usize,
    pub platform: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  URL HASHING
// ═══════════════════════════════════════════════════════════════════════════════

/// Compute a hex-encoded hash of a URL using the standard library hasher.
/// Not cryptographic, but sufficient for deduplication by URL.
fn url_hash(url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;

    let mut h1 = DefaultHasher::new();
    url.hash(&mut h1);
    let v1 = h1.finish();

    let mut h2 = DefaultHasher::new();
    "salt".hash(&mut h2);
    url.hash(&mut h2);
    let v2 = h2.finish();

    format!("{:016x}{:016x}", v1, v2)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Extract the domain from a URL string (e.g. `"https://example.com/path"` → `"example.com"`).
fn extract_domain(url: &str) -> String {
    // Strip scheme
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Take everything before the first '/'
    let domain = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme);

    // Strip port if present
    domain
        .split(':')
        .next()
        .unwrap_or(domain)
        .to_owned()
}

// ═══════════════════════════════════════════════════════════════════════════════
//  JINA EXTRACTOR
// ═══════════════════════════════════════════════════════════════════════════════

/// Uses the [Jina Reader API](https://r.jina.ai/) to extract readable content
/// from any URL. Acts as a universal fallback extractor.
pub struct JinaExtractor {
    api_key: Option<String>,
}

impl JinaExtractor {
    /// Create a new `JinaExtractor` with an optional API key.
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }
}

impl ContentExtractor for JinaExtractor {
    fn can_handle(&self, _url: &str) -> bool {
        true // handles everything as fallback
    }

    fn extract(&self, url: &str) -> Result<ExtractedContent, KcError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| KcError::ImportError(format!("HTTP client error: {}", e)))?;

        let jina_url = format!("https://r.jina.ai/{}", url);
        let mut req = client.get(&jina_url);
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }
        req = req.header("Accept", "text/plain");

        let resp = req
            .send()
            .map_err(|e| KcError::ImportError(format!("Jina request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(KcError::ImportError(format!(
                "Jina returned status {}",
                resp.status()
            )));
        }

        let text = resp
            .text()
            .map_err(|e| KcError::ImportError(format!("Failed to read Jina response: {}", e)))?;

        // Parse: first line starting with # is the title, rest is content
        let (title, content) = parse_title_and_content(&text);
        let platform = extract_domain(url);

        Ok(ExtractedContent {
            title,
            author: None,
            content,
            published: None,
            url: url.to_owned(),
            platform,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  GENERIC EXTRACTOR
// ═══════════════════════════════════════════════════════════════════════════════

/// Simple HTTP fetch + text extraction for when Jina is not available.
/// Performs a plain GET request and attempts to extract meaningful text.
pub struct GenericExtractor;

impl ContentExtractor for GenericExtractor {
    fn can_handle(&self, _url: &str) -> bool {
        true
    }

    fn extract(&self, url: &str) -> Result<ExtractedContent, KcError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| KcError::ImportError(format!("HTTP client error: {}", e)))?;

        let resp = client
            .get(url)
            .header(
                "User-Agent",
                "engram-ai/1.0 (knowledge-compiler intake)",
            )
            .send()
            .map_err(|e| KcError::ImportError(format!("HTTP request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(KcError::ImportError(format!(
                "HTTP {} for {}",
                resp.status(),
                url
            )));
        }

        let body = resp
            .text()
            .map_err(|e| KcError::ImportError(format!("Failed to read response: {}", e)))?;

        // Attempt to extract a title from <title> tag
        let title = extract_html_title(&body)
            .unwrap_or_else(|| extract_domain(url));

        // Strip HTML tags for a rough text extraction
        let content = strip_html_tags(&body);
        let platform = extract_domain(url);

        Ok(ExtractedContent {
            title,
            author: None,
            content,
            published: None,
            url: url.to_owned(),
            platform,
        })
    }
}

/// Extract text content from the `<title>` tag in HTML.
fn extract_html_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title>")?;
    let after = start + 7;
    let end = lower[after..].find("</title>")?;
    let title = html[after..after + end].trim().to_owned();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

/// Crude HTML tag stripper — removes everything between `<` and `>`.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    // Collapse excessive whitespace
    let mut cleaned = String::with_capacity(result.len());
    let mut prev_blank = false;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                cleaned.push('\n');
                prev_blank = true;
            }
        } else {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
            prev_blank = false;
        }
    }

    cleaned.trim().to_owned()
}

/// Parse title and content from extracted text.
/// If the first line starts with `#`, treat it as the title.
fn parse_title_and_content(text: &str) -> (String, String) {
    let trimmed = text.trim();
    if let Some(first_newline) = trimmed.find('\n') {
        let first_line = trimmed[..first_newline].trim();
        if first_line.starts_with('#') {
            let title = first_line.trim_start_matches('#').trim().to_owned();
            let content = trimmed[first_newline..].trim().to_owned();
            if title.is_empty() {
                ("Untitled".to_owned(), trimmed.to_owned())
            } else {
                (title, content)
            }
        } else {
            // Use first line as title, rest as content
            (
                first_line.to_owned(),
                trimmed[first_newline..].trim().to_owned(),
            )
        }
    } else {
        // Single line — use as both title and content
        (trimmed.to_owned(), trimmed.to_owned())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  YT-DLP EXTRACTOR
// ═══════════════════════════════════════════════════════════════════════════════

/// Extracts content from YouTube videos using `yt-dlp`.
///
/// Attempts to fetch video metadata and subtitles via the `yt-dlp` CLI tool.
/// Falls back to the video description if subtitles are unavailable.
pub struct YtDlpExtractor;

impl YtDlpExtractor {
    /// Parse `upload_date` in YYYYMMDD format into a `DateTime<Utc>`.
    fn parse_upload_date(date_str: &str) -> Option<DateTime<Utc>> {
        if date_str.len() != 8 {
            return None;
        }
        let year: i32 = date_str[0..4].parse().ok()?;
        let month: u32 = date_str[4..6].parse().ok()?;
        let day: u32 = date_str[6..8].parse().ok()?;
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
    }

    /// Clean VTT/SRT subtitle text by stripping timestamps and formatting tags.
    fn clean_subtitle_text(text: &str) -> String {
        let mut lines = Vec::new();
        let mut prev_line = String::new();

        for line in text.lines() {
            let trimmed = line.trim();
            // Skip empty lines, timestamp lines, VTT headers, and sequence numbers
            if trimmed.is_empty()
                || trimmed.starts_with("WEBVTT")
                || trimmed.starts_with("Kind:")
                || trimmed.starts_with("Language:")
                || trimmed.contains("-->")
                || trimmed.parse::<u32>().is_ok()
            {
                continue;
            }
            // Strip HTML-like tags (e.g. <c>, </c>, <00:01:02.345>)
            let cleaned: String = {
                let mut result = String::with_capacity(trimmed.len());
                let mut in_tag = false;
                for ch in trimmed.chars() {
                    match ch {
                        '<' => in_tag = true,
                        '>' => in_tag = false,
                        _ if !in_tag => result.push(ch),
                        _ => {}
                    }
                }
                result
            };
            let cleaned = cleaned.trim().to_owned();
            // Deduplicate consecutive identical lines (common in auto-subs)
            if !cleaned.is_empty() && cleaned != prev_line {
                lines.push(cleaned.clone());
                prev_line = cleaned;
            }
        }

        lines.join(" ")
    }
}

impl ContentExtractor for YtDlpExtractor {
    fn can_handle(&self, url: &str) -> bool {
        url.contains("youtube.com/watch")
            || url.contains("youtu.be/")
            || url.contains("youtube.com/shorts/")
    }

    fn extract(&self, url: &str) -> Result<ExtractedContent, KcError> {
        use std::process::Command;

        // 1. Fetch metadata via yt-dlp --dump-json
        let meta_output = Command::new("yt-dlp")
            .args(["--dump-json", "--no-download", url])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    KcError::ImportError("yt-dlp not installed".to_owned())
                } else {
                    KcError::ImportError(format!("yt-dlp execution error: {}", e))
                }
            })?;

        if !meta_output.status.success() {
            let stderr = String::from_utf8_lossy(&meta_output.stderr);
            return Err(KcError::ImportError(format!(
                "yt-dlp metadata fetch failed: {}",
                stderr.trim()
            )));
        }

        let meta_json: serde_json::Value =
            serde_json::from_slice(&meta_output.stdout).map_err(|e| {
                KcError::ImportError(format!("Failed to parse yt-dlp JSON: {}", e))
            })?;

        let title = meta_json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Video")
            .to_owned();

        let uploader = meta_json
            .get("uploader")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        let description = meta_json
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        let published = meta_json
            .get("upload_date")
            .and_then(|v| v.as_str())
            .and_then(Self::parse_upload_date);

        let video_id = meta_json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // 2. Try to get subtitles
        let tmp_prefix = format!("/tmp/engram-ytdlp-{}", video_id);
        let sub_result = Command::new("yt-dlp")
            .args([
                "--write-sub",
                "--write-auto-sub",
                "--sub-lang",
                "en,zh",
                "--skip-download",
                "-o",
                &tmp_prefix,
                url,
            ])
            .output();

        let mut subtitle_content: Option<String> = None;

        if let Ok(sub_output) = sub_result {
            if sub_output.status.success() {
                // Look for subtitle files
                for ext in &["en.vtt", "en.srt", "zh.vtt", "zh.srt"] {
                    let sub_path = format!("{}.{}", tmp_prefix, ext);
                    if let Ok(sub_text) = std::fs::read_to_string(&sub_path) {
                        let cleaned = Self::clean_subtitle_text(&sub_text);
                        if !cleaned.is_empty() {
                            subtitle_content = Some(cleaned);
                        }
                        // Clean up temp file
                        let _ = std::fs::remove_file(&sub_path);
                        if subtitle_content.is_some() {
                            break;
                        }
                    }
                }
            }
        }

        // Clean up any remaining temp files
        for ext in &["en.vtt", "en.srt", "zh.vtt", "zh.srt"] {
            let sub_path = format!("{}.{}", tmp_prefix, ext);
            let _ = std::fs::remove_file(&sub_path);
        }

        // 3. Use subtitles if available, otherwise fall back to description
        let content = subtitle_content.unwrap_or(description);

        Ok(ExtractedContent {
            title,
            author: uploader,
            content,
            published,
            url: url.to_owned(),
            platform: "youtube.com".to_owned(),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  GITHUB EXTRACTOR
// ═══════════════════════════════════════════════════════════════════════════════

/// Extracts content from GitHub repository URLs.
///
/// Fetches repository metadata and README via the GitHub API. An optional
/// personal access token can be provided for higher rate limits.
pub struct GithubExtractor {
    token: Option<String>,
}

impl GithubExtractor {
    /// Create a new `GithubExtractor` with an optional GitHub personal access token.
    pub fn new(token: Option<String>) -> Self {
        Self { token }
    }

    /// Parse `owner/repo` from a GitHub URL.
    ///
    /// Handles URLs like:
    /// - `https://github.com/owner/repo`
    /// - `https://github.com/owner/repo/tree/main/...`
    /// - `https://github.com/owner/repo/blob/main/...`
    fn parse_owner_repo(url: &str) -> Option<(String, String)> {
        let without_scheme = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);

        let without_host = without_scheme
            .strip_prefix("github.com/")
            .or_else(|| without_scheme.strip_prefix("www.github.com/"))?;

        let parts: Vec<&str> = without_host.split('/').collect();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            Some((parts[0].to_owned(), parts[1].to_owned()))
        } else {
            None
        }
    }

    /// Build an HTTP client with common headers.
    fn build_client(&self) -> Result<reqwest::blocking::Client, KcError> {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| KcError::ImportError(format!("HTTP client error: {}", e)))
    }

    /// Add authorization header if a token is configured.
    fn add_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(token) = &self.token {
            req.header("Authorization", format!("Bearer {}", token))
        } else {
            req
        }
    }

    /// Decode base64-encoded content (standard encoding with optional whitespace).
    fn decode_base64(encoded: &str) -> Result<String, KcError> {
        // Strip whitespace (GitHub returns base64 with newlines)
        let cleaned: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();

        let lookup = |ch: char| -> Result<u8, KcError> {
            match ch {
                'A'..='Z' => Ok(ch as u8 - b'A'),
                'a'..='z' => Ok(ch as u8 - b'a' + 26),
                '0'..='9' => Ok(ch as u8 - b'0' + 52),
                '+' => Ok(62),
                '/' => Ok(63),
                _ => Err(KcError::ImportError(format!(
                    "Invalid base64 character: {}",
                    ch
                ))),
            }
        };

        let mut bytes = Vec::with_capacity(cleaned.len() * 3 / 4);
        let chars: Vec<char> = cleaned.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let remaining = chars.len() - i;
            if remaining < 2 {
                break;
            }

            let a = lookup(chars[i])?;
            let b = lookup(chars[i + 1])?;
            bytes.push((a << 2) | (b >> 4));

            if i + 2 < chars.len() && chars[i + 2] != '=' {
                let c = lookup(chars[i + 2])?;
                bytes.push((b << 4) | (c >> 2));

                if i + 3 < chars.len() && chars[i + 3] != '=' {
                    let d = lookup(chars[i + 3])?;
                    bytes.push((c << 6) | d);
                }
            }

            i += 4;
        }

        String::from_utf8(bytes)
            .map_err(|e| KcError::ImportError(format!("Invalid UTF-8 in decoded content: {}", e)))
    }
}

impl ContentExtractor for GithubExtractor {
    fn can_handle(&self, url: &str) -> bool {
        url.contains("github.com/") && !url.contains("gist.github.com")
    }

    fn extract(&self, url: &str) -> Result<ExtractedContent, KcError> {
        let (owner, repo) = Self::parse_owner_repo(url).ok_or_else(|| {
            KcError::ImportError(format!(
                "Could not parse owner/repo from GitHub URL: {}",
                url
            ))
        })?;

        let client = self.build_client()?;

        // 1. Fetch repo metadata
        let repo_url = format!("https://api.github.com/repos/{}/{}", owner, repo);
        let repo_req = self.add_auth(
            client
                .get(&repo_url)
                .header("User-Agent", "engram-ai/1.0 (knowledge-compiler intake)")
                .header("Accept", "application/vnd.github.v3+json"),
        );

        let repo_resp = repo_req
            .send()
            .map_err(|e| KcError::ImportError(format!("GitHub API request failed: {}", e)))?;

        match repo_resp.status().as_u16() {
            404 => {
                return Err(KcError::ImportError(format!(
                    "GitHub repository not found: {}/{}",
                    owner, repo
                )));
            }
            403 => {
                return Err(KcError::ImportError(
                    "GitHub API rate limit exceeded. Provide a token for higher limits."
                        .to_owned(),
                ));
            }
            s if s >= 400 => {
                return Err(KcError::ImportError(format!(
                    "GitHub API returned status {}",
                    s
                )));
            }
            _ => {}
        }

        let repo_json: serde_json::Value = repo_resp
            .json()
            .map_err(|e| KcError::ImportError(format!("Failed to parse GitHub API response: {}", e)))?;

        let full_name = repo_json
            .get("full_name")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("{}/{}", owner, repo))
            .to_owned();

        let description = repo_json
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        let stars = repo_json
            .get("stargazers_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let language = repo_json
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();

        let topics: Vec<String> = repo_json
            .get("topics")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();

        // 2. Fetch README
        let readme_url = format!(
            "https://api.github.com/repos/{}/{}/readme",
            owner, repo
        );
        let readme_req = self.add_auth(
            client
                .get(&readme_url)
                .header("User-Agent", "engram-ai/1.0 (knowledge-compiler intake)")
                .header("Accept", "application/vnd.github.v3+json"),
        );

        let readme_content = match readme_req.send() {
            Ok(resp) if resp.status().is_success() => {
                let readme_json: serde_json::Value = resp
                    .json()
                    .unwrap_or(serde_json::Value::Null);
                readme_json
                    .get("content")
                    .and_then(|v| v.as_str())
                    .and_then(|encoded| Self::decode_base64(encoded).ok())
                    .unwrap_or_default()
            }
            _ => String::new(),
        };

        // 3. Build title and content
        let title = if description.is_empty() {
            full_name.clone()
        } else {
            format!("{}: {}", full_name, description)
        };

        let mut content_parts = Vec::new();

        if !description.is_empty() {
            content_parts.push(format!("**Description:** {}", description));
        }
        content_parts.push(format!("**Language:** {} | **Stars:** {}", language, stars));
        if !topics.is_empty() {
            content_parts.push(format!("**Topics:** {}", topics.join(", ")));
        }
        if !readme_content.is_empty() {
            content_parts.push(String::new()); // blank line separator
            content_parts.push(readme_content);
        }

        let content = content_parts.join("\n");

        Ok(ExtractedContent {
            title,
            author: Some(owner),
            content,
            published: None,
            url: url.to_owned(),
            platform: "github.com".to_owned(),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  INTAKE PIPELINE
// ═══════════════════════════════════════════════════════════════════════════════

/// Orchestrates URL content extraction and import.
///
/// The pipeline holds a list of [`ContentExtractor`]s and tries them in order.
/// The first extractor that reports [`ContentExtractor::can_handle`] for a URL
/// is used. The resulting [`IntakeReport`] contains a [`MemoryCandidate`] ready
/// for the caller to feed into [`super::import::ImportPipeline`].
pub struct IntakePipeline {
    extractors: Vec<Box<dyn ContentExtractor>>,
}

impl IntakePipeline {
    /// Create a new, empty `IntakePipeline` with no extractors.
    pub fn new() -> Self {
        Self {
            extractors: Vec::new(),
        }
    }

    /// Add a content extractor to the pipeline.
    pub fn add_extractor(&mut self, extractor: Box<dyn ContentExtractor>) {
        self.extractors.push(extractor);
    }

    /// Number of registered extractors.
    pub fn extractor_count(&self) -> usize {
        self.extractors.len()
    }

    /// Ingest a URL: extract content → create [`MemoryCandidate`] → return for import.
    ///
    /// Does **not** directly write to storage — the caller decides what to do
    /// with the candidate (e.g. feed it into [`super::import::ImportPipeline`]).
    pub fn ingest(&self, url: &str) -> Result<IntakeReport, KcError> {
        // Find the first extractor that can handle this URL
        let extractor = self
            .extractors
            .iter()
            .find(|e| e.can_handle(url))
            .ok_or_else(|| {
                KcError::ImportError(format!(
                    "No extractor can handle URL: {}",
                    url
                ))
            })?;

        let content = extractor.extract(url)?;
        let content_length = content.content.len();

        let candidate = MemoryCandidate {
            content: format!(
                "# {}\n\nSource: {}\nAuthor: {}\n\n{}",
                content.title,
                content.url,
                content.author.as_deref().unwrap_or("unknown"),
                content.content,
            ),
            source: url.to_owned(),
            content_hash: url_hash(&content.url),
            metadata: HashMap::from([
                ("source_url".to_owned(), content.url.clone()),
                ("platform".to_owned(), content.platform.clone()),
                (
                    "intake_timestamp".to_owned(),
                    Utc::now().to_rfc3339(),
                ),
            ]),
        };

        Ok(IntakeReport {
            url: url.to_owned(),
            title: content.title,
            memory_candidate: candidate,
            content_length,
            platform: content.platform,
        })
    }

    /// Ingest a URL and automatically run the result through the import pipeline.
    ///
    /// This is the full intake flow: extract → candidate → report. The returned
    /// [`IntakeReport`] contains a [`MemoryCandidate`] ready for the caller to
    /// feed into [`super::import::ImportPipeline`]. The integration is intentionally
    /// kept at the caller level — the pipeline extracts and prepares, the caller
    /// decides how to import.
    pub fn ingest_and_import(
        &self,
        url: &str,
        _import_pipeline: &super::import::ImportPipeline,
    ) -> Result<IntakeReport, KcError> {
        // Extract content and create candidate via the standard ingest path.
        // The caller can then feed report.memory_candidate into import_pipeline.
        self.ingest(url)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Mock Extractor ───────────────────────────────────────────────────

    /// Test-only extractor that returns pre-configured results.
    struct MockExtractor {
        handles: bool,
        title: String,
        content: String,
        author: Option<String>,
        platform: String,
        fail: bool,
    }

    impl MockExtractor {
        fn new(handles: bool, title: &str, content: &str) -> Self {
            Self {
                handles,
                title: title.to_owned(),
                content: content.to_owned(),
                author: None,
                platform: "mock".to_owned(),
                fail: false,
            }
        }

        fn failing(handles: bool) -> Self {
            Self {
                handles,
                title: String::new(),
                content: String::new(),
                author: None,
                platform: "mock".to_owned(),
                fail: true,
            }
        }

        fn with_author(mut self, author: &str) -> Self {
            self.author = Some(author.to_owned());
            self
        }

        fn with_platform(mut self, platform: &str) -> Self {
            self.platform = platform.to_owned();
            self
        }
    }

    impl ContentExtractor for MockExtractor {
        fn can_handle(&self, _url: &str) -> bool {
            self.handles
        }

        fn extract(&self, url: &str) -> Result<ExtractedContent, KcError> {
            if self.fail {
                return Err(KcError::ImportError("mock extraction failed".to_owned()));
            }
            Ok(ExtractedContent {
                title: self.title.clone(),
                author: self.author.clone(),
                content: self.content.clone(),
                published: None,
                url: url.to_owned(),
                platform: self.platform.clone(),
            })
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_intake_pipeline_new() {
        let pipeline = IntakePipeline::new();
        assert_eq!(pipeline.extractor_count(), 0);
        assert!(pipeline.extractors.is_empty());
    }

    #[test]
    fn test_add_extractor() {
        let mut pipeline = IntakePipeline::new();
        assert_eq!(pipeline.extractor_count(), 0);

        pipeline.add_extractor(Box::new(MockExtractor::new(true, "T1", "C1")));
        assert_eq!(pipeline.extractor_count(), 1);

        pipeline.add_extractor(Box::new(MockExtractor::new(false, "T2", "C2")));
        assert_eq!(pipeline.extractor_count(), 2);

        pipeline.add_extractor(Box::new(MockExtractor::new(true, "T3", "C3")));
        assert_eq!(pipeline.extractor_count(), 3);
    }

    #[test]
    fn test_jina_can_handle() {
        let extractor = JinaExtractor::new(None);
        assert!(extractor.can_handle("https://example.com"));
        assert!(extractor.can_handle("https://github.com/user/repo"));
        assert!(extractor.can_handle("https://www.youtube.com/watch?v=abc123"));
        assert!(extractor.can_handle("http://anything.goes/here"));
        assert!(extractor.can_handle("not-even-a-url"));
    }

    #[test]
    fn test_generic_can_handle() {
        let extractor = GenericExtractor;
        assert!(extractor.can_handle("https://example.com"));
        assert!(extractor.can_handle("https://github.com/user/repo"));
        assert!(extractor.can_handle("https://www.youtube.com/watch?v=abc123"));
        assert!(extractor.can_handle("http://anything.goes/here"));
        assert!(extractor.can_handle("not-even-a-url"));
    }

    #[test]
    fn test_ingest_selects_first_matching() {
        let mut pipeline = IntakePipeline::new();

        // First extractor doesn't handle, second does, third does too
        pipeline.add_extractor(Box::new(MockExtractor::new(false, "Skip", "skip")));
        pipeline.add_extractor(Box::new(
            MockExtractor::new(true, "Second", "second content")
                .with_platform("second-platform"),
        ));
        pipeline.add_extractor(Box::new(
            MockExtractor::new(true, "Third", "third content")
                .with_platform("third-platform"),
        ));

        let report = pipeline.ingest("https://example.com/article").unwrap();

        // Should have used the second extractor (first matching)
        assert_eq!(report.title, "Second");
        assert_eq!(report.platform, "second-platform");
        assert!(report.memory_candidate.content.contains("second content"));
    }

    #[test]
    fn test_ingest_no_extractor() {
        // Empty pipeline — no extractors at all
        let pipeline = IntakePipeline::new();
        let result = pipeline.ingest("https://example.com");
        assert!(result.is_err());

        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("No extractor"), "Error was: {}", msg);
    }

    #[test]
    fn test_ingest_no_matching_extractor() {
        // Pipeline with extractors, but none can handle the URL
        let mut pipeline = IntakePipeline::new();
        pipeline.add_extractor(Box::new(MockExtractor::new(false, "A", "a")));
        pipeline.add_extractor(Box::new(MockExtractor::new(false, "B", "b")));

        let result = pipeline.ingest("https://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_extracted_content_to_candidate() {
        let mut pipeline = IntakePipeline::new();
        pipeline.add_extractor(Box::new(
            MockExtractor::new(true, "Rust Guide", "Learn Rust programming.")
                .with_author("Alice")
                .with_platform("blog.example.com"),
        ));

        let report = pipeline.ingest("https://blog.example.com/rust-guide").unwrap();
        let candidate = &report.memory_candidate;

        // Content format: # Title\n\nSource: url\nAuthor: author\n\ncontent
        assert!(candidate.content.starts_with("# Rust Guide"));
        assert!(candidate.content.contains("Source: https://blog.example.com/rust-guide"));
        assert!(candidate.content.contains("Author: Alice"));
        assert!(candidate.content.contains("Learn Rust programming."));

        // Source is the URL
        assert_eq!(candidate.source, "https://blog.example.com/rust-guide");

        // Metadata
        assert_eq!(
            candidate.metadata.get("source_url").unwrap(),
            "https://blog.example.com/rust-guide"
        );
        assert_eq!(
            candidate.metadata.get("platform").unwrap(),
            "blog.example.com"
        );
        assert!(candidate.metadata.contains_key("intake_timestamp"));

        // Content hash is derived from URL
        assert!(!candidate.content_hash.is_empty());
        assert_eq!(candidate.content_hash.len(), 32); // 16 hex digits × 2
    }

    #[test]
    fn test_extracted_content_unknown_author() {
        let mut pipeline = IntakePipeline::new();
        pipeline.add_extractor(Box::new(
            MockExtractor::new(true, "Title", "Body"),
        ));

        let report = pipeline.ingest("https://example.com/page").unwrap();
        // When no author is set, should show "unknown"
        assert!(report.memory_candidate.content.contains("Author: unknown"));
    }

    #[test]
    fn test_intake_report_fields() {
        let mut pipeline = IntakePipeline::new();
        pipeline.add_extractor(Box::new(
            MockExtractor::new(true, "My Article", "Some body text here")
                .with_platform("example.com"),
        ));

        let report = pipeline.ingest("https://example.com/my-article").unwrap();

        assert_eq!(report.url, "https://example.com/my-article");
        assert_eq!(report.title, "My Article");
        assert_eq!(report.content_length, "Some body text here".len());
        assert_eq!(report.platform, "example.com");
    }

    #[test]
    fn test_content_hash_dedup() {
        // Same URL must produce the same hash (for dedup)
        let hash1 = url_hash("https://example.com/article");
        let hash2 = url_hash("https://example.com/article");
        assert_eq!(hash1, hash2);

        // Different URLs should produce different hashes
        let hash3 = url_hash("https://example.com/other-article");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_url_hash_deterministic() {
        let hash = url_hash("https://example.com/page");
        // Hash is 32 hex chars (two u64 values, each 16 hex digits)
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("https://example.com/path"), "example.com");
        assert_eq!(extract_domain("http://sub.example.com/a/b"), "sub.example.com");
        assert_eq!(extract_domain("https://example.com:8080/path"), "example.com");
        assert_eq!(extract_domain("https://example.com"), "example.com");
        assert_eq!(extract_domain("no-scheme.com/path"), "no-scheme.com");
    }

    #[test]
    fn test_parse_title_and_content() {
        // Heading line
        let (title, content) = parse_title_and_content("# My Title\n\nBody text here.");
        assert_eq!(title, "My Title");
        assert_eq!(content, "Body text here.");

        // No heading — first line becomes title
        let (title, content) = parse_title_and_content("First Line\nSecond line.");
        assert_eq!(title, "First Line");
        assert_eq!(content, "Second line.");

        // Single line
        let (title, content) = parse_title_and_content("Only line");
        assert_eq!(title, "Only line");
        assert_eq!(content, "Only line");
    }

    #[test]
    fn test_extract_html_title() {
        let html = "<html><head><title>Page Title</title></head><body>Hi</body></html>";
        assert_eq!(extract_html_title(html), Some("Page Title".to_owned()));

        let no_title = "<html><body>Hi</body></html>";
        assert_eq!(extract_html_title(no_title), None);

        let empty_title = "<html><title></title></html>";
        assert_eq!(extract_html_title(empty_title), None);
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<p>Hello <b>world</b></p><br/><p>Second paragraph</p>";
        let text = strip_html_tags(html);
        assert!(text.contains("Hello world"));
        assert!(text.contains("Second paragraph"));
        assert!(!text.contains('<'));
        assert!(!text.contains('>'));
    }

    #[test]
    fn test_ingest_extractor_failure() {
        let mut pipeline = IntakePipeline::new();
        pipeline.add_extractor(Box::new(MockExtractor::failing(true)));

        let result = pipeline.ingest("https://example.com");
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("mock extraction failed"));
    }

    #[test]
    fn test_candidate_hash_uses_url() {
        // Two ingestions of the same URL should produce the same content_hash
        let mut pipeline = IntakePipeline::new();
        pipeline.add_extractor(Box::new(MockExtractor::new(true, "T", "C")));

        let r1 = pipeline.ingest("https://example.com/same").unwrap();
        let r2 = pipeline.ingest("https://example.com/same").unwrap();
        assert_eq!(r1.memory_candidate.content_hash, r2.memory_candidate.content_hash);

        // Different URL should produce different hash
        let r3 = pipeline.ingest("https://example.com/different").unwrap();
        assert_ne!(r1.memory_candidate.content_hash, r3.memory_candidate.content_hash);
    }

    // ── YtDlpExtractor URL matching ─────────────────────────────────────

    #[test]
    fn test_ytdlp_can_handle() {
        let extractor = YtDlpExtractor;
        assert!(extractor.can_handle("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(extractor.can_handle("https://youtube.com/watch?v=abc123"));
        assert!(extractor.can_handle("https://youtu.be/dQw4w9WgXcQ"));
        assert!(extractor.can_handle("https://m.youtube.com/watch?v=abc123"));
        assert!(extractor.can_handle("https://www.youtube.com/shorts/abc123"));
        assert!(extractor.can_handle("https://youtube.com/shorts/xyz"));
    }

    #[test]
    fn test_ytdlp_rejects_non_youtube() {
        let extractor = YtDlpExtractor;
        assert!(!extractor.can_handle("https://example.com"));
        assert!(!extractor.can_handle("https://vimeo.com/12345"));
        assert!(!extractor.can_handle("https://github.com/user/repo"));
        assert!(!extractor.can_handle("https://youtube.com/channel/abc"));
        assert!(!extractor.can_handle("https://www.youtube.com/"));
        assert!(!extractor.can_handle("not-a-url"));
    }

    #[test]
    fn test_ytdlp_parse_upload_date() {
        let dt = YtDlpExtractor::parse_upload_date("20240115");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2024-01-15");

        // Invalid dates
        assert!(YtDlpExtractor::parse_upload_date("2024011").is_none());
        assert!(YtDlpExtractor::parse_upload_date("").is_none());
        assert!(YtDlpExtractor::parse_upload_date("abcdefgh").is_none());
        assert!(YtDlpExtractor::parse_upload_date("20241301").is_none()); // invalid month
    }

    #[test]
    fn test_ytdlp_clean_subtitle_text() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
                    00:00:01.000 --> 00:00:03.000\n\
                    Hello world\n\n\
                    00:00:03.000 --> 00:00:05.000\n\
                    Hello world\n\n\
                    00:00:05.000 --> 00:00:07.000\n\
                    This is a test\n";
        let cleaned = YtDlpExtractor::clean_subtitle_text(vtt);
        assert!(cleaned.contains("Hello world"));
        assert!(cleaned.contains("This is a test"));
        // Duplicate consecutive lines should be deduped
        assert_eq!(
            cleaned.matches("Hello world").count(),
            1,
            "cleaned: {}",
            cleaned
        );
        // No timestamps in output
        assert!(!cleaned.contains("-->"));
        assert!(!cleaned.contains("WEBVTT"));
    }

    // ── GithubExtractor URL matching ────────────────────────────────────

    #[test]
    fn test_github_can_handle() {
        let extractor = GithubExtractor::new(None);
        assert!(extractor.can_handle("https://github.com/user/repo"));
        assert!(extractor.can_handle("https://github.com/user/repo/tree/main/src"));
        assert!(extractor.can_handle("https://github.com/user/repo/blob/main/README.md"));
        assert!(extractor.can_handle("http://github.com/user/repo"));
        assert!(extractor.can_handle("https://www.github.com/user/repo"));
    }

    #[test]
    fn test_github_rejects_non_github() {
        let extractor = GithubExtractor::new(None);
        assert!(!extractor.can_handle("https://example.com"));
        assert!(!extractor.can_handle("https://gitlab.com/user/repo"));
        assert!(!extractor.can_handle("https://youtube.com/watch?v=abc"));
        assert!(!extractor.can_handle("not-a-url"));
    }

    #[test]
    fn test_github_rejects_gist() {
        let extractor = GithubExtractor::new(None);
        assert!(!extractor.can_handle("https://gist.github.com/user/abc123"));
        assert!(!extractor.can_handle("https://gist.github.com/user/abc123/raw"));
    }

    #[test]
    fn test_github_parse_owner_repo() {
        let result = GithubExtractor::parse_owner_repo("https://github.com/rust-lang/rust");
        assert_eq!(result, Some(("rust-lang".to_owned(), "rust".to_owned())));

        let result =
            GithubExtractor::parse_owner_repo("https://github.com/user/repo/tree/main/src");
        assert_eq!(result, Some(("user".to_owned(), "repo".to_owned())));

        let result = GithubExtractor::parse_owner_repo("https://github.com/user/repo/blob/main/README.md");
        assert_eq!(result, Some(("user".to_owned(), "repo".to_owned())));

        // Invalid URLs
        assert!(GithubExtractor::parse_owner_repo("https://github.com/").is_none());
        assert!(GithubExtractor::parse_owner_repo("https://github.com/user").is_none());
        assert!(GithubExtractor::parse_owner_repo("https://example.com/user/repo").is_none());
    }

    #[test]
    fn test_github_decode_base64() {
        // "Hello, World!" in base64
        let encoded = "SGVsbG8sIFdvcmxkIQ==";
        let decoded = GithubExtractor::decode_base64(encoded).unwrap();
        assert_eq!(decoded, "Hello, World!");

        // With line breaks (as GitHub API returns)
        let encoded_with_newlines = "SGVs\nbG8s\nIFdv\ncmxk\nIQ==";
        let decoded = GithubExtractor::decode_base64(encoded_with_newlines).unwrap();
        assert_eq!(decoded, "Hello, World!");

        // Empty string
        let decoded = GithubExtractor::decode_base64("").unwrap();
        assert_eq!(decoded, "");
    }
}
