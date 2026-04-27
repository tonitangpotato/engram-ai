//! Round-trip tests for the v0.3 graph-layer Memory API additions
//! (task `graph-impl-memory-api`, design §5).
//!
//! Each test writes through `Memory::graph_mut()` (the new exclusive
//! write view) and reads back through one of the new convenience
//! methods (`get_entity`, `find_entity`, `neighbors`, `edges_as_of`,
//! `list_failed_episodes`). The point is to verify the *Memory API
//! surface*, not the underlying graph store — store-level invariants
//! are covered by the unit tests inside `graph::store`.
//!
//! Defers per task scope (separately tracked):
//!
//! * `add_episode` — ISS-041 (`Episode` struct lives in v03-resolution).
//! * `reextract_episodes` — ISS-042 (`ReextractReport` lives in v03-resolution).
//! * `graph(&self) -> &dyn GraphRead` — ISS-040 (storage-layer refactor).

use chrono::Utc;
use engramai::graph::{
    edge::{Edge, EdgeEnd},
    entity::{Entity, EntityKind},
    schema::{CanonicalPredicate, Predicate},
    store::GraphWrite, // for `graph_mut().insert_*`
};
use engramai::memory::Memory;
use uuid::Uuid;

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

// -----------------------------------------------------------------
// get_entity
// -----------------------------------------------------------------

#[test]
fn get_entity_returns_none_for_unknown_id() {
    let mut mem = new_mem();
    let got = mem.get_entity(Uuid::new_v4()).expect("read ok");
    assert!(got.is_none(), "unknown id must read as None, not error");
}

#[test]
fn get_entity_round_trips_a_freshly_inserted_entity() {
    let mut mem = new_mem();
    let now = Utc::now();
    let e = Entity::new("Alice".into(), EntityKind::Person, now);
    let id = e.id;

    mem.graph_mut().insert_entity(&e).expect("insert ok");

    let got = mem
        .get_entity(id)
        .expect("read ok")
        .expect("entity present");
    assert_eq!(got.id, id);
    assert_eq!(got.canonical_name, "Alice");
    assert_eq!(got.kind, EntityKind::Person);
}

// -----------------------------------------------------------------
// find_entity (alias resolution)
// -----------------------------------------------------------------

#[test]
fn find_entity_returns_none_when_no_alias_matches() {
    let mut mem = new_mem();
    let got = mem.find_entity("nobody-here").expect("read ok");
    assert!(got.is_none());
}

#[test]
fn find_entity_resolves_canonical_name_via_alias_table() {
    // `find_entity` delegates to `resolve_alias`, which reads from the
    // `graph_entity_aliases` table. `insert_entity` does **not**
    // auto-register the canonical name — alias upsert is a separate
    // write. We exercise the full path here so the test pins what
    // `find_entity` actually surfaces (registered aliases), not an
    // imagined auto-registration that doesn't exist.
    let mut mem = new_mem();
    let now = Utc::now();
    let e = Entity::new("Carol".into(), EntityKind::Person, now);
    let id = e.id;

    {
        let mut g = mem.graph_mut();
        g.insert_entity(&e).expect("insert ok");
        g.upsert_alias(
            /* normalized = */ "carol",
            /* alias_raw  = */ "Carol",
            /* canonical  = */ id,
            /* source_ep  = */ None,
        )
        .expect("upsert alias ok");
    }

    let got = mem.find_entity("Carol").expect("read ok");
    assert_eq!(
        got,
        Some(id),
        "find_entity should resolve a registered alias to the entity id"
    );
}

// -----------------------------------------------------------------
// neighbors (BFS over canonical predicates)
// -----------------------------------------------------------------

#[test]
fn neighbors_returns_empty_for_isolated_node() {
    let mut mem = new_mem();
    let now = Utc::now();
    let e = Entity::new("Loner".into(), EntityKind::Person, now);
    let id = e.id;
    mem.graph_mut().insert_entity(&e).expect("insert ok");

    let got = mem.neighbors(id, /* max_depth = */ 3).expect("read ok");
    assert!(got.is_empty(), "no edges → no neighbors, got {got:?}");
}

