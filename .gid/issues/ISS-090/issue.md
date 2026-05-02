---
id: ISS-090
title: Retire add_with_emotion / add_with_emotion_at — convert to store_raw shims, complete ISS-019 Step 4.5
status: in_progress
priority: P2
severity: medium
tags:
- api-cleanup
- deprecation
- store-raw
- iss-019-followup
- emotion
related:
- ISS-089
- ISS-087
- ISS-019
---

# ISS-090: Retire `add_with_emotion` / `add_with_emotion_at`

## Summary

`add_with_emotion` (deprecated since 0.2.3) and `add_with_emotion_at` (added during ISS-087, never deprecated) both call `add_raw` — the v0.2-compat path that bypasses extractor dispatch, graph emit, and resolution-queue enqueue. As long as these two methods exist in their current form, callers using them silently produce v0.2-shaped substrate.

ISS-019 Step 4.5 set the invariant: "all writes flow through `store_raw`". These two methods are the last documented violation of that invariant. This issue retires them properly.

## Background — why these exist

- `add_with_emotion` predates ISS-019. Originally, it was the canonical "store with affect tag" entry point; emotion was attached at record level because there was no fact concept yet.
- `add_with_emotion_at` was added in ISS-087's implementation as a CLI-side workaround when the implementer didn't want to thread `occurred_at` through `store_raw`. It's a sibling of the same shape, just with one extra `Option<DateTime<Utc>>` parameter.

After ISS-089 lands, the CLI no longer calls `add_with_emotion_at` for the `--occurred-at` path (it calls `store_raw`). The CLI still calls `add_with_emotion` for the `--emotion + --domain` path. So:

- `add_with_emotion_at`: zero internal callers after ISS-089. Pure dead-weight public API.
- `add_with_emotion`: one internal caller (CLI `--emotion + --domain` branch). Still routes through `add_raw`.

## Acceptance Criteria

- [ ] **AC-1**: `StorageMeta` gains `emotion: Option<f64>` and `domain: Option<String>` fields. Default `None`. Serialized to `user_metadata` if set (round-trip preserved).
- [ ] **AC-2**: `store_raw` Path A (extractor present) applies caller emotion to extracted facts using a **fallback rule**: `final_valence = fact.valence.or(meta.emotion)`. Extractor's per-fact judgment wins; caller emotion is the prior used only when extractor produced no valence for that fact. Document this behavior in `store_raw`'s rustdoc.
- [ ] **AC-3**: `store_raw` Path A also applies caller `domain` to facts that have no extractor-assigned domain (same fallback discipline).
- [ ] **AC-4**: `store_raw` Path B (no extractor) uses caller emotion/domain directly on the single admitted record (no fact split — no aggregation question).
- [ ] **AC-5**: CLI `Commands::Store`: collapse the `(emotion, domain).is_some()` branch. ALL invocations route through `mem.store_raw(content, StorageMeta { occurred_at, emotion, domain, … })`. Single call site.
- [ ] **AC-6**: `add_with_emotion` becomes a `#[deprecated]` shim that calls `store_raw` (NOT `add_raw`). Signature preserved for any external caller. Internally goes through Path A.
- [ ] **AC-7**: `add_with_emotion_at` becomes a `#[deprecated]` shim that calls `store_raw` (same treatment). Note ISS-090 in the deprecation message.
- [ ] **AC-8**: `add_raw` is unchanged (still has its own deprecation note from ISS-019). Out of scope here.
- [ ] **AC-9**: Test `tests/iss_090_emotion_through_store_raw.rs`:
  - Store via `add_with_emotion(content, …, emotion=-0.5, domain="trading")` → assert graph node exists (proof Path A ran)
  - Store via CLI `engram store --emotion -0.5 --domain trading "…"` → assert graph node exists
  - Store text where extractor produces a fact with explicit valence → assert caller emotion does NOT override (fallback rule honored)
  - Store text where extractor produces facts with no valence → assert caller emotion is applied
- [ ] **AC-10**: `EmpathyBus::process_interaction` integration verified — domain trend accumulation still works through new path. (Likely just works because emotion lands on each fact and EmpathyBus reads facts. But add a regression test.)
- [ ] **AC-11**: All `add_with_emotion*` rustdoc examples in the codebase updated to show the `store_raw` path as the recommended way (shim docs say "prefer `store_raw` directly").

## Out of Scope

- **Removing the methods entirely.** They become shims, not deletions. Full removal waits for v0.4 per existing deprecation policy. (A future ISS-09x can do removal once `git grep add_with_emotion` returns zero hits in dependent crates.)
- **`add_raw` deprecation/removal.** Separate ISS-019 follow-up.
- **CLI `--emotion` / `--domain` flag UX changes.** Flags stay, semantics stay (caller-provided affect prior). Only the routing under the hood changes.
- **EmpathyBus redesign.** Out of scope. Current bus reads from records/facts as-is; AC-10 just verifies it keeps working.
- **`occurred_at` plumbing.** ISS-089's job. ISS-090 assumes ISS-089 has landed (or lands together).

## Design notes — fallback rule rationale

Why `fact.valence.or(meta.emotion)` and not override or aggregate?

1. **Extractor wins on per-fact judgment.** If extractor says fact F1 is happy and F2 is sad, caller blanket-tagging both as "negative" destroys signal. The extractor sees finer detail.
2. **Caller emotion is a useful prior.** When extractor returns no valence (neutral or undetermined), the caller's intent ("I'm logging this with sadness in mind") is the best available signal.
3. **No aggregation needed.** Each fact carries its own valence; EmpathyBus accumulates per-fact. No averaging or weighting decision required.
4. **Compatible with v0.2 `add_with_emotion` semantics.** In v0.2 there were no facts — caller emotion was the only valence on the single record. After this change, the single-record case (Path B, no extractor) preserves that exact behavior. The multi-fact case (Path A) is a strict refinement, not a regression.

## Why P2 (not P1)

ISS-089 fixes the immediate RUN-0010 blocker — once it lands, the temporal eval pipeline works. ISS-090 is hygiene: it shrinks API surface and removes the foot-gun where future callers using `add_with_emotion` get v0.2-shaped substrate. Important but not blocking.

## Lessons captured (cross-ref)

ISS-089's lessons section already covers the meta-pattern: "during ISS-087 implementation, the path of least resistance copy-pasted an existing branch instead of doing the harder root fix." ISS-090 is the cleanup pass that closes that loophole. After both land, the CLI has exactly one store path and `MemoryManager` has exactly one canonical write API.
