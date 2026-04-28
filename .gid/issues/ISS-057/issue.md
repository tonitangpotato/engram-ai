# ISS-057: Promote `namespace` to a first-class `Namespace` newtype to prevent the bug class entirely

- **Status**: open (P2 — do only if/when the predicted bug class materializes)
- **Severity**: enhancement (no current production bug; preventive type-system hardening)
- **Filed**: 2026-04-28
- **Trigger condition**: file/start work on this issue ONLY if a runtime bug occurs that fits one of:
  1. A new retrieval/storage/ingest API is added and silently uses wrong/empty namespace because the author forgot to plumb it.
  2. A function-argument-ordering bug between two `&str` parameters causes namespace confusion.
  3. Someone accidentally calls `.to_string()` / `&str` conversion that defaults to `""` or `"default"` and data lands in the wrong namespace.
- **Related**: ISS-056 (the actual root fix for the current LoCoMo blocker — single-point hole at retrieval API). ISS-055, ISS-050, ISS-048, ISS-046 (historical namespace bugs, all already fixed via point fixes).

---

## Summary

`namespace` is currently passed around the engramai codebase as `&str`,
`Option<&str>`, `String`, or `Option<String>` across ~30 modules and
~100+ function signatures. The type system doesn't distinguish a
`namespace` argument from any other string — making it possible (in
principle) to:

- Forget to thread namespace through a new code path → data lands in
  the wrong namespace.
- Swap the order of two `&str` arguments → namespace confused with
  another field.
- Accidentally fall through to a `Default` (`""` or `"default"`) →
  data leaks across tenants.

A `Namespace` newtype with no `Default`, no `From<&str>`, and a
fallible constructor would make the entire bug class **a compile
error**.

---

## Why This is Filed but Not Scheduled

**No current evidence the bug class is real.** All 4 historical
namespace bugs (ISS-046, ISS-048, ISS-055, ISS-056) had distinct
non-newtype root causes:

- ISS-046: CLI didn't wire pipeline pool at all → graph empty (orthogonal to namespace typing).
- ISS-048: graph store hardcoded "default" in one specific wiring point → manually fixed.
- ISS-055: `PipelineConfig::default()` → `namespace = ""` because Default was wrong, not because the type was a `&str`.
- ISS-056: `GraphQuery` had no namespace field at all → caller couldn't even *say* which namespace.

**None of these would have been caught by a `Namespace` newtype.** They
are all "caller didn't pass namespace through this specific channel"
bugs, which are about API design, not about type confusion.

→ Filing this issue is **defensive documentation**: capture the design
so we don't reinvent it next time. Do NOT pre-emptively schedule the
3-day refactor.

---

## Proposed Design (when triggered)

### 1. `Namespace` newtype

```rust
// crates/engramai/src/namespace.rs

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Namespace(String);

impl Namespace {
    /// Construct from a string. Empty / reserved namespaces rejected.
    pub fn new(s: impl Into<String>) -> Result<Self, NamespaceError> { ... }

    /// System-internal namespace (schema metadata, etc.). Must be
    /// called explicitly — there is no fallthrough default.
    pub fn system() -> Self { Self("__system__".into()) }

    pub fn as_str(&self) -> &str { &self.0 }
}

// Deliberately NOT implemented:
// - Default               (no implicit empty/default namespace)
// - From<&str> / From<String>  (force explicit ::new() with validation)
// - Display               (avoid accidental .to_string() into SQL/log fields)
```

### 2. Replace `&str` / `Option<&str>` signatures

- All public APIs that take namespace → `&Namespace`.
- All struct fields → `Namespace` (not `Option<String>`).
- "Cross-namespace" or "all namespaces" queries → separate API:
  ```rust
  pub enum NamespaceScope {
      Single(Namespace),
      Set(Vec<Namespace>),
      All,  // admin only
  }
  ```

### 3. Builder enforcement

`MemoryBuilder`, `PipelineConfigBuilder`, `GraphQuery::new`, etc. require
`Namespace` at build time — `build()` returns `Err(MissingNamespace)`
otherwise.

### 4. Storage-layer invariant assertion

DB write hooks check non-empty namespace; integration test scans tables
for `''` / `NULL` namespace rows and panics if any found (catches
manual SQL or migration regressions).

---

## Migration Plan (if executed)

- **Phase 1** (~1 day): Introduce `Namespace` type alongside existing
  `&str`. Add deprecated `&str` shims that delegate to `Namespace::new`.
- **Phase 2** (~1 day): Migrate internal callsites; chase deprecation
  warnings via `cargo check`.
- **Phase 3** (~half day): Remove `&str` / `Option<&str>` shims. Now
  the compiler enforces namespace propagation.
- **Phase 4** (~half day): Add storage-layer invariant test.

**Total**: ~3 days focused work.

---

## Acceptance (if executed)

1. `cargo check` clean — no `&str` namespace signatures remain in
   public API.
2. Removing namespace from any callsite is a compile error.
3. New storage invariant test passes — no orphan `''` / `NULL`
   namespace rows in any namespaced table.
4. Full `cargo test --workspace` green.
5. Performance regression test: namespace newtype overhead < 1% (it's
   a `String` wrapper, should be free).

---

## Decision Log

- **2026-04-28**: Filed after a debug session where the initial proposal
  was to do this newtype refactor as the "root fix" for ISS-056.
  Rescoped after audit revealed ISS-056 is a single-point hole, not a
  type-system gap. potato pushed back: "怎么又要重构🤯" — correct call.
  Logged as preventive design; revisit only if/when the predicted bug
  class actually fires.
