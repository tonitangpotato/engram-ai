//! Episode — v0.3 ingestion contract.
//!
//! `Episode` is the public input type for `Memory::add_episode`. It packages a
//! single ingested memory event with optional caller-supplied identity, session
//! affinity, timestamp, and free-form metadata. The resolution pipeline
//! (see `crates/engramai/src/resolution/`) consumes this structure and produces
//! v0.3 semantic graph nodes and edges.
//!
//! ## Field semantics
//!
//! - `id`: `None` → server mints a fresh UUID. `Some(uuid)` → caller-supplied
//!   idempotency key; ingesting the same id twice is a no-op (per ISS-041 spec
//!   in v03-resolution design §3).
//! - `text`: required raw episodic content. Must be non-empty (validated at
//!   `Memory::add_episode` boundary, not here).
//! - `session_id`: optional session affinity tag used for routing in the
//!   v03-graph-layer (see v03-graph-layer design §5.1). `None` → no session
//!   correlation.
//! - `when`: optional event timestamp. `None` → ingestion server uses
//!   `Utc::now()` at receive time.
//! - `metadata`: arbitrary user metadata as `serde_json::Value`. Persisted
//!   verbatim, not interpreted by the pipeline.
//!
//! ## Wire-format guarantees
//!
//! - `None` optional fields are **omitted** from JSON, not serialized as
//!   `null`. This keeps wire payloads compact and lets API contracts evolve.
//! - Unknown fields in incoming JSON are **rejected** (`deny_unknown_fields`).
//!   v0.3 is a fresh API; client-side typos must surface, not silently drop.
//!
//! ## Example
//!
//! ```
//! use engramai::resolution::Episode;
//! use serde_json::json;
//! use uuid::Uuid;
//!
//! // Minimal: only text. Server mints id, picks current time.
//! let ep = Episode::new("The cat sat on the mat");
//! assert!(ep.id.is_none());
//! assert!(ep.session_id.is_none());
//! assert!(ep.when.is_none());
//!
//! // Full: caller supplies idempotency key, session, and metadata.
//! let session = Uuid::new_v4();
//! let ep = Episode::new("Bob met Alice at the café")
//!     .with_id(Uuid::new_v4())
//!     .with_session(session)
//!     .with_metadata(json!({"source": "chat", "channel": "telegram"}));
//! assert!(ep.id.is_some());
//! assert_eq!(ep.session_id, Some(session));
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Public ingestion contract for `Memory::add_episode` (v0.3).
///
/// See module-level docs for field semantics and wire-format guarantees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Episode {
    /// Optional caller-supplied idempotency key. `None` → server mints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,

    /// Required raw episodic content.
    pub text: String,

    /// Optional session affinity tag. `None` → no session correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,

    /// Optional event timestamp. `None` → server uses `Utc::now()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<DateTime<Utc>>,

    /// Arbitrary user metadata. Defaults to `Value::Null`.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Episode {
    /// Construct a minimal Episode from text alone.
    ///
    /// All other fields default to `None` / `Value::Null` and may be set via
    /// the `with_*` builder methods.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: None,
            text: text.into(),
            session_id: None,
            when: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// Set the caller-supplied idempotency key.
    pub fn with_id(mut self, id: Uuid) -> Self {
        self.id = Some(id);
        self
    }

    /// Set the session affinity tag.
    pub fn with_session(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set the event timestamp.
    pub fn with_when(mut self, when: DateTime<Utc>) -> Self {
        self.when = Some(when);
        self
    }

    /// Set the user metadata payload.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn new_minimal_defaults() {
        let ep = Episode::new("hello");
        assert_eq!(ep.text, "hello");
        assert!(ep.id.is_none());
        assert!(ep.session_id.is_none());
        assert!(ep.when.is_none());
        assert_eq!(ep.metadata, serde_json::Value::Null);
    }

    #[test]
    fn builder_chain_sets_each_field() {
        let id = Uuid::new_v4();
        let session = Uuid::new_v4();
        let when = Utc.with_ymd_and_hms(2026, 4, 29, 6, 30, 0).unwrap();
        let meta = json!({"k": "v"});

        let ep = Episode::new("text")
            .with_id(id)
            .with_session(session)
            .with_when(when)
            .with_metadata(meta.clone());

        assert_eq!(ep.id, Some(id));
        assert_eq!(ep.session_id, Some(session));
        assert_eq!(ep.when, Some(when));
        assert_eq!(ep.metadata, meta);
    }

    #[test]
    fn serde_round_trip_minimal() {
        let ep = Episode::new("minimal text");
        let json_str = serde_json::to_string(&ep).expect("serialize");
        let back: Episode = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(ep, back);
    }

    #[test]
    fn serde_round_trip_full() {
        let ep = Episode::new("full payload")
            .with_id(Uuid::new_v4())
            .with_session(Uuid::new_v4())
            .with_when(Utc.with_ymd_and_hms(2026, 4, 29, 12, 0, 0).unwrap())
            .with_metadata(json!({
                "nested": {"a": 1, "b": [true, false]},
                "tag": "test"
            }));

        let json_str = serde_json::to_string(&ep).expect("serialize");
        let back: Episode = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(ep, back);
    }

    #[test]
    fn serde_skips_none_optionals_on_wire() {
        let ep = Episode::new("only text");
        let json_str = serde_json::to_string(&ep).expect("serialize");
        // None fields must NOT appear as nulls — wire stays clean.
        assert!(!json_str.contains("\"id\""), "id should be omitted: {json_str}");
        assert!(
            !json_str.contains("\"session_id\""),
            "session_id should be omitted: {json_str}"
        );
        assert!(
            !json_str.contains("\"when\""),
            "when should be omitted: {json_str}"
        );
        // text and metadata always present.
        assert!(json_str.contains("\"text\""));
        assert!(json_str.contains("\"metadata\""));
    }

    #[test]
    fn deserialize_unknown_field_rejected() {
        // deny_unknown_fields → strict ingestion contract.
        let bad = r#"{"text":"hi","unexpected_field":"oops"}"#;
        let result: Result<Episode, _> = serde_json::from_str(bad);
        assert!(
            result.is_err(),
            "unknown field must be rejected; got Ok({:?})",
            result.ok()
        );
    }
}
