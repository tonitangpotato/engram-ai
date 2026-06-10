//! LLM-based memory extraction.
//!
//! Converts raw text into structured facts using LLMs. Optional feature
//! that preserves backward compatibility — if no extractor is set,
//! memories are stored as-is.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::error::Error;
use std::time::Duration;

// ============================================================================
// ISS-176: Retry / backoff for transient extractor HTTP failures
// ============================================================================
//
// Why this lives here, not in `Memory::store_raw`:
// By the time the error reaches `store_raw` it has been wrapped in
// `Quarantined(ExtractorError(...))` which has discarded the response
// status code — we can't classify retryability after the fact. The
// retry must happen inside the HTTP call site.
//
// Why not pull in `wiremock` / `httpmock`:
// We don't want a dev-dependency on an HTTP mock server just to test
// "did we retry?". Instead we extract the retry *decision* into a pure
// function (`classify_retry`) that is exhaustively unit-tested, and
// leave the I/O loop thin. Empirical validation of the real network
// path is AC-8 (ISS-175 probe re-run), not a unit test.

/// Tunable retry policy shared by the Anthropic and Ollama extractors.
///
/// Defaults (per ISS-176 design):
/// - `max_retries = 3`         → 4 total attempts before giving up
/// - `initial_backoff_ms = 500`
/// - `backoff_multiplier = 3.0`
/// - `max_backoff_ms = 10000`  → caps the third retry
/// - `enable_jitter = true`    → +/- 50% uniform jitter on every backoff
///
/// Setting `max_retries = 0` preserves pre-ISS-176 fail-fast behaviour
/// byte-identically (used by tests that need to assert specific failure
/// semantics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u8,
    pub initial_backoff_ms: u64,
    pub backoff_multiplier: f64,
    pub max_backoff_ms: u64,
    pub enable_jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 500,
            backoff_multiplier: 3.0,
            max_backoff_ms: 10_000,
            enable_jitter: true,
        }
    }
}

impl RetryConfig {
    /// Disable retries entirely. Used by tests that need to observe
    /// the raw (single-attempt) failure path.
    pub fn off() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }
}

/// Decision returned by `classify_retry` for a single attempt.
///
/// `RetryAfter(Duration)` carries the *exact* sleep duration the caller
/// should observe before the next attempt. The caller does not need
/// to compute jitter or check `max_backoff_ms` again — those are
/// applied here.
#[derive(Debug, Clone, PartialEq)]
pub enum RetryDecision {
    /// Sleep this long, then try again. `attempt` is already incremented
    /// for the next call.
    RetryAfter(Duration),
    /// Give up. Either the status is non-retryable, or we've exhausted
    /// `max_retries`. The caller should return the underlying error.
    GiveUp,
}

/// Failure category. `None` for `status` means a transport-level failure
/// (TCP/TLS/DNS — no HTTP response was received).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FailureKind {
    /// `reqwest::Error` with no status — DNS, TCP, TLS, connection-reset.
    Transport,
    /// HTTP response with a non-2xx status code.
    HttpStatus(u16),
}

/// Pure retry classifier. No I/O, no clock, no RNG — fully unit-testable.
///
/// `attempt` is 1-indexed: `attempt = 1` means the call just failed once.
/// Returns `GiveUp` when `attempt > max_retries` or when the failure is
/// permanent (4xx that isn't 408/429/auth).
///
/// Retryability rules (per ISS-176 Design table):
/// - Transport error                       → retryable
/// - 5xx (500/502/503/504)                 → retryable
/// - 429 (rate-limited)                    → retryable
/// - 408 (request timeout)                 → retryable
/// - 401/403 (auth)                        → retryable but capped (observed
///                                           empirically transient on OAuth;
///                                           we let the normal max_retries
///                                           handle the cap)
/// - Any other 4xx (400, 404, 422, …)      → permanent, give up immediately
pub fn classify_retry(failure: FailureKind, attempt: u8, cfg: &RetryConfig) -> RetryDecision {
    // Exhausted budget?
    if attempt > cfg.max_retries {
        return RetryDecision::GiveUp;
    }

    let retryable = match failure {
        FailureKind::Transport => true,
        FailureKind::HttpStatus(code) => match code {
            408 | 429 => true, // timeout / rate-limited
            500..=599 => true, // upstream brown-out
            401 | 403 => true, // observed transient on OAuth
            _ => false,        // 400, 404, 422, … permanent
        },
    };

    if !retryable {
        return RetryDecision::GiveUp;
    }

    RetryDecision::RetryAfter(compute_backoff(attempt, cfg))
}

/// Pure backoff calculator. Exponential growth, capped, optional jitter.
///
/// Schedule with defaults (initial=500ms, multiplier=3.0, cap=10s):
///   attempt 1 → 500ms   (+ 0..250 jitter)
///   attempt 2 → 1500ms  (+ 0..750 jitter)
///   attempt 3 → 4500ms  (+ 0..2250 jitter)
///   attempt 4 → 10000ms (capped, + 0..5000 jitter)
///
/// Jitter is *additive uniform* in [0, base/2). We don't use symmetric
/// jitter because we never want to shrink the backoff below the
/// exponential floor — quota windows reset on absolute time, not
/// relative.
fn compute_backoff(attempt: u8, cfg: &RetryConfig) -> Duration {
    // attempt is 1-indexed; first retry uses initial_backoff_ms unscaled.
    let exp = (attempt as i32).saturating_sub(1).max(0);
    let raw = (cfg.initial_backoff_ms as f64) * cfg.backoff_multiplier.powi(exp);
    let capped = raw.min(cfg.max_backoff_ms as f64);

    let base_ms = capped as u64;
    let jitter_ms = if cfg.enable_jitter {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        rng.gen_range(0..=(base_ms / 2).max(1))
    } else {
        0
    };
    Duration::from_millis(base_ms + jitter_ms)
}

/// Classify a `reqwest::Error` into a `FailureKind` for the retry layer.
///
/// `reqwest::Error::status()` returns `None` for transport-level errors
/// (DNS, TCP, TLS, connection-reset) and `Some(code)` only when the
/// error came from a non-2xx response — but in our code path we only
/// hit `?` on `send()` which fails for transport reasons. Status-based
/// errors are surfaced by the explicit `response.status().is_success()`
/// check after `send()` returns Ok.
pub(crate) fn classify_reqwest_error(err: &reqwest::Error) -> FailureKind {
    match err.status() {
        Some(s) => FailureKind::HttpStatus(s.as_u16()),
        None => FailureKind::Transport,
    }
}

