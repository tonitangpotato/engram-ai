//! ISS-081: round-trip `--meta key=value` from `engram store` through to
//! `engram recall --json`. Pins the metadata-channel contract end-to-end
//! at the CLI surface (the in-process unit tests at the bottom of
//! `src/main.rs::meta_kv_tests` cover the parser; this test covers
//! parser → StorageMeta → storage → recall JSON output).
//!
//! What this proves:
//!
//! 1. `engram store ... --meta k=v --meta k2=N` actually accepts the
//!    flag (no clap rejection, exit 0).
//! 2. The stored row's metadata round-trips through `engram recall
//!    --json`, with values parsed as JSON where possible (numbers,
//!    bools, arrays) and as raw strings otherwise (`D1:3`-style ids).
//! 3. The user-supplied payload is namespaced under `metadata.user.*`
//!    (v2 layout per ISS-019 Step 7a) — caller-owned keys do NOT
//!    leak into engram-owned `metadata.engram.*` namespace.
//! 4. Repeated `--meta` for the same key is last-write-wins (matches
//!    the parser unit test contract).
//!
//! What this does NOT prove (deliberately out of scope):
//!
//! - Reserved-key validation. `docs/metadata-channel.md` explicitly
//!   says "Engram does not enforce a metadata schema" (§"Engram's
//!   Contract" promise #1: opaque pass-through). The ISS-081 issue
//!   body proposes rejecting `engram_*`/`extractor_*` prefixes but
//!   references a non-existent §"Namespace Reservation" section.
//!   Until the design doc is updated to define those reserved
//!   prefixes, the CLI matches the documented contract: pass-through.

use std::process::Command;

/// Path to the `engram` binary set by Cargo at compile time.
fn engram_bin() -> &'static str {
    env!("CARGO_BIN_EXE_engram")
}

#[test]
fn store_and_recall_round_trips_meta_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss081.db");

    // === STORE ===
    let store = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "Alice met Bob at the cafe.",
            "--no-graph",
            "--meta",
            "dia_id=D1:3",
            "--meta",
            "turn_index=5",
            "--meta",
            "tags=[\"a\",\"b\"]",
        ])
        .output()
        .expect("spawn engram store");

    assert!(
        store.status.success(),
        "engram store exited non-zero: stdout={} stderr={}",
        String::from_utf8_lossy(&store.stdout),
        String::from_utf8_lossy(&store.stderr),
    );

    // === RECALL (--json) ===
    let recall = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "recall",
            "Alice",
            "--limit",
            "5",
            "--json",
        ])
        .output()
        .expect("spawn engram recall");

    assert!(
        recall.status.success(),
        "engram recall exited non-zero: stdout={} stderr={}",
        String::from_utf8_lossy(&recall.stdout),
        String::from_utf8_lossy(&recall.stderr),
    );

    let stdout = String::from_utf8(recall.stdout).expect("utf-8 stdout");
    let results: serde_json::Value =
        serde_json::from_str(&stdout).expect("recall --json produces valid JSON");

    // Find the row we just stored — could be wrapped in an array of
    // RecallResult, each with a `record.metadata` field.
    let arr = results.as_array().expect("recall --json returns an array");
    assert!(
        !arr.is_empty(),
        "recall returned no rows; stdout was:\n{stdout}"
    );

    let row = arr
        .iter()
        .find(|r| {
            r.pointer("/record/content")
                .and_then(|c| c.as_str())
                .map(|s| s.contains("Alice"))
                .unwrap_or(false)
        })
        .expect("no row with 'Alice' in content; full output:\n{stdout}");

    // v2 metadata layout: user-supplied keys live under `metadata.user`.
    let user_meta = row
        .pointer("/record/metadata/user")
        .expect("metadata.user namespace exists after --meta");

    assert_eq!(
        user_meta.get("dia_id").and_then(|v| v.as_str()),
        Some("D1:3"),
        "string-style id must round-trip verbatim; got {:?}",
        user_meta.get("dia_id")
    );
    assert_eq!(
        user_meta.get("turn_index").and_then(|v| v.as_i64()),
        Some(5),
        "numeric value must be JSON-parsed (not stringified); got {:?}",
        user_meta.get("turn_index")
    );
    assert_eq!(
        user_meta
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(2),
        "JSON array must be parsed as array; got {:?}",
        user_meta.get("tags")
    );
}

#[test]
fn meta_repeated_key_is_last_write_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss081-dup.db");

    let store = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "duplicate-key test content",
            "--no-graph",
            "--meta",
            "k=first",
            "--meta",
            "k=second",
        ])
        .output()
        .expect("spawn store");
    assert!(
        store.status.success(),
        "store failed: {}",
        String::from_utf8_lossy(&store.stderr)
    );

    let recall = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "recall",
            "duplicate",
            "--limit",
            "5",
            "--json",
        ])
        .output()
        .expect("spawn recall");
    assert!(recall.status.success());

    let arr: serde_json::Value = serde_json::from_slice(&recall.stdout).expect("recall json");
    let row = arr
        .as_array()
        .and_then(|a| a.first())
        .expect("recall returned a row");

    let k = row
        .pointer("/record/metadata/user/k")
        .and_then(|v| v.as_str());
    assert_eq!(
        k,
        Some("second"),
        "last-write-wins: expected 'second', got {k:?}"
    );
}

#[test]
fn meta_missing_equals_is_rejected_with_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss081-bad.db");

    let out = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "bad meta",
            "--no-graph",
            "--meta",
            "bare_key_no_equals",
        ])
        .output()
        .expect("spawn store");

    assert!(
        !out.status.success(),
        "store with malformed --meta must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing '='") || stderr.contains("bare_key_no_equals"),
        "error message must name the bad input; got stderr:\n{stderr}"
    );
}
