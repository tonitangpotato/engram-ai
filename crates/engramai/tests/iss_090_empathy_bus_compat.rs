#![allow(deprecated, clippy::field_reassign_with_default)]

//! ISS-090 (AC-10): regression test for the deprecated
//! `add_with_emotion` shim's Empathy Bus side effect.
//!
//! Pre-ISS-090, `add_with_emotion` called `bus.process_interaction`
//! directly. ISS-090 made the shim delegate to `store_raw`, which
//! now drives the bus from inside the write path. The shim still
//! defensively calls `process_interaction` for backwards-compat
//! (see `// Empathy Bus interaction is now driven` in `memory.rs`).
//!
//! This test guards the storage path of that shim — it asserts:
//! 1. `add_with_emotion(...)` returns `Ok(non_empty_id)`.
//! 2. The record is recallable via `mem.recall(...)`.
//! 3. The Empathy Bus accumulator reflects the caller-supplied
//!    valence on the supplied domain (i.e. `process_interaction`
//!    fired and the `emotional_trends` row exists with the right
//!    valence prior).
//!
//! Full bus assertions beyond (3) (drive alignment boost, feedback
//! loop integration, multi-call averaging across paths) are
//! tracked separately and require richer infrastructure; this test
//! is intentionally narrow — it guards regression of the shim's
//! storage + bus-emit path so downstream callers that still use
//! `add_with_emotion` are not silently broken.

use engramai::bus::accumulator::EmpathyAccumulator;
use engramai::{Memory, MemoryConfig, MemoryType};
use std::fs;
use tempfile::TempDir;

fn setup_workspace() -> TempDir {
    let tmpdir = TempDir::new().unwrap();
    let workspace = tmpdir.path();

    fs::write(
        workspace.join("SOUL.md"),
        "# Core Drives\ncuriosity: learn deeply\nhonesty: be direct\n",
    )
    .unwrap();
    fs::write(
        workspace.join("HEARTBEAT.md"),
        "# Daily Tasks\n- [ ] consolidate memories\n",
    )
    .unwrap();
    fs::write(
        workspace.join("IDENTITY.md"),
        "name: TestAgent\ncreature: Cat\nvibe: focused\nemoji: 🐱\n",
    )
    .unwrap();

    tmpdir
}

#[test]
fn add_with_emotion_shim_fires_empathy_bus() {
    let tmpdir = setup_workspace();
    let db_path = tmpdir.path().join("iss090_bus_compat.db");

    let mut mem = Memory::with_empathy_bus(
        db_path.to_str().unwrap(),
        tmpdir.path().to_str().unwrap(),
        Some(MemoryConfig::default()),
    )
    .expect("memory boots with empathy bus");

    // Sanity: bus is actually attached.
    assert!(
        mem.empathy_bus().is_some(),
        "with_empathy_bus must attach a bus"
    );

    // Call the deprecated shim. Pre-ISS-090 this hit add_raw + bus
    // directly; post-ISS-090 it goes through store_raw and the bus
    // is driven from inside (with the shim retaining a defensive
    // process_interaction call for back-compat).
    #[allow(deprecated)]
    let id = mem
        .add_with_emotion(
            "trading is fun",
            MemoryType::Episodic,
            None,
            None,
            None,
            None,
            0.7,
            "trading",
        )
        .expect("add_with_emotion shim succeeds");

    assert!(!id.is_empty(), "shim must return a non-empty memory id");

    // (1) + (2): the record is stored AND recallable.
    let results = mem
        .recall("trading", 10, None, None)
        .expect("recall succeeds");
    assert!(
        results
            .iter()
            .any(|r| r.record.content.contains("trading is fun")),
        "stored content must be recallable; got {} results",
        results.len()
    );

    // (3): Empathy Bus accumulator reflects valence 0.7 on the
    // "trading" domain. We read it via a fresh EmpathyAccumulator
    // bound to the same connection — this is the exact storage
    // surface `process_interaction` writes to (see
    // `bus::accumulator::EmpathyAccumulator::record_emotion`).
    let acc =
        EmpathyAccumulator::new(mem.connection()).expect("accumulator binds to memory connection");
    let trend = acc
        .get_trend("trading")
        .expect("trend lookup ok")
        .expect("trading trend row must exist after add_with_emotion");

    assert_eq!(trend.domain, "trading");
    assert!(
        trend.count >= 1,
        "expected count >= 1 after one shim call, got {}",
        trend.count
    );
    // Running-average valence: with count >= 1 and only valence=0.7
    // values having been recorded for this domain in this DB, the
    // averaged value should equal 0.7 (allowing fp slack). If the
    // shim were also to (correctly) emit through store_raw's bus
    // hook with the same (0.7, "trading") pair, the running average
    // is still 0.7 — multiple identical samples don't move the mean.
    assert!(
        (trend.valence - 0.7).abs() < 1e-6,
        "expected averaged valence ~0.7, got {}",
        trend.valence
    );
}