/// Deserializer for dimensional fields that tolerates LLM output variance.
///
/// LLMs occasionally express "empty dimension" as `[]`, `null`, or `""`
/// instead of omitting the field. We also accept `["Alice", "Bob"]` arrays
/// and flatten them into a comma-separated string (provisional — see
/// tech debt ticket for future `Vec<String>` refactor).
///
/// Accepted inputs → output:
/// - `null` / missing       → `None`
/// - `""` / `"   "`         → `None`
/// - `"Alice"`              → `Some("Alice")`
/// - `[]`                   → `None`
/// - `["Alice"]`            → `Some("Alice")`
/// - `["Alice", "Bob"]`     → `Some("Alice, Bob")`
/// - `[null, "Alice", ""]`  → `Some("Alice")` (filter empty entries)
fn deserialize_flexible_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde_json::Value;

    let value: Option<Value> = Option::deserialize(deserializer)?;
    let value = match value {
        Some(v) => v,
        None => return Ok(None),
    };

    match value {
        Value::Null => Ok(None),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Value::Array(items) => {
            let parts: Vec<String> = items
                .into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => {
                        let t = s.trim();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t.to_string())
                        }
                    }
                    Value::Null => None,
                    other => Some(other.to_string()),
                })
                .collect();
            if parts.is_empty() {
                Ok(None)
            } else {
                Ok(Some(parts.join(", ")))
            }
        }
        // Numbers, bools, objects: coerce to string as last resort
        other => {
            let s = other.to_string();
            if s.is_empty() || s == "\"\"" {
                Ok(None)
            } else {
                Ok(Some(s))
            }
        }
    }
}

/// A single extracted fact from a conversation (dimensional format).
///
/// 11 semantic dimensions: core_fact (required) + 10 optional dimensions.
/// Type classification is inferred from dimension presence via `infer_type_weights()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    /// Core fact — the essential information (required). Maps to MemoryRecord.content.
    pub core_fact: String,
    /// Participants — who was involved
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub participants: Option<String>,
    /// Temporal — when it happened
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub temporal: Option<String>,
    /// Location / source
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub location: Option<String>,
    /// Background / surrounding situation
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub context: Option<String>,
    /// Cause / motivation
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub causation: Option<String>,
    /// Result / impact
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub outcome: Option<String>,
    /// How it was done / steps
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub method: Option<String>,
    /// Connections to other known things
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub relations: Option<String>,
    /// Emotional expression if present (e.g., frustrated, excited)
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub sentiment: Option<String>,
    /// Opinion / preference / position (e.g., prefers X over Y)
    #[serde(
        default,
        deserialize_with = "deserialize_flexible_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub stance: Option<String>,
    /// Importance score (0.0–1.0)
    pub importance: f64,
    /// Tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// Confidence: "confident" / "likely" / "uncertain"
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// Emotional valence: -1.0 to 1.0. Drives interoceptive emotion system.
    #[serde(default)]
    pub valence: f64,
    /// Domain: "coding" / "trading" / "research" / "communication" / "general"
    #[serde(default = "default_domain")]
    pub domain: String,
}

impl Default for ExtractedFact {
    fn default() -> Self {
        Self {
            core_fact: String::new(),
            participants: None,
            temporal: None,
            location: None,
            context: None,
            causation: None,
            outcome: None,
            method: None,
            relations: None,
            sentiment: None,
            stance: None,
            importance: 0.5,
            tags: Vec::new(),
            confidence: default_confidence(),
            valence: 0.0,
            domain: default_domain(),
        }
    }
}

/// Legacy format for backward-compatible parsing of old-style extraction responses.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // memory_type is read by serde deserialization
struct LegacyExtractedFact {
    pub content: String,
    #[serde(default)]
    pub memory_type: String,
    #[serde(default = "default_importance")]
    pub importance: f64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: String,
    #[serde(default)]
    pub valence: f64,
    #[serde(default = "default_domain")]
    pub domain: String,
}

fn default_importance() -> f64 {
    0.5
}

/// Wrapper for dimensional structured output: `{"memories": [...]}`.
#[derive(Debug, Deserialize)]
struct DimensionalResponse {
    memories: Vec<ExtractedFact>,
}

fn default_confidence() -> String {
    "likely".to_string()
}

fn default_domain() -> String {
    "general".to_string()
}

/// Trait for memory extraction — converts raw text into structured facts.
///
/// Implement this trait to use different LLM backends for extraction.
pub trait MemoryExtractor: Send + Sync {
    /// Extract key facts from raw conversation text.
    ///
    /// `reference` is the wall-clock time the episode occurred (when known).
    /// Implementations that call an LLM MUST inject it into the prompt so the
    /// model can resolve relative / duration time expressions to absolute
    /// dates at store time (ISS-190). When `None`, no reference is available
    /// and relative expressions are left unresolved.
    ///
    /// Returns empty vec if nothing worth remembering.
    /// Returns an error if the extraction fails (network, parsing, etc.).
    fn extract(
        &self,
        text: &str,
        reference: Option<DateTime<Utc>>,
    ) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>>;
}

/// Build the per-episode reference-date preamble injected ahead of the
/// conversation text (ISS-190). Returns an empty string when no reference is
/// known, so the prompt is byte-identical to the pre-ISS-190 behaviour for
/// callers that don't supply an `occurred_at`.
///
/// When a reference IS supplied, the model is told to resolve every relative /
/// duration time expression to an absolute date (preserving uncertainty —
/// "owned for 3 years" on 2023-03-27 → "~2020", year-granular) and to OMIT the
/// temporal field rather than fabricate one when no time cue exists.
fn reference_preamble(reference: Option<DateTime<Utc>>) -> String {
    match reference {
        Some(r) => format!(
            "This episode occurred on {}. When filling the temporal field, \
resolve the single most specific relative or duration time expression to one \
absolute date or year based on this reference (e.g. \"owned for 3 years\" \
stated on 2023-03-27 → \"~2020\"; \"two months ago\" → the resulting \
month/year). Emit ONLY the bare resolved value as ISO \"YYYY-MM-DD\" (or \
\"YYYY\" / \"~YYYY\" for year granularity) — no brackets, no parenthetical \
reasoning, no provenance note, no list of alternatives. Preserve uncertainty \
by keeping the \"~\" prefix or year-only granularity when the source is \
vague; do NOT invent a fake-precise day or time. If the text contains no time \
reference at all, OMIT the temporal field entirely — do NOT fabricate one.\n\n",
            r.format("%Y-%m-%d")
        ),
        None => String::new(),
    }
}

/// Assemble the windowed extraction input (ISS-162).
///
/// Prepends the preceding conversation turns (oldest-first) as
/// **coreference context only**, explicitly quarantined from extraction,
/// ahead of the current turn. This is the byte-for-byte framing proven in
/// `examples/iss201_window_verify.rs` (4/4 retrievable, context turns NOT
/// double-extracted) and validated end-to-end by the ISS-201 / ISS-162
/// isolation sweeps (conv-26 overall J 0.2697 → 0.3882 at window=4).
///
/// When `context` is empty, returns `turn` unchanged — the write path is
/// then byte-identical to the pre-ISS-162 behaviour.
pub(crate) fn assemble_with_context(context: &[String], turn: &str) -> String {
    if context.is_empty() {
        return turn.to_string();
    }
    format!(
        "Prior conversation context (for coreference resolution ONLY \
— do NOT extract facts from these lines; they are already stored):\n{}\n\n\
Extract facts ONLY from this final turn, resolving any pronouns or \
references against the context above so each core_fact is self-contained:\n{}",
        context.join("\n"),
        turn
    )
}