#[test]
fn neighbors_walks_one_hop_over_a_canonical_edge() {
    let mut mem = new_mem();
    let now = Utc::now();

    let alice = Entity::new("Alice".into(), EntityKind::Person, now);
    let acme = Entity::new("Acme".into(), EntityKind::Concept, now);
    let alice_id = alice.id;
    let acme_id = acme.id;

    {
        let mut g = mem.graph_mut();
        g.insert_entity(&alice).expect("insert alice");
        g.insert_entity(&acme).expect("insert acme");

        let edge = Edge::new(
            alice_id,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: acme_id },
            Some(now),
            now,
        );
        g.insert_edge(&edge).expect("insert edge");
    }

    let got = mem.neighbors(alice_id, /* max_depth = */ 1).expect("read ok");
    assert_eq!(got.len(), 1, "expected exactly one 1-hop neighbor, got {got:?}");
    assert_eq!(got[0].0, acme_id, "neighbor id should be Acme");
    assert_eq!(
        got[0].1.subject_id, alice_id,
        "edge should be the Alice→Acme one we inserted"
    );
}

// -----------------------------------------------------------------
// edges_as_of
// -----------------------------------------------------------------

#[test]
fn edges_as_of_returns_empty_for_unknown_subject() {
    let mut mem = new_mem();
    let got = mem
        .edges_as_of(Uuid::new_v4(), Utc::now())
        .expect("read ok");
    assert!(got.is_empty());
}

#[test]
fn edges_as_of_returns_inserted_edge_when_queried_at_or_after_recorded_at() {
    let mut mem = new_mem();
    let now = Utc::now();

    let alice = Entity::new("Alice".into(), EntityKind::Person, now);
    let acme = Entity::new("Acme".into(), EntityKind::Concept, now);
    let alice_id = alice.id;
    let acme_id = acme.id;

    let edge = Edge::new(
        alice_id,
        Predicate::Canonical(CanonicalPredicate::WorksAt),
        EdgeEnd::Entity { id: acme_id },
        Some(now),
        now,
    );

    {
        let mut g = mem.graph_mut();
        g.insert_entity(&alice).expect("insert alice");
        g.insert_entity(&acme).expect("insert acme");
        g.insert_edge(&edge).expect("insert edge");
    }

    // Query at +1s — the freshest (and only) row in the window must come back.
    let later = now + chrono::Duration::seconds(1);
    let got = mem.edges_as_of(alice_id, later).expect("read ok");
    assert_eq!(got.len(), 1, "expected one edge as of +1s, got {got:?}");
    assert_eq!(got[0].subject_id, alice_id);
}

// -----------------------------------------------------------------
// list_failed_episodes
// -----------------------------------------------------------------

#[test]
fn list_failed_episodes_is_empty_on_a_fresh_memory() {
    // No pipeline runs have been recorded yet → no failures to surface.
    // The deeper failure-bookkeeping semantics are covered by graph-store
    // unit tests; this just pins the Memory shim's empty-case contract.
    let mut mem = new_mem();
    let got = mem.list_failed_episodes().expect("read ok");
    assert!(
        got.is_empty(),
        "fresh Memory must have no failed episodes; got {got:?}"
    );
}

// -----------------------------------------------------------------
// graph_mut: re-borrowable across calls (no captured borrow on Memory)
// -----------------------------------------------------------------

#[test]
fn graph_mut_can_be_re_acquired_across_sequential_calls() {
    // Smoke test for the borrow shape: `graph_mut()` must release its
    // `&mut self` borrow on `Memory` between calls so that follow-up
    // Memory methods are reachable. If we ever accidentally returned a
    // type that captured the Memory borrow longer than its statement,
    // this test would stop compiling.
    let mut mem = new_mem();
    let now = Utc::now();

    let e1 = Entity::new("First".into(), EntityKind::Person, now);
    let id1 = e1.id;
    mem.graph_mut().insert_entity(&e1).expect("insert e1");

    // Re-borrow — would fail to compile if `graph_mut()` returned a
    // `Self`-borrowing handle that outlived the previous statement.
    let e2 = Entity::new("Second".into(), EntityKind::Person, now);
    let id2 = e2.id;
    mem.graph_mut().insert_entity(&e2).expect("insert e2");

    // And convenience reads work after writes.
    assert!(mem.get_entity(id1).expect("read ok").is_some());
    assert!(mem.get_entity(id2).expect("read ok").is_some());
}
