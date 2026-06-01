//! Regression test for ISS-059: orchestrator namespace propagation into
//! the Abstract plan.
//!
//! The bug: `crates/engramai/src/retrieval/orchestrator.rs` constructed
//! `AbstractPlanInputs` with `namespace: "default"` hardcoded at two
//! sites — the Hybrid sub-plan (lines 638-649, inside
//! `HybridDispatchExecutor::run`) and the direct `PlanKind::Abstract`
//! arm in `execute_plan` (lines 938-948). Even when callers said
//! `GraphQuery::with_namespace("alpha")`, the Abstract plan inside the
//! orchestrator looked at `default`, found no L5 topics, downgraded to
//! Associative, and returned
//! `EmptyResultSet { reason: "abstract_then_associative_empty" }`.
//!
//! The fix: read `query.namespace.as_deref().unwrap_or("default")` at
//! both sites so Abstract sees the same namespace the topic searcher
//! adapter is wired against in `Memory::graph_query`.
//!
//! What this test asserts (the regression-shaped invariant): with an
//! L5 topic planted in namespace `alpha`, an `Intent::Abstract` query
//! `.with_namespace("alpha")` does **not** end up at
//! `EmptyResultSet { reason: "abstract_then_associative_empty" }`. With
//! the bug present, the orchestrator would read namespace `default`,
//! find no topics, and produce exactly that empty-set outcome — so the
//! assertion fires precisely when ISS-059 regresses.
//!
//! Scope. This is the orchestrator-layer regression test (ISS-059
//! AC #4). It does **not** reproduce a full LoCoMo P@5 sweep — that's
//! ISS-059 AC #5 / NG-3 / NG-4, deferred per `design.md §7.2` (needs
//! the full ingest pipeline + dataset + minutes of runtime). The
//! orchestrator unit test below is sufficient to catch the namespace
//! regression: it exercises the exact code path that was buggy
//! (`AbstractPlanInputs::namespace`) end-to-end through
//! `Memory::graph_query`.

use chrono::Utc;
use rusqlite::Connection;
use tempfile::tempdir;
use uuid::Uuid;

use engramai::graph::store::{GraphWrite, SqliteGraphStore};
use engramai::graph::{init_graph_tables, Entity, EntityKind, KnowledgeTopic};
use engramai::memory::Memory;
use engramai::retrieval::api::{GraphQuery, RetrievalOutcome};
use engramai::retrieval::classifier::Intent;

/// Minimal blocking executor — same pattern used in
/// `iss056_retrieval_namespace_test.rs` and `retrieval::api::tests`.
/// `Memory::graph_query` is `async fn` but the body is synchronous, so
/// the future resolves on the first poll.
fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    let waker = Waker::from(Arc::new(NoopWaker));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

/// Plant a `KnowledgeTopic` row in the given namespace, mirroring the
/// `Topic` entity FK (production path:
/// `crates/engramai/src/synthesis.rs` and the test helper at
/// `retrieval/adapters/graph_topic_searcher.rs:206-237`).
///
/// Token-overlap fallback (no embedding) is enough for
/// `GraphTopicSearcher` to score this topic when the query mentions
/// its title terms — that's how this test gets a non-empty Abstract
/// result without wiring an embedding model.
fn plant_topic(graph_db: &std::path::Path, ns: &str, title: &str, summary: &str) -> Uuid {
    let mut conn = Connection::open(graph_db).expect("open graph db for planting topic");
    init_graph_tables(&conn).expect("init graph tables");
    let mut store = SqliteGraphStore::new(&mut conn).with_namespace(ns);

    let topic_id = Uuid::new_v4();

    // graph_topics.id has an FK on graph_entities(id); production
    // synthesis writes a Topic entity row before upserting the topic.
    let mut e = Entity::new_random_id(title.to_string(), EntityKind::Topic, Utc::now());
    e.id = topic_id;
    store.insert_entity(&e).expect("insert mirror Topic entity");

    let topic = KnowledgeTopic {
        topic_id,
        title: title.to_string(),
        summary: summary.to_string(),
        embedding: None,
        source_memories: Vec::new(),
        contributing_entities: Vec::new(),
        cluster_weights: None,
        synthesis_run_id: None,
        synthesized_at: Utc::now().timestamp() as f64,
        superseded_by: None,
        superseded_at: None,
        namespace: ns.to_string(),
    };
    store.upsert_topic(&topic).expect("upsert topic");
    topic_id
}