/// Specificity-preserving windowed extraction framing (ISS-218).
///
/// Identical to [`assemble_with_context`] but adds an explicit PRESERVATION
/// clause to the final-turn instruction. ISS-217's candidate-dump probe proved
/// the bare windowed framing's "make each core_fact self-contained" instruction
/// causes the LLM to *rewrite* the final turn and drop discriminating tokens —
/// proper-noun titles (q129 "Brave by Sara Bareilles" → dropped) and explicit
/// dates (q6/q20 → date anchor dropped) — which then re-embed differently and
/// churn the gold-bearing memory out of top-K (6 of 10 conv-26 window-losses).
///
/// The fix: the window may only ADD a resolved antecedent/date the final turn
/// lacked; it must NEVER replace or paraphrase a proper noun, title, or
/// explicit date the final turn already states. Empty context returns `turn`
/// unchanged (byte-identical to the no-window path), same as the bare variant.
///
/// Opt-in: the single call site selects this variant when `ENGRAM_WINDOW_PRESERVE`
/// is set truthy, so the proven default framing stays byte-identical until this
/// variant is benched (ISS-218 ACs).
pub(crate) fn assemble_with_context_preserving(context: &[String], turn: &str) -> String {
    if context.is_empty() {
        return turn.to_string();
    }
    format!(
        "Prior conversation context (for coreference resolution ONLY \
— do NOT extract facts from these lines; they are already stored):\n{}\n\n\
Extract facts ONLY from this final turn, resolving any pronouns or \
references against the context above so each core_fact is self-contained.\n\
PRESERVE VERBATIM every proper noun, title, name, and explicit date that \
already appears in this final turn — you may ADD a resolved antecedent or date \
drawn from the context above, but you must NEVER replace, omit, or paraphrase a \
specific token (a book/song title, a person's name, a calendar date) the final \
turn itself states:\n{}",
        context.join("\n"),
        turn
    )
}

/// The extraction prompt template (dimensional format).
///
/// Uses structured output with 11 semantic dimensions. LLM fills only dimensions
/// that are explicitly present in the text — no inference or fabrication.
const EXTRACTION_PROMPT: &str = r#"You are a memory extraction system. Extract key facts from the following conversation that are worth remembering long-term.

Rules:
- Extract concrete facts, preferences, decisions, and commitments
- Each fact should have a self-contained core_fact (understandable without context)
- Fill dimensional fields ONLY if the information is explicitly present — do NOT infer or fabricate
- Skip greetings, filler, acknowledgments
- Rate importance 0.0-1.0 (preferences=0.6, decisions=0.8, commitments=0.9)
- Rate confidence: "confident" (direct statement), "likely" (reasonable inference), "uncertain" (vague mention)
- If nothing worth remembering, return {"memories": []}
- Respond in the SAME LANGUAGE as the input

DO NOT extract any of these — return {"memories": []} if the input contains ONLY these:
- System instructions or agent identity setup ("You are X agent", "你是 XX", "Read SOUL.md", "Follow AGENTS.md")
- Tool/function schema definitions (JSON with "type", "properties", "required" describing tool parameters)
- Agent role/persona descriptions ("You are an AI assistant running on...", framework version info)
- Template operational reports with no decisions or events ("所有系统正常", "无新 commit", "Disk: XXG free")
- Raw config file contents (YAML/JSON configuration being loaded, not discussed)
- Heartbeat check results that are pure status repetition with no new information
- Memory recall results being echoed back (content starting with "Recalled Memories" or lists of previously stored memories)
- Trivial Q&A: single punctuation/emoji questions ("？", "ok", "👍") with filler responses ("嗯？怎么了", "收到", "好的")
- Already-known identity facts: username, timezone, Telegram ID — these are in config files, not memories
- Pure acknowledgments with no new information: "好的", "收到", "了解", "ok got it"
- Repetitive status pings: "还在跑吗" → "还在跑" (no new state change)

STILL extract from these (they contain real information):
- Conversations about system instructions (e.g., "let's update SOUL.md to add X") — the discussion IS worth remembering
- Heartbeat reports that discover actual issues (test failures, disk critical, new commits)
- Status reports with decisions or action items
- Any user preferences, requests, commitments, or decisions
- Short messages that contain actual decisions: "ok 那就用方案B" — extract the decision, not the "ok"

Respond with ONLY valid JSON (no markdown, no explanation):
{"memories": [
  {
    "core_fact": "What happened — the essential fact (REQUIRED)",
    "participants": "Who was involved — extract EVERY named person/agent/entity. Required if ANY person/agent is named anywhere in the text or in core_fact itself. Do NOT skip just because the name already appears in core_fact — still list it here. Only omit when truly nobody is named.",
    "temporal": "When it happened — emit EXACTLY ONE canonical value, never a list or concatenation of multiple time cues. Detect a time reference from FOUR sources: (1) absolute (2026-04-22, Monday, 3pm), (2) relative (yesterday, last week, just now, last year, next month), (3) duration (for 2 hours, 5 years), (4) GRAMMATICAL ASPECT that implies time: progressive ('is running', 'is planning' → 'ongoing'), future ('will', 'plans to', 'gonna' → 'future/planned'), perfect ('got married', 'has decided' → 'past/completed'), habitual ('every day', 'daily' → 'recurring'). When a reference date is supplied above, RESOLVE the SINGLE most specific reference to the absolute date it denotes and output it as a bare ISO date 'YYYY-MM-DD' (or 'YYYY' / '~YYYY' when only year granularity is justified — keep '~' for vague sources). Output ONLY the value: no surrounding brackets, no parenthetical reasoning, no provenance notes, no multiple alternatives. If several time cues appear, pick the one that dates the core event and discard the rest. Required if ANY time cue exists — including grammatical aspect alone (use the aspect keyword e.g. 'ongoing' when no concrete date is derivable). Only omit for pure stative descriptions with no time dimension (e.g. 'X is tall', 'Y is blue').",
    "location": "Where / in what context (omit if not mentioned)",
    "context": "Background / surrounding situation (omit if not relevant)",
    "causation": "Why it happened / motivation / trigger / purpose — extract if the text contains ANY of FOUR cue types: (A) Explicit connectors: because, since, due to, so, so that, in order to, 因为, 所以, 为了, triggered by, caused by, required for, leads to, results in. (B) Structural causation verbs where subject-verb-object expresses cause→effect: necessitates, penalizes, forces, enables, prevents, blocks, helps, encouraged, gave courage to, inspired, motivates, drives, 'lacks X needs Y', 'failure modes include X', 'X is needed for Y'. (C) Implicit reason clauses: 'X is lowest-risk because Y', 'X performs worse under Y', 'X is the chosen approach for Y reason'. (D) PURPOSE / GOAL / MOTIVATION — this is the MOST COMMON form in everyday conversation, do not miss it: 'X to Y' (infinitive of purpose: 'plans to continue education to explore careers' → causation = 'to explore careers'), 'X's goal is Y' → causation = Y, 'interested in X to Y' → causation = Y, 'values X as a source of motivation' → causation = 'X is source of motivation', 'finds motivation from X' → causation = X, 'pursuing X to support Y' → causation = 'to support Y', 'gained courage to X' → causation = led to X. Required if any cue exists. Only omit when the fact is purely descriptive with no why/purpose/motivation/enablement.",
    "outcome": "What resulted / impact (omit if not mentioned)",
    "method": "How it was done / steps (omit if not mentioned)",
    "relations": "Connections to other known things (omit if none)",
    "sentiment": "Emotional expression if present, e.g. frustrated, excited (omit if neutral)",
    "stance": "Opinion / preference / position if present (omit if none)",
    "importance": 0.0,
    "tags": ["tag1"],
    "confidence": "confident",
    "valence": 0.0,
    "domain": "general"
  }
]}

