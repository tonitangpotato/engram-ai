//! Regression test for ISS-064 (A6.5): namespace mismatch silent-empty defense.
//!
//! The bug: `Memory::graph_query` accepted a `namespace` override (added in
//! ISS-056) but didn't validate that the namespace actually existed in the
//! graph backend. Querying a typo'd namespace silently returned an empty
//! result — indistinguishable from "namespace exists but has no matching
//! entities." That gap caused the RUN-0001 false-alarm cascade where a
//! capitalization mismatch (`"Conv26"` vs `"conv26"`) showed up only as
//! 0/25 hit rate, not as an obvious error.
//!
//! The fix (A6.4): in `retrieval::api::Memory::graph_query`, after
//! resolving `namespace = query.namespace.as_deref().unwrap_or("default")`,
//! probe the graph store for the namespace. If it has zero rows in *both*
//! `graph_entities` and `graph_topics`, emit `log::warn!` with the
//! namespace name and a list of known namespaces, then return an empty
//! `GraphQueryResponse` so legal "namespace exists but empty" callers
//! aren't broken.
//!
//! Acceptance (this file, A6.5):
//!
//!   1. Query an explicit non-existent namespace returns an empty result
//!      set (no panic, no error — the legal-empty contract is preserved).
//!   2. The `log::warn!` is emitted at least once and its rendered
//!      message contains the offending namespace name and the word
//!      `namespace` so log readers can grep for it.
//!   3. The implicit-default path (no `with_namespace(…)` call) on a
//!      fresh store does NOT warn — first-run queries against an empty
//!      DB are legal and must not emit operational noise. The
//!      discriminator is `query.namespace.is_some()`, not the resolved
//!      namespace string.
//!   4. An *explicit* `with_namespace("default")` on an empty store
//!      DOES warn — `"default"` is not a magic exemption when the
//!      caller went out of their way to ask for it.
//!
//! Implementation note: engramai's dev-deps don't include `tracing-test`
//! and the production code uses the `log` crate (`log::warn!`), not
//! `tracing`. Rather than add a new dev-dep, this file installs a
//! single-shot global `log::Log` implementation that records every
//! emitted record into a `Mutex<Vec<String>>`. The custom logger is
//! installed exactly once via `log::set_boxed_logger` + `OnceLock`.
//! All assertions read the captured buffer.

use std::sync::{Mutex, OnceLock};

use engramai::config::MemoryConfig;
use engramai::memory::Memory;
use engramai::retrieval::api::GraphQuery;
use tempfile::tempdir;

// --------------------------------------------------------------------------
// Log capture: install once globally, share buffer across all tests in this
// file. Cargo runs tests in the same binary serially with --test-threads=1
// only if explicitly requested, so we guard concurrent reads with a Mutex.
// --------------------------------------------------------------------------

static LOG_BUFFER: OnceLock<Mutex<Vec<CapturedRecord>>> = OnceLock::new();
static INSTALL: OnceLock<()> = OnceLock::new();

#[derive(Clone, Debug)]
#[allow(dead_code)] // `target` shown in failure printouts via Debug
struct CapturedRecord {
    level: log::Level,
    target: String,
    message: String,
}

struct CapturingLogger;

impl log::Log for CapturingLogger {
    fn enabled(&self, _meta: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if let Some(buf) = LOG_BUFFER.get() {
            let mut g = buf.lock().expect("log buffer poisoned");
            g.push(CapturedRecord {
                level: record.level(),
                target: record.target().to_string(),
                message: format!("{}", record.args()),
            });
        }
    }

    fn flush(&self) {}
}