/// **ISS-059 regression.** Abstract query with `.with_namespace("alpha")`
/// reaches the topic in `alpha` (orchestrator threads the query's
/// namespace into `AbstractPlanInputs`).
///
/// Pre-fix behavior: orchestrator hardcoded `namespace = "default"`
/// inside `AbstractPlanInputs`, so even though the query said `alpha`
/// the Abstract plan looked at `default`, found no topics, downgraded
/// to Associative, hit an empty graph, and returned
/// `EmptyResultSet { reason: "abstract_then_associative_empty" }`.
#[test]
fn iss059_abstract_query_with_namespace_finds_topic() {
    let tmp = tempdir().expect("tempdir");
    let db_path = tmp.path().join("iss059-mem.db");
    let graph_db = tmp.path().join("iss059-graph.db");

    // Plant an L5 topic in namespace `alpha`. `default` stays empty —
    // that's the bug-trap: with the regression, orchestrator reads
    // `default`, sees nothing, downgrades.
    let _topic_id = plant_topic(
        &graph_db,
        "alpha",
        "Architecture overview of the engram system",
        "engram is a memory crate for AI agents with namespace-scoped storage.",
    );

    // Wire `Memory` to the same graph file. `with_graph_store` opens
    // its own connection; the planted row is persistent on disk so the
    // orchestrator's adapter sees it.
    let mem = Memory::new(db_path.to_str().unwrap(), None)
        .expect("memory init")
        .with_graph_store(&graph_db)
        .expect("install graph store");

    let q = GraphQuery::new("explain the engram architecture overview")
        .with_namespace("alpha")
        .with_intent(Intent::Abstract);

    let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");

    // Plan dispatch is unaffected by the bug — both pre- and post-fix
    // dispatch to Abstract. We assert it for documentation, not to
    // catch the regression.
    assert_eq!(resp.plan_used, Intent::Abstract);

    // The regression-shaped assertion: the buggy code path produces
    // exactly `EmptyResultSet { reason: "abstract_then_associative_empty" }`
    // for this fixture (no topic in `default` → DowngradedL5Unavailable
    // → Associative on empty graph → empty). The fix prevents that.
    if let RetrievalOutcome::EmptyResultSet { reason } = &resp.outcome {
        assert_ne!(
            reason, "abstract_then_associative_empty",
            "ISS-059 regressed: orchestrator ignored query.namespace=\"alpha\" \
             and read namespace=\"default\" for Abstract; got \
             EmptyResultSet {{ reason: \"abstract_then_associative_empty\" }} \
             despite a planted topic in `alpha`. \
             Check `crates/engramai/src/retrieval/orchestrator.rs` \
             AbstractPlanInputs construction at lines ~640 and ~941: \
             namespace must read `query.namespace.as_deref().unwrap_or(\"default\")`."
        );
    }

    // Stronger positive assertion: with the fix, the topic in `alpha`
    // is reachable, so the outcome must be `Ok` with at least one
    // candidate. (If a future change to the topic-searcher scoring
    // breaks this, the failure message above still identifies ISS-059
    // as one possible cause.)
    assert!(
        matches!(resp.outcome, RetrievalOutcome::Ok),
        "Abstract over namespace `alpha` with a token-overlapping topic \
         should produce RetrievalOutcome::Ok; got {:?}",
        resp.outcome
    );
    assert!(
        !resp.results.is_empty(),
        "Abstract over namespace `alpha` should return at least one \
         scored result for the planted topic; got empty results"
    );
}

/// Backward-compat: a query without `.with_namespace()` still reads
/// `default`. The fix uses `unwrap_or("default")`, so this contract is
/// preserved. Together with the test above, this pins down the
/// fall-through invariant from `design.md §3 I-1` and §3 I-2.
#[test]
fn iss059_abstract_query_without_namespace_uses_default() {
    let tmp = tempdir().expect("tempdir");
    let db_path = tmp.path().join("iss059-default-mem.db");
    let graph_db = tmp.path().join("iss059-default-graph.db");

    // Plant the topic in `default` this time — query omits
    // `.with_namespace()`.
    let _topic_id = plant_topic(
        &graph_db,
        "default",
        "Architecture overview of the engram system",
        "engram is a memory crate for AI agents.",
    );

    let mem = Memory::new(db_path.to_str().unwrap(), None)
        .expect("memory init")
        .with_graph_store(&graph_db)
        .expect("install graph store");

    let q =
        GraphQuery::new("explain the engram architecture overview").with_intent(Intent::Abstract);
    // Note: no .with_namespace().

    let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");
    assert_eq!(resp.plan_used, Intent::Abstract);
    assert!(
        matches!(resp.outcome, RetrievalOutcome::Ok),
        "Abstract without explicit namespace should fall through to \
         `default` and find the planted topic; got {:?}",
        resp.outcome
    );
    assert!(
        !resp.results.is_empty(),
        "expected at least one scored result"
    );
}
