# ISS-090 — Subagent B Report (AC-10 + AC-11)

## AC-10 — EmpathyBus integration through new `store_raw` path: **PASS**

Test: `crates/engramai/tests/iss_090_empathy_bus_compat.rs` (written by Subagent A).

Verified the test asserts the three required behaviors:
1. `add_with_emotion(...)` returns `Ok(non_empty_id)` (the shim now delegates to `store_raw`).
2. The record is recallable via `mem.recall(...)`.
3. The Empathy Bus accumulator reflects the caller-supplied valence on the supplied domain (proves `process_interaction` fired and `emotional_trends` row exists with the right valence prior).

Run output:

```
$ cargo test -p engramai --test iss_090_empathy_bus_compat -- --nocapture
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.59s
     Running tests/iss_090_empathy_bus_compat.rs

running 1 test
test add_with_emotion_shim_fires_empathy_bus ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
```

No code or test changes required for AC-10 — the production path already works correctly through the shim.

## AC-11 — Rustdoc updates: **DONE**

### Files modified
- **`crates/engramai/src/memory.rs`** — added `# Recommended: use store_raw directly` block to the rustdoc for both:
  - `add_with_emotion` (around line 2615+)
  - `add_with_emotion_at` (around line 2670+)

  Each new block contains a `no_run` doctest demonstrating the canonical `store_raw` + `StorageMeta` pattern, with `emotion`/`domain` (and `occurred_at` for the `_at` variant) fields populated. Existing `# Arguments` lists and ISS-087/ISS-090 history notes were preserved.

### Doctest verification

```
$ cargo test -p engramai --doc -- memory::Memory::add_with_emotion
running 2 tests
test crates/engramai/src/memory.rs - memory::Memory::add_with_emotion (line 2642) - compile ... ok
test crates/engramai/src/memory.rs - memory::Memory::add_with_emotion_at (line 2692) - compile ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.08s
```

### Other rustdoc references to `add_with_emotion` (audited, not modified)

`grep -rn "add_with_emotion" --include="*.rs" | grep -E "///|//!"` returned 9 hits. All are descriptive (test-module headers, ISS-087/ISS-089 historical context, the "this method delegates to" sentence inside `add_with_emotion_at` itself, and `store_api.rs:114-115` which says `store_raw` *replaces* `add_with_emotion*` — i.e. correctly recommends the new path). None recommend the deprecated method as canonical, so no further updates were needed.

### Doctest API note (heads-up to main agent)

The example in the original prompt used `MemoryManager` + `async`/`await` + `..Default::default()` struct-update on `StorageMeta`. The actual current API is sync and the type is named `Memory` (not `MemoryManager`). I adapted the doctests to match the real API:

```rust
let mut meta = StorageMeta::default();
meta.emotion = Some(0.8);
meta.domain = Some("coding".into());
mem.store_raw("...", meta)?;
```

`StorageMeta` does derive `Default` and field-by-field assignment is what `add_with_emotion_at`'s own implementation does internally, so this is the idiomatic pattern. (Used `#![allow(clippy::field_reassign_with_default)]` is **not** needed in doctests because clippy doesn't run on them by default.)

## Build status

```
$ cargo build -p engramai
   Compiling engramai v0.2.4
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.40s
```

Clean. No new warnings introduced.

## Incidental findings / concerns

1. **`add_with_emotion` deprecation `since` is `0.2.3`** while `add_with_emotion_at` is `since = "0.3.0"`. That's intentional per the issue (the former was already deprecated; the latter is newly-deprecated by ISS-090) — not a defect, just flagging.
2. **Crate version is currently `0.2.4`** but the new deprecation says `since = "0.3.0"`. Cosmetic — the `since` field just informs `rustc`'s warning text; no functional issue. Worth bumping to `0.3.0` when this lands, but that's a release-engineering concern, not part of this issue.
3. The regression test does **not** exercise `add_with_emotion_at`'s shim path. Subagent A's test only covers `add_with_emotion`. AC-10 as written says "Empathy Bus integration works through new store_raw path" — singular — and the `_at` variant routes through the same `store_raw` call, so this is acceptable coverage. Optional follow-up: a parallel test for the `_at` variant would tighten the safety net but isn't required by the AC.

## Suggested next step

**Ready to mark ISS-090 done.** All 11 ACs are verified:
- AC-1..AC-9: previously completed (verified by inspection; not modified per scope)
- AC-10: regression test passes
- AC-11: rustdoc updated on both shims, doctests compile, no stale references elsewhere

Recommend the main agent close ISS-090 and consider a follow-up issue for the version bump to `0.3.0` if not already tracked.