Field notes:
- core_fact (REQUIRED): The essential information
- importance (REQUIRED): 0.0-1.0 based on long-term relevance
- tags (REQUIRED): relevant keywords
- confidence (REQUIRED): confident | likely | uncertain
- valence (REQUIRED): -1.0 (very negative) to 1.0 (very positive). 0.0 = neutral. Consider speaker's emotional state.
- domain (REQUIRED): coding | trading | research | communication | general
- All other fields: include if information is present in the text OR in core_fact. Structural fields (participants, temporal, causation) MUST be filled when relevant information exists — do not skip them just because the info is already captured in core_fact. Fields are extraction of dimensions, not deduplication.

"#;

/// Configuration for Anthropic-based extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicExtractorConfig {
    /// Model to use (default: "claude-haiku-4-5-20251001")
    pub model: String,
    /// API base URL (default: "https://api.anthropic.com")
    pub api_url: String,
    /// Maximum tokens for response (default: 1024)
    pub max_tokens: usize,
    /// Request timeout in seconds (default: 30)
    pub timeout_secs: u64,
    /// Retry / backoff policy for transient HTTP failures (ISS-176).
    /// Defaults retry up to 3 times on transport errors, 5xx, 429, and
    /// transient auth errors. Set to `RetryConfig::off()` to disable.
    #[serde(default)]
    pub retry: RetryConfig,
}

impl Default for AnthropicExtractorConfig {
    fn default() -> Self {
        Self {
            model: "claude-haiku-4-5-20251001".to_string(),
            api_url: "https://api.anthropic.com".to_string(),
            max_tokens: 1024,
            timeout_secs: 30,
            retry: RetryConfig::default(),
        }
    }
}

/// Extracts facts using Anthropic Claude API.
///
/// Token provider trait for dynamic auth token resolution.
///
/// Implement this to provide tokens that auto-refresh (e.g., OAuth managed tokens).
/// The extractor calls `get_token()` before each request, so expired tokens
/// get refreshed transparently.
pub trait TokenProvider: Send + Sync {
    /// Get a valid auth token. May refresh if expired.
    fn get_token(&self) -> Result<String, Box<dyn Error + Send + Sync>>;
}

use crate::anthropic_client::StaticToken;

/// Supports both OAuth tokens (Claude Max) and API keys.
/// Haiku is recommended for cost/speed balance.
///
/// Auth tokens can be:
/// - Static (fixed string, backward compatible)
/// - Dynamic (via `TokenProvider` trait, auto-refreshes on each request)
pub struct AnthropicExtractor {
    config: AnthropicExtractorConfig,
    token_provider: Box<dyn TokenProvider>,
    is_oauth: bool,
    client: reqwest::blocking::Client,
}

impl AnthropicExtractor {
    /// Create a new AnthropicExtractor with a static token.
    ///
    /// # Arguments
    ///
    /// * `auth_token` - API key or OAuth token (fixed string)
    /// * `is_oauth` - True if using OAuth token (Claude Max), false for API key
    pub fn new(auth_token: &str, is_oauth: bool) -> Self {
        Self::with_config(auth_token, is_oauth, AnthropicExtractorConfig::default())
    }

    /// Create with a static token and custom config.
    pub fn with_config(auth_token: &str, is_oauth: bool, config: AnthropicExtractorConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to create HTTP client");

        Self {
            config,
            token_provider: Box::new(StaticToken(auth_token.to_string())),
            is_oauth,
            client,
        }
    }

    /// Create with a dynamic token provider (auto-refreshes on each request).
    ///
    /// Use this for OAuth managed tokens that may expire and need refresh.
    /// The provider's `get_token()` is called before each extraction request.
    pub fn with_token_provider(
        provider: Box<dyn TokenProvider>,
        is_oauth: bool,
        config: AnthropicExtractorConfig,
    ) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to create HTTP client");

        Self {
            config,
            token_provider: provider,
            is_oauth,
            client,
        }
    }

    /// Build the request headers based on auth type.
    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let token = self.token_provider.get_token()?;
        Ok(crate::anthropic_client::build_anthropic_headers(
            &token,
            self.is_oauth,
        ))
    }
}

impl MemoryExtractor for AnthropicExtractor {
    fn extract(
        &self,
        text: &str,
        reference: Option<DateTime<Utc>>,
    ) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
        let prompt = format!(
            "{}{}Conversation:\n{}",
            EXTRACTION_PROMPT,
            reference_preamble(reference),
            text
        );

        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "temperature": 0,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let url = format!("{}/v1/messages", self.config.api_url);

        // ISS-176: retry loop. `attempt` is 1-indexed and counts attempts
        // that have already FAILED — we sleep and increment between
        // attempts. Loop exits via:
        //   - Ok(response) with 2xx → fall through to parse
        //   - Ok(response) with retryable status → log, sleep, retry
        //   - Ok(response) with permanent status → return Err immediately
        //   - Err(transport) retryable → log, sleep, retry
        //   - GiveUp on either path → return the last error
        let response = {
            let mut attempt: u8 = 0;
            loop {
                let headers = self.build_headers()?;
                let send_result = self.client.post(&url).headers(headers).json(&body).send();

                match send_result {
                    Ok(resp) if resp.status().is_success() => break resp,
                    Ok(resp) => {
                        let status_code = resp.status().as_u16();
                        attempt = attempt.saturating_add(1);
                        match classify_retry(
                            FailureKind::HttpStatus(status_code),
                            attempt,
                            &self.config.retry,
                        ) {
                            RetryDecision::RetryAfter(delay) => {
                                log::warn!(
                                    "Anthropic API HTTP {} (attempt {}/{}), retrying in {}ms",
                                    status_code,
                                    attempt,
                                    self.config.retry.max_retries,
                                    delay.as_millis()
                                );
                                std::thread::sleep(delay);
                                continue;
                            }
                            RetryDecision::GiveUp => {
                                let status = resp.status();
                                let body = resp.text().unwrap_or_default();
                                return Err(
                                    format!("Anthropic API error {}: {}", status, body).into()
                                );
                            }
                        }
                    }
                    Err(err) => {
                        let failure = classify_reqwest_error(&err);
                        attempt = attempt.saturating_add(1);
                        match classify_retry(failure, attempt, &self.config.retry) {
                            RetryDecision::RetryAfter(delay) => {
                                log::warn!(
                                    "Anthropic transport error (attempt {}/{}): {} — retrying in {}ms",
                                    attempt,
                                    self.config.retry.max_retries,
                                    err,
                                    delay.as_millis()
                                );
                                std::thread::sleep(delay);
                                continue;
                            }
                            RetryDecision::GiveUp => return Err(err.into()),
                        }
                    }
                }
            }
        };

        let response_json: serde_json::Value = response.json()?;

        // Extract the text content from the response
        let content_text = response_json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .ok_or("Invalid response structure from Anthropic API")?;

        parse_extraction_response(content_text)
    }
}