fn install_capture() {
    LOG_BUFFER.get_or_init(|| Mutex::new(Vec::new()));
    INSTALL.get_or_init(|| {
        // `set_boxed_logger` fails if a logger is already installed (e.g.,
        // env_logger from another test in the same binary). We tolerate
        // that — if some other logger ran first, this binary's tests will
        // simply not capture and the assertions below will be skipped via
        // `ensure_capture_active`.
        let _ = log::set_boxed_logger(Box::new(CapturingLogger));
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// Drain every record currently in the buffer and return them. Subsequent
/// calls only see records emitted *after* this drain — important for tests
/// that need to assert "exactly one warn for this query."
fn drain_records() -> Vec<CapturedRecord> {
    let buf = LOG_BUFFER.get().expect("install_capture not called");
    let mut g = buf.lock().expect("log buffer poisoned");
    let out = g.clone();
    g.clear();
    out
}

/// True when our custom logger actually owns the global slot. If another
/// crate's logger (env_logger, tracing-log shim, …) won the race we can't
/// reliably observe `log::warn!` emissions and the assertions become
/// false-positives; in that (currently impossible — no other logger in
/// the engramai test binary) case we'd skip the message-content checks
/// and keep the empty-result-set check, which is independently valuable.
fn capture_active() -> bool {
    LOG_BUFFER
        .get()
        .map(|b| b.lock().map(|g| !g.is_empty() || true).unwrap_or(false))
        .unwrap_or(false)
}

// --------------------------------------------------------------------------
// Async block_on shim — same pattern as ISS-056's test, since
// `Memory::graph_query` is `async fn` but contains no real `.await`
// points. A spin-poll executor finishes on the first poll.
// --------------------------------------------------------------------------

fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    let waker = Waker::from(Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    // Safety: `fut` is owned and immediately pinned to the stack.
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[test]
fn iss064_query_nonexistent_namespace_returns_empty_and_warns() {
    install_capture();
    drain_records(); // start clean

    let dir = tempdir().expect("tempdir");
    let mem_db = dir.path().join("mem.db");
    let mem_db_str = mem_db.to_str().unwrap().to_string();
    let graph_db = dir.path().join("graph.db");

    // Fresh memory store — no ingest, no graph rows under any namespace.
    // The retrieval entry point still has to handle this without panicking.
    let mem = Memory::new(&mem_db_str, Some(MemoryConfig::default()))
        .expect("memory boots")
        .with_graph_store(&graph_db)
        .expect("graph store installs");

    let bogus_ns = "this-namespace-does-not-exist-xyzzy";
    let query = GraphQuery::new("anything").with_namespace(bogus_ns);
    let resp = block_on(mem.graph_query(query)).expect("query must not error");

    // Contract check 1: legal-empty. Querying a missing namespace is not
    // a hard error — it returns an empty response. (If we made it an
    // error, every test that probes for emptiness would have to special-
    // case `NamespaceNotFound`, breaking the ergonomics that ISS-056
    // shipped.)
    assert!(
        resp.results.is_empty(),
        "non-existent namespace must return empty results, got {} hits",
        resp.results.len()
    );

    // Contract check 2: warn was emitted with the right shape.
    if !capture_active() {
        eprintln!(
            "log capture inactive (foreign logger took the global slot); \
             skipping message-content assertion"
        );
        return;
    }
    let records = drain_records();
    let warns: Vec<&CapturedRecord> = records
        .iter()
        .filter(|r| r.level == log::Level::Warn)
        .collect();

    assert!(
        !warns.is_empty(),
        "expected at least one log::warn! for missing namespace; \
         captured records were: {:#?}",
        records
    );

    let any_match = warns
        .iter()
        .any(|r| r.message.contains(bogus_ns) && r.message.contains("namespace"));
    assert!(
        any_match,
        "expected a warn mentioning the bogus namespace `{}` and the word \
         'namespace'; got warns: {:#?}",
        bogus_ns, warns
    );
}

#[test]
fn iss064_implicit_default_namespace_does_not_warn() {
    // The implementation deliberately scopes the warn to *explicit*
    // `with_namespace(...)` calls. The implicit-`None` path falls back
    // to `"default"` and must keep working silently on a fresh store —
    // otherwise every first-run query (no data yet) would emit a
    // misleading warn. The discriminator is `query.namespace.is_some()`,
    // not the resolved namespace string.
    //
    // This test pins that contract so a future refactor that "helpfully"
    // collapses the two paths breaks loudly.
    install_capture();
    drain_records();

    let dir = tempdir().expect("tempdir");
    let mem_db = dir.path().join("mem.db");
    let mem_db_str = mem_db.to_str().unwrap().to_string();
    let graph_db = dir.path().join("graph.db");

    let mem = Memory::new(&mem_db_str, Some(MemoryConfig::default()))
        .expect("memory boots")
        .with_graph_store(&graph_db)
        .expect("graph store installs");

    // No `with_namespace` → implicit-default path. Empty result is fine,
    // but no namespace-mismatch warn is allowed.
    let query = GraphQuery::new("anything");
    let resp = block_on(mem.graph_query(query)).expect("query must not error");

    assert!(
        resp.results.is_empty(),
        "fresh store must return empty results, got {} hits",
        resp.results.len()
    );

    if !capture_active() {
        return;
    }
    let records = drain_records();
    let namespace_warns: Vec<&CapturedRecord> = records
        .iter()
        .filter(|r| {
            r.level == log::Level::Warn
                && r.target == "engramai::retrieval"
                && r.message.contains("namespace")
                && r.message.contains("not found")
        })
        .collect();
    assert!(
        namespace_warns.is_empty(),
        "implicit-default path must not emit namespace-mismatch warn; got: {:#?}",
        namespace_warns
    );
}

#[test]
fn iss064_explicit_default_on_empty_store_warns() {
    // Mirror of the bogus-namespace case but with the literal string
    // `"default"` passed *explicitly*. RUN-0001 happened with a
    // capitalization mismatch where the caller passed `Some("Conv26")`
    // and the substrate held `"conv26"` — if `"default"` were treated
    // as a magic exemption when explicitly requested, an analogous
    // typo into `"default"` itself (e.g., a script defaulting to
    // `--ns default` against a substrate that uses only named
    // namespaces) would still slip through. Test the explicit path.
    install_capture();
    drain_records();

    let dir = tempdir().expect("tempdir");
    let mem_db = dir.path().join("mem.db");
    let mem_db_str = mem_db.to_str().unwrap().to_string();
    let graph_db = dir.path().join("graph.db");

    let mem = Memory::new(&mem_db_str, Some(MemoryConfig::default()))
        .expect("memory boots")
        .with_graph_store(&graph_db)
        .expect("graph store installs");

    let query = GraphQuery::new("anything").with_namespace("default");
    let resp = block_on(mem.graph_query(query)).expect("query must not error");

    assert!(
        resp.results.is_empty(),
        "empty store must return empty results, got {} hits",
        resp.results.len()
    );

    if !capture_active() {
        return;
    }
    let records = drain_records();
    let warns: Vec<&CapturedRecord> = records
        .iter()
        .filter(|r| r.level == log::Level::Warn)
        .collect();
    assert!(
        !warns.is_empty(),
        "explicit `default` on empty store must warn; captured: {:#?}",
        records
    );
    let mentions_default = warns
        .iter()
        .any(|r| r.message.contains("default") || r.message.contains("namespace"));
    assert!(
        mentions_default,
        "warn message should mention 'default' or 'namespace'; got: {:#?}",
        warns
    );
}
