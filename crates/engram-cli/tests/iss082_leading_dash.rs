//! ISS-082: leading-dash payloads must not crash the CLI.
//!
//! `engram store`, `recall`, and `reward` historically took their primary
//! payload as a positional argument. Clap parses positional values that
//! start with `-` or `--` as unknown flags, so `engram store ... -0` etc.
//! exited with code 2 before any command logic ran. This test pins the
//! ISS-082 fix: each command grew a sibling `--content` / `--query` /
//! `--feedback` flag (mutually exclusive with the positional form) so
//! callers can safely transport leading-dash payloads.

use std::process::Command;

fn engram_bin() -> &'static str {
    env!("CARGO_BIN_EXE_engram")
}

#[test]
fn store_accepts_leading_dash_payload_via_content_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss082-store.db");

    // Payload starts with '-' — would crash clap as positional.
    let payload = "-0 reward signal applied at turn 92";

    let store = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "--no-graph",
            "--content",
            payload,
        ])
        .output()
        .expect("spawn engram store");

    assert!(
        store.status.success(),
        "store --content with leading-dash payload must succeed; got stdout={} stderr={}",
        String::from_utf8_lossy(&store.stdout),
        String::from_utf8_lossy(&store.stderr),
    );

    // Confirm the row landed by recalling it.
    let recall = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "recall",
            "--query",
            "reward signal turn",
            "--limit",
            "5",
            "--json",
        ])
        .output()
        .expect("spawn recall");
    assert!(recall.status.success());

    let arr: serde_json::Value =
        serde_json::from_slice(&recall.stdout).expect("json");
    let any_match = arr
        .as_array()
        .map(|a| {
            a.iter().any(|r| {
                r.pointer("/record/content")
                    .and_then(|c| c.as_str())
                    .map(|s| s.contains("reward signal applied at turn 92"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(
        any_match,
        "stored row not found in recall results; recall stdout:\n{}",
        String::from_utf8_lossy(&recall.stdout)
    );
}

#[test]
fn recall_accepts_leading_dash_query_via_query_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss082-recall.db");

    // Seed a row so recall has something to match on.
    let seed = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "--no-graph",
            "negative-prefix probe content",
        ])
        .output()
        .expect("spawn seed");
    assert!(seed.status.success());

    // Query starting with '-' — uses the new --query flag form.
    let recall = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "recall",
            "--query",
            "-negative-prefix probe",
            "--limit",
            "5",
            "--json",
        ])
        .output()
        .expect("spawn recall");
    assert!(
        recall.status.success(),
        "recall --query with leading-dash must succeed; stderr={}",
        String::from_utf8_lossy(&recall.stderr),
    );
}

#[test]
fn store_positional_and_flag_form_are_mutually_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss082-conflict.db");

    let out = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "--no-graph",
            "positional payload",
            "--content",
            "flag payload",
        ])
        .output()
        .expect("spawn store");

    assert!(
        !out.status.success(),
        "store with BOTH positional and --content must fail (clap conflicts_with)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("conflict") || stderr.contains("cannot be used with"),
        "error must explain the conflict; got stderr:\n{stderr}"
    );
}

#[test]
fn store_requires_either_positional_or_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss082-missing.db");

    let out = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "--no-graph",
        ])
        .output()
        .expect("spawn store");

    assert!(
        !out.status.success(),
        "store with neither positional nor --content must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing content"),
        "error must be diagnostic; got stderr:\n{stderr}"
    );
}

#[test]
fn store_positional_form_still_works_for_normal_payloads() {
    // Pin backward compatibility: the existing positional form must
    // continue to accept non-dash payloads. This is what every existing
    // caller / script uses; ISS-082 fix MUST NOT regress this.
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("iss082-compat.db");

    let store = Command::new(engram_bin())
        .args([
            "-d",
            db.to_str().unwrap(),
            "store",
            "--no-graph",
            "ordinary positional payload",
        ])
        .output()
        .expect("spawn store");

    assert!(
        store.status.success(),
        "positional payload must still work for backward compat; stderr={}",
        String::from_utf8_lossy(&store.stderr),
    );
}