/// Configuration for Ollama-based extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaExtractorConfig {
    /// Ollama host URL (default: "http://localhost:11434")
    pub host: String,
    /// Model to use (default: "llama3.2:3b")
    pub model: String,
    /// Request timeout in seconds (default: 60)
    pub timeout_secs: u64,
    /// Retry / backoff policy for transient HTTP failures (ISS-176).
    /// Ollama is local so failures are rarer, but the contract is
    /// uniform with Anthropic. Defaults to the standard 3-retry
    /// exponential schedule.
    #[serde(default)]
    pub retry: RetryConfig,
}

impl Default for OllamaExtractorConfig {
    fn default() -> Self {
        Self {
            host: "http://localhost:11434".to_string(),
            model: "llama3.2:3b".to_string(),
            timeout_secs: 60,
            retry: RetryConfig::default(),
        }
    }
}

/// Extracts facts using a local Ollama chat model.
///
/// Useful for local/private extraction without API costs.
pub struct OllamaExtractor {
    config: OllamaExtractorConfig,
    client: reqwest::blocking::Client,
}

impl OllamaExtractor {
    /// Create a new OllamaExtractor with the specified model.
    pub fn new(model: &str) -> Self {
        Self::with_config(OllamaExtractorConfig {
            model: model.to_string(),
            ..Default::default()
        })
    }

    /// Create a new OllamaExtractor with custom host and model.
    pub fn with_host(model: &str, host: &str) -> Self {
        Self::with_config(OllamaExtractorConfig {
            host: host.to_string(),
            model: model.to_string(),
            ..Default::default()
        })
    }

    /// Create a new OllamaExtractor with full config.
    pub fn with_config(config: OllamaExtractorConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to create HTTP client");

        Self { config, client }
    }
}

impl MemoryExtractor for OllamaExtractor {
    fn extract(
        &self,
        text: &str,
        reference: Option<DateTime<Utc>>,
    ) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
        let prompt = format!(
            "{}{}Conversation:\n{}",
            EXTRACTION_PROMPT,
            reference_preamble(reference),
            text
        );

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "stream": false
        });

        let url = format!("{}/api/chat", self.config.host);

        // ISS-176: retry loop. Mirrors AnthropicExtractor::extract — see
        // there for the loop contract. Ollama is local so transport
        // failures are rare, but the uniform retry policy means a
        // momentary `ollama serve` blip doesn't kill a bench run.
        let response = {
            let mut attempt: u8 = 0;
            loop {
                let send_result = self
                    .client
                    .post(&url)
                    .header("content-type", "application/json")
                    .json(&body)
                    .send();

                match send_result {
                    Ok(resp) if resp.status().is_success() => break resp,
                    Ok(resp) => {
                        let status_code = resp.status().as_u16();
                        attempt = attempt.saturating_add(1);
                        match classify_retry(
                            FailureKind::HttpStatus(status_code),
                            attempt,
                            &self.config.retry,
                        ) {
                            RetryDecision::RetryAfter(delay) => {
                                log::warn!(
                                    "Ollama API HTTP {} (attempt {}/{}), retrying in {}ms",
                                    status_code,
                                    attempt,
                                    self.config.retry.max_retries,
                                    delay.as_millis()
                                );
                                std::thread::sleep(delay);
                                continue;
                            }
                            RetryDecision::GiveUp => {
                                let status = resp.status();
                                let body = resp.text().unwrap_or_default();
                                return Err(format!("Ollama API error {}: {}", status, body).into());
                            }
                        }
                    }
                    Err(err) => {
                        let failure = classify_reqwest_error(&err);
                        attempt = attempt.saturating_add(1);
                        match classify_retry(failure, attempt, &self.config.retry) {
                            RetryDecision::RetryAfter(delay) => {
                                log::warn!(
                                    "Ollama transport error (attempt {}/{}): {} — retrying in {}ms",
                                    attempt,
                                    self.config.retry.max_retries,
                                    err,
                                    delay.as_millis()
                                );
                                std::thread::sleep(delay);
                                continue;
                            }
                            RetryDecision::GiveUp => return Err(err.into()),
                        }
                    }
                }
            }
        };

        let response_json: serde_json::Value = response.json()?;

        // Extract the message content from Ollama response
        let content_text = response_json
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or("Invalid response structure from Ollama API")?;

        parse_extraction_response(content_text)
    }
}

/// Parse LLM extraction response into ExtractedFacts.
///
/// Dual-path parser:
/// - Path 1: New dimensional format `{"memories": [{core_fact, ...}]}`
/// - Path 2: Legacy format `[{content, memory_type, importance, tags}]`
///
/// Handles common LLM quirks: markdown-wrapped JSON, extra whitespace.
fn parse_extraction_response(
    content: &str,
) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
    // Strip markdown code blocks if present
    let json_str = content
        .trim()
        .strip_prefix("```json")
        .or_else(|| content.trim().strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(content)
        .trim();

    // Path 1: Try new dimensional format {"memories": [...]}
    if let Ok(dimensional) = serde_json::from_str::<DimensionalResponse>(json_str) {
        let valid: Vec<ExtractedFact> = dimensional
            .memories
            .into_iter()
            .map(|mut f| {
                f.importance = f.importance.clamp(0.0, 1.0);
                f.valence = f.valence.clamp(-1.0, 1.0);
                f
            })
            .filter(|f| !f.core_fact.is_empty())
            .collect();
        return Ok(valid);
    }

    // Also try: the LLM might return just the array without the wrapper
    // (i.e., `[{core_fact: ...}]` instead of `{"memories": [...]}`)
    if let Some(start) = json_str.find('[') {
        if let Some(end) = json_str.rfind(']') {
            if start < end {
                let arr_str = &json_str[start..=end];
                // Try parsing as array of dimensional facts
                if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(arr_str) {
                    let valid: Vec<ExtractedFact> = facts
                        .into_iter()
                        .map(|mut f| {
                            f.importance = f.importance.clamp(0.0, 1.0);
                            f.valence = f.valence.clamp(-1.0, 1.0);
                            f
                        })
                        .filter(|f| !f.core_fact.is_empty())
                        .collect();
                    if !valid.is_empty() {
                        return Ok(valid);
                    }
                }
            }
        }
    }

    // Path 2: Legacy format [{content, memory_type, importance, ...}]
    // Handle empty array case
    if json_str.trim() == "[]"
        || json_str.contains(r#""memories": []"#)
        || json_str.contains(r#""memories":[]"#)
    {
        return Ok(vec![]);
    }

    let json_start = json_str.find('[');
    let json_end = json_str.rfind(']');

    let json_to_parse = match (json_start, json_end) {
        (Some(start), Some(end)) if start < end => &json_str[start..=end],
        _ => {
            log::warn!("No JSON array found in extraction response: {}", json_str);
            return Ok(vec![]);
        }
    };

    match serde_json::from_str::<Vec<LegacyExtractedFact>>(json_to_parse) {
        Ok(facts) => {
            let valid_facts: Vec<ExtractedFact> = facts
                .into_iter()
                .filter(|f| !f.content.is_empty())
                .map(|f| ExtractedFact {
                    core_fact: f.content,
                    importance: f.importance.clamp(0.0, 1.0),
                    tags: f.tags,
                    confidence: f.confidence,
                    valence: f.valence.clamp(-1.0, 1.0),
                    domain: f.domain,
                    // All dimensions empty — legacy format has none
                    ..Default::default()
                })
                .collect();
            Ok(valid_facts)
        }
        Err(e) => {
            log::warn!(
                "Failed to parse extraction JSON: {} - content: {}",
                e,
                json_to_parse
            );
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ISS-218: the specificity-preserving framing must (a) keep the empty-context
    /// byte-identity contract (no window → bare turn), (b) carry the preservation
    /// clause, and (c) leave every proper noun / title / explicit date the final
    /// turn states intact in the assembled input (the extractor input must never
    /// be the lossy side — ISS-217 proved the lossiness happens at extraction, so
    /// the input must at minimum still contain the tokens for the LLM to preserve).
    #[test]
    fn iss218_preserving_framing_keeps_specificity_tokens() {
        // (a) empty context → bare turn, byte-identical to no-window path.
        assert_eq!(
            assemble_with_context_preserving(&[], "Caroline loves 'Brave' by Sara Bareilles"),
            "Caroline loves 'Brave' by Sara Bareilles"
        );

        let ctx = vec!["What song means the most to you?".to_string()];
        let turn = "It's 'Brave' by Sara Bareilles, released on 2013-04-23";
        let out = assemble_with_context_preserving(&ctx, turn);

        // (b) preservation clause is present and explicit.
        assert!(
            out.contains("PRESERVE VERBATIM"),
            "preserving framing must carry the preservation clause"
        );
        assert!(
            out.to_lowercase().contains("never replace")
                || out.to_lowercase().contains("must never"),
            "clause must forbid replacing specific tokens"
        );

        // (c) every discriminating token the final turn states survives into the input.
        for tok in ["Brave", "Sara Bareilles", "2013-04-23"] {
            assert!(
                out.contains(tok),
                "specificity token {tok:?} must remain in the assembled input"
            );
        }

        // The context preamble must still be quarantined from extraction.
        assert!(out.contains("for coreference resolution ONLY"));
        // And it must differ from the bare ISS-162 framing (the clause is additive).
        assert_ne!(out, assemble_with_context(&ctx, turn));
    }

    /// ISS-162 path-equivalence: the LIBRARY windowing framing
    /// (`assemble_with_context`) must be byte-identical to the ISS-201
    /// ISOLATION framing (`examples/iss201_window_verify.rs::windowed`),
    /// which produced the +11.85pp conv-26 lift. If these ever drift, the
    /// library no longer reproduces the proven prompt and any lift delta is a
    /// code defect rather than an envelope difference.
    #[test]
    fn iss162_assemble_matches_isolation_framing() {
        // Verbatim transcription of the isolation `windowed()` body. Kept
        // here so the test detects drift on EITHER side.
        fn isolation_windowed(context: &[String], turn: &str) -> String {
            if context.is_empty() {
                return turn.to_string();
            }
            format!(
                "Prior conversation context (for coreference resolution ONLY \
— do NOT extract facts from these lines; they are already stored):\n{}\n\n\
Extract facts ONLY from this final turn, resolving any pronouns or \
references against the context above so each core_fact is self-contained:\n{}",
                context.join("\n"),
                turn
            )
        }

        let cases: Vec<(Vec<String>, &str)> = vec![
            // Turn 0: empty context → bare turn (the driver hits this every
            // conversation; the isolation example never did, so this is the
            // most important branch to pin).
            (vec![], "Luna and Oliver!"),
            // Single preceding turn.
            (
                vec!["Have you thought about adopting?".to_string()],
                "Researching adoption agencies",
            ),
            // Full window=4 (DEFAULT_WINDOW) context.
            (
                vec![
                    "Caroline asked about pet names.".to_string(),
                    "I suggested some options.".to_string(),
                    "What did you decide?".to_string(),
                    "We narrowed it down.".to_string(),
                ],
                "Luna and Oliver!",
            ),
        ];

        for (ctx, turn) in &cases {
            assert_eq!(
                assemble_with_context(ctx, turn),
                isolation_windowed(ctx, turn),
                "library framing diverged from isolation framing for ctx={ctx:?} turn={turn:?}"
            );
        }
    }

    #[test]
    fn iss190_reference_preamble_resolves_relative_time() {
        // q29 topology: episode states "owned for 3 years"; reference date
        // 2023-03-27 → the preamble must carry the date AND instruct the LLM
        // to resolve relative/duration expressions to an absolute year.
        use chrono::TimeZone;
        let reference = Utc.with_ymd_and_hms(2023, 3, 27, 0, 0, 0).unwrap();
        let preamble = reference_preamble(Some(reference));

        assert!(
            preamble.contains("2023-03-27"),
            "preamble must inject the reference date: {preamble}"
        );
        assert!(
            preamble.contains("absolute"),
            "preamble must instruct absolute resolution: {preamble}"
        );
        // Uncertainty preservation (deliberate divergence from Zep): the
        // worked example keeps the approximate "~2020" marker.
        assert!(
            preamble.contains("~2020"),
            "preamble must demonstrate uncertainty-preserving resolution: {preamble}"
        );
    }

    #[test]
    fn iss190_reference_preamble_absent_is_byte_identical_legacy() {
        // Negative guard: no reference → empty preamble, so the assembled
        // prompt is byte-identical to the pre-ISS-190 behaviour. This is the
        // "do NOT fabricate" safety net for callers without an occurred_at.
        assert_eq!(reference_preamble(None), "");
    }

    #[test]
    fn iss190_reference_preamble_forbids_fabrication() {
        // When a reference exists but the text has no time cue, the model
        // must OMIT rather than invent a temporal value.
        use chrono::TimeZone;
        let reference = Utc.with_ymd_and_hms(2023, 3, 27, 0, 0, 0).unwrap();
        let preamble = reference_preamble(Some(reference));
        assert!(
            preamble.contains("OMIT") && preamble.contains("fabricate"),
            "preamble must forbid fabrication when no time cue exists: {preamble}"
        );
    }

    #[test]
    fn test_parse_new_dimensional_format() {
        let response = r#"{"memories": [{"core_fact": "User prefers tea over coffee", "stance": "prefers tea", "importance": 0.6, "tags": ["preference"], "confidence": "confident", "valence": 0.1, "domain": "general"}]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "User prefers tea over coffee");
        assert_eq!(facts[0].stance.as_deref(), Some("prefers tea"));
        assert!((facts[0].importance - 0.6).abs() < 0.001);
    }

    #[test]
    fn test_parse_new_format_array_without_wrapper() {
        let response = r#"[{"core_fact": "Meeting at 3pm", "temporal": "3pm today", "importance": 0.7, "tags": ["meeting"], "confidence": "confident", "valence": 0.0, "domain": "communication"}]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Meeting at 3pm");
        assert_eq!(facts[0].temporal.as_deref(), Some("3pm today"));
    }

    #[test]
    fn test_parse_markdown_wrapped_new_format() {
        let response = r#"```json
{"memories": [{"core_fact": "Meeting scheduled for Friday", "temporal": "Friday", "importance": 0.8, "tags": [], "confidence": "confident", "valence": 0.0, "domain": "communication"}]}
```"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Meeting scheduled for Friday");
    }

    #[test]
    fn test_parse_legacy_format() {
        let response = r#"[{"content": "User prefers tea over coffee", "memory_type": "relational", "importance": 0.6}]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "User prefers tea over coffee");
        // Legacy format: no dimensional fields
        assert!(facts[0].participants.is_none());
        assert!(facts[0].temporal.is_none());
    }

    #[test]
    fn test_parse_legacy_with_surrounding_text() {
        let response = r#"Here are the extracted facts:
[{"content": "Project deadline is next week", "memory_type": "factual", "importance": 0.9}]
Hope this helps!"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Project deadline is next week");
    }

    #[test]
    fn test_parse_empty_array() {
        let response = "[]";
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn test_parse_empty_memories() {
        let response = r#"{"memories": []}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn test_parse_invalid_json() {
        let response = "This is not JSON at all";
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn test_parse_clamps_importance() {
        let response = r#"{"memories": [
            {"core_fact": "Low", "importance": -0.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"},
            {"core_fact": "High", "importance": 1.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"}
        ]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].importance, 0.0);
        assert_eq!(facts[1].importance, 1.0);
    }

    #[test]
    fn test_parse_filters_empty_core_fact() {
        let response = r#"{"memories": [
            {"core_fact": "", "importance": 0.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"},
            {"core_fact": "Valid fact", "importance": 0.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"}
        ]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Valid fact");
    }

    #[test]
    fn test_parse_legacy_filters_empty() {
        let response = r#"[
            {"content": "", "memory_type": "factual", "importance": 0.5},
            {"content": "Valid fact", "memory_type": "factual", "importance": 0.5}
        ]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Valid fact");
    }

    #[test]
    fn test_parse_multiple_dimensional_facts() {
        let response = r#"{"memories": [
            {"core_fact": "Fact 1", "importance": 0.3, "tags": [], "confidence": "confident", "valence": 0.0, "domain": "general"},
            {"core_fact": "Fact 2", "temporal": "yesterday", "importance": 0.7, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "coding"},
            {"core_fact": "Fact 3", "participants": "potato", "importance": 0.9, "tags": [], "confidence": "confident", "valence": 0.3, "domain": "communication"}
        ]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 3);
        assert!(facts[1].temporal.is_some());
        assert!(facts[2].participants.is_some());
    }

    #[test]
    fn test_parse_all_dimensions() {
        let response = r#"{"memories": [{
            "core_fact": "potato rewrote in Rust",
            "participants": "potato",
            "temporal": "yesterday",
            "location": "home office",
            "context": "Python was too slow",
            "causation": "performance bottleneck",
            "outcome": "rewrite completed",
            "method": "spent evening coding",
            "relations": "related to engramai project",
            "sentiment": "excited",
            "stance": "prefers Rust over Python for perf",
            "importance": 0.8,
            "tags": ["rust", "python"],
            "confidence": "confident",
            "valence": 0.6,
            "domain": "coding"
        }]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        let f = &facts[0];
        assert_eq!(f.core_fact, "potato rewrote in Rust");
        assert_eq!(f.participants.as_deref(), Some("potato"));
        assert_eq!(f.temporal.as_deref(), Some("yesterday"));
        assert_eq!(f.location.as_deref(), Some("home office"));
        assert_eq!(f.context.as_deref(), Some("Python was too slow"));
        assert_eq!(f.causation.as_deref(), Some("performance bottleneck"));
        assert_eq!(f.outcome.as_deref(), Some("rewrite completed"));
        assert_eq!(f.method.as_deref(), Some("spent evening coding"));
        assert_eq!(f.relations.as_deref(), Some("related to engramai project"));
        assert_eq!(f.sentiment.as_deref(), Some("excited"));
        assert_eq!(
            f.stance.as_deref(),
            Some("prefers Rust over Python for perf")
        );
        assert_eq!(f.valence, 0.6);
        assert_eq!(f.domain, "coding");
    }

    #[test]
    fn test_extraction_prompt_format() {
        assert!(EXTRACTION_PROMPT.contains("core_fact"));
        assert!(EXTRACTION_PROMPT.contains("SAME LANGUAGE"));
        assert!(EXTRACTION_PROMPT.contains("importance"));
        assert!(EXTRACTION_PROMPT.contains("dimensional"));
    }

    #[test]
    fn test_default_extracted_fact() {
        let fact = ExtractedFact::default();
        assert!(fact.core_fact.is_empty());
        assert!(fact.participants.is_none());
        assert!(fact.temporal.is_none());
        assert_eq!(fact.confidence, "likely");
        assert_eq!(fact.domain, "general");
        assert_eq!(fact.valence, 0.0);
    }

    #[test]
    fn test_flexible_dim_accepts_empty_array() {
        // LLM sometimes outputs empty arrays for "unknown" dimensions.
        // Regression test for ISS-021 storage coverage drop.
        let json = r#"[{"core_fact": "Test fact", "participants": [], "temporal": [], "causation": [], "importance": 0.5, "tags": []}]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Test fact");
        assert!(facts[0].participants.is_none());
        assert!(facts[0].temporal.is_none());
        assert!(facts[0].causation.is_none());
    }

    #[test]
    fn test_flexible_dim_accepts_null() {
        let json = r#"[{"core_fact": "Test", "participants": null, "temporal": null, "importance": 0.5, "tags": []}]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].participants.is_none());
        assert!(facts[0].temporal.is_none());
    }

    #[test]
    fn test_flexible_dim_accepts_empty_string() {
        let json = r#"[{"core_fact": "Test", "participants": "", "temporal": "   ", "importance": 0.5, "tags": []}]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].participants.is_none(), "empty string → None");
        assert!(facts[0].temporal.is_none(), "whitespace-only → None");
    }

    #[test]
    fn test_flexible_dim_accepts_single_string() {
        // Normal case still works
        let json =
            r#"[{"core_fact": "Test", "participants": "Alice", "importance": 0.5, "tags": []}]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts[0].participants.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_flexible_dim_accepts_string_array() {
        // LLM returns list — we join with ", " (provisional; see Vec<String> tech debt)
        let json = r#"[{"core_fact": "Test", "participants": ["Alice", "Bob"], "importance": 0.5, "tags": []}]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts[0].participants.as_deref(), Some("Alice, Bob"));
    }

    #[test]
    fn test_flexible_dim_filters_empty_items_in_array() {
        let json = r#"[{"core_fact": "Test", "participants": ["", "Alice", null, "   "], "importance": 0.5, "tags": []}]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts[0].participants.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_flexible_dim_mixed_forms_in_single_payload() {
        // Real-world LLM output mixing all forms in one fact
        let json = r#"[{
            "core_fact": "Mixed test",
            "participants": ["Alice"],
            "temporal": [],
            "causation": "because X",
            "outcome": null,
            "importance": 0.5,
            "tags": []
        }]"#;
        let facts = parse_extraction_response(json).unwrap();
        assert_eq!(facts[0].participants.as_deref(), Some("Alice"));
        assert!(facts[0].temporal.is_none());
        assert_eq!(facts[0].causation.as_deref(), Some("because X"));
        assert!(facts[0].outcome.is_none());
    }

    #[test]
    #[ignore] // Requires Ollama running locally
    fn test_ollama_extraction() {
        let extractor = OllamaExtractor::new("llama3.2:3b");
        let facts = extractor
            .extract(
                "I really love pizza, especially pepperoni. My favorite restaurant is Mario's.",
                None,
            )
            .unwrap();
        println!("Extracted facts: {:?}", facts);
    }

    #[test]
    #[ignore] // Requires Anthropic API key
    fn test_anthropic_extraction() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let extractor = AnthropicExtractor::new(&api_key, false);
        let facts = extractor
            .extract(
                "我昨天和小明一起去吃了火锅，很好吃。小明说他下周要去上海出差。",
                None,
            )
            .unwrap();
        println!("Extracted facts: {:?}", facts);
    }

    // ----------------------------------------------------------------------
    // ISS-176 retry classifier tests — pure-function coverage of the retry
    // decision matrix. No HTTP / sleeps involved; the I/O loop that consumes
    // these decisions is exercised empirically by the ISS-175 probe re-run
    // (AC-8).
    // ----------------------------------------------------------------------

    fn assert_retry(decision: &RetryDecision, expected_base_ms: u64, jitter_max_ms: u64) {
        match decision {
            RetryDecision::RetryAfter(d) => {
                let actual = d.as_millis() as u64;
                let max_allowed = expected_base_ms + jitter_max_ms;
                assert!(
                    actual >= expected_base_ms && actual <= max_allowed,
                    "expected retry delay in [{expected_base_ms}, {max_allowed}] ms, got {actual}"
                );
            }
            RetryDecision::GiveUp => panic!("expected RetryAfter, got GiveUp"),
        }
    }

    fn assert_give_up(decision: &RetryDecision) {
        assert!(
            matches!(decision, RetryDecision::GiveUp),
            "expected GiveUp, got {decision:?}"
        );
    }

    #[test]
    fn iss176_transport_error_retries_with_default_config() {
        let cfg = RetryConfig::default();
        // attempt 1 just failed → next backoff is initial_backoff_ms (500)
        // jitter is [0, base/2] = [0, 250]
        let decision = classify_retry(FailureKind::Transport, 1, &cfg);
        assert_retry(&decision, 500, 250);
    }

    #[test]
    fn iss176_backoff_grows_exponentially() {
        let cfg = RetryConfig {
            enable_jitter: false,
            ..RetryConfig::default()
        };
        // Without jitter, exact values are deterministic.
        assert_eq!(
            classify_retry(FailureKind::Transport, 1, &cfg),
            RetryDecision::RetryAfter(Duration::from_millis(500))
        );
        assert_eq!(
            classify_retry(FailureKind::Transport, 2, &cfg),
            RetryDecision::RetryAfter(Duration::from_millis(1500))
        );
        assert_eq!(
            classify_retry(FailureKind::Transport, 3, &cfg),
            RetryDecision::RetryAfter(Duration::from_millis(4500))
        );
    }

    #[test]
    fn iss176_backoff_capped_at_max() {
        let cfg = RetryConfig {
            enable_jitter: false,
            max_retries: 10,
            ..RetryConfig::default()
        };
        // attempt 4 → 500 * 3^3 = 13500ms, but capped at 10000ms
        assert_eq!(
            classify_retry(FailureKind::Transport, 4, &cfg),
            RetryDecision::RetryAfter(Duration::from_millis(10_000))
        );
        // attempt 5 → still capped
        assert_eq!(
            classify_retry(FailureKind::Transport, 5, &cfg),
            RetryDecision::RetryAfter(Duration::from_millis(10_000))
        );
    }

    #[test]
    fn iss176_exhausted_budget_gives_up() {
        let cfg = RetryConfig {
            max_retries: 3,
            ..RetryConfig::default()
        };
        // attempt 4 = one past max_retries → give up
        assert_give_up(&classify_retry(FailureKind::Transport, 4, &cfg));
        assert_give_up(&classify_retry(FailureKind::HttpStatus(503), 4, &cfg));
        assert_give_up(&classify_retry(FailureKind::HttpStatus(429), 4, &cfg));
    }

    #[test]
    fn iss176_retries_disabled_gives_up_immediately() {
        let cfg = RetryConfig::off();
        // Even on attempt 1, max_retries=0 means we already exhausted budget.
        assert_give_up(&classify_retry(FailureKind::Transport, 1, &cfg));
        assert_give_up(&classify_retry(FailureKind::HttpStatus(503), 1, &cfg));
    }

    #[test]
    fn iss176_5xx_statuses_retry() {
        let cfg = RetryConfig::default();
        for status in [500_u16, 502, 503, 504, 599] {
            let decision = classify_retry(FailureKind::HttpStatus(status), 1, &cfg);
            assert!(
                matches!(decision, RetryDecision::RetryAfter(_)),
                "expected retry on {status}, got {decision:?}"
            );
        }
    }

    #[test]
    fn iss176_429_and_408_retry() {
        let cfg = RetryConfig::default();
        assert!(matches!(
            classify_retry(FailureKind::HttpStatus(429), 1, &cfg),
            RetryDecision::RetryAfter(_)
        ));
        assert!(matches!(
            classify_retry(FailureKind::HttpStatus(408), 1, &cfg),
            RetryDecision::RetryAfter(_)
        ));
    }

    #[test]
    fn iss176_auth_errors_retry_within_budget() {
        let cfg = RetryConfig::default();
        // 401 and 403 are observed transient on OAuth — retry within budget.
        assert!(matches!(
            classify_retry(FailureKind::HttpStatus(401), 1, &cfg),
            RetryDecision::RetryAfter(_)
        ));
        assert!(matches!(
            classify_retry(FailureKind::HttpStatus(403), 1, &cfg),
            RetryDecision::RetryAfter(_)
        ));
        // But still bounded by max_retries.
        assert_give_up(&classify_retry(FailureKind::HttpStatus(401), 4, &cfg));
    }

    #[test]
    fn iss176_permanent_4xx_gives_up_immediately() {
        let cfg = RetryConfig::default();
        // 400 bad request, 404 model not found, 422 unprocessable → no point retrying.
        for status in [400_u16, 404, 405, 410, 422, 451] {
            assert_give_up(&classify_retry(FailureKind::HttpStatus(status), 1, &cfg));
        }
    }

    #[test]
    fn iss176_jitter_stays_within_bounds() {
        let cfg = RetryConfig::default();
        // Sample many times; every sample must be within [base, base + base/2].
        for _ in 0..100 {
            let d = classify_retry(FailureKind::Transport, 1, &cfg);
            assert_retry(&d, 500, 250);
        }
    }

    #[test]
    fn iss176_retry_config_off_preserves_pre_fix_behaviour() {
        let cfg = RetryConfig::off();
        assert_eq!(cfg.max_retries, 0);
        // Every attempt → GiveUp, regardless of failure kind.
        for failure in [
            FailureKind::Transport,
            FailureKind::HttpStatus(500),
            FailureKind::HttpStatus(429),
            FailureKind::HttpStatus(401),
            FailureKind::HttpStatus(400),
        ] {
            assert_give_up(&classify_retry(failure, 1, &cfg));
        }
    }
}
