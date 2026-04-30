---
id: ISS-072-design-r1
parent: ISS-072-design
kind: review
date: 2026-04-30
reviewer: rustclaw
status: applied
---

# Design Review — ISS-072 GOAL-2 (A-clean kind plumbing)

5 findings: 1 critical / 2 medium / 2 minor. **All applied 2026-04-30.**

## 🔴 FINDING-1 (critical) — `merge_into_canonical` lacks precedence enforcement

**Issue.** Design locks precedence `EnrichmentLLM > DictionaryMatch > TripleHint > Default` but §1–§7 wire-level changes never touch `merge_into_canonical` (`stage_persist.rs:453`). It writes neither `kind` nor `attributes["kind_source"]` on the canonical row — incoming draft fields are dropped on merge. Today this is benign (dictionary path empty in production, triple path is the only writer and always loses to itself). The moment a second source ships, canonical entities freeze at whatever the first triple emitted.

**Fix applied.** §6 now flags this explicitly with a "this is the ONLY place §1–§7 writes provenance, the merge path does not" callout. Added new **§8 "Merge-time precedence enforcement (deferred to GOAL-2.b)"** that:
- Names the gap and where it lives.
- Defines the contract the GOAL-2.b PR must satisfy (`should_overwrite_kind` + `rank` pseudocode).
- Locks the rule that `kind` and `kind_source` writes move together (no split writes).
- States what A-clean explicitly does NOT do, what it DOES guarantee for B.
- Specifies backfill behavior (missing key defaults to `Default` — new signal beats absence).

## 🟡 FINDING-2 (medium) — `RawTriple` file path was wrong

**Issue.** §1 said `triple.rs — RawTriple schema` but `RawTriple` is a parser-local struct in `triple_extractor.rs:~94`. `triple.rs` has no `RawTriple`.

**Fix applied.** §1 retitled to `triple_extractor.rs — RawTriple (parser-local) schema` with explicit note that earlier drafts mislabeled the file.

## 🟡 FINDING-3 (medium) — `Triple` propagation path missing

**Issue.** §5 used `triple.subject_kind` / `triple.object_kind`, but `Triple` (in `triple.rs`) doesn't have those fields and the design didn't say to add them. As written, §5 wouldn't compile.

**Fix applied.** Added new **§1b "`triple.rs` — `Triple` struct kind hint propagation"**:
- Adds `pub subject_kind_hint: Option<EntityKind>` and `pub object_kind_hint: Option<EntityKind>` to `Triple`.
- `#[serde(default, skip_serializing_if = "Option::is_none")]` keeps wire/storage byte-identical when absent — no migration.
- Construction logic in `triple_extractor.rs` (parse hint via `parse_kind_hint` before building `Triple`).
- Naming convention: `_kind_hint` suffix to make "unvalidated guess" obvious at every call site.
- §5 rewritten to read the propagated `triple.subject_kind_hint` / `triple.object_kind_hint` directly (no second `parse_kind_hint` call).

## 🟢 FINDING-4 (minor) — `EntityKind` allowlist had wrong variant names

**Issue.** §4 allowlist had `Location` (real variant is `Place`) and was missing `Topic` entirely.

**Fix applied.**
- §4 `parse_kind_hint`: `"Location"` → `"Place"`; added `"Topic" => Some(EntityKind::Topic)` arm.
- §2 prompt: allowlist updated to `Person, Organization, Place, Concept, Artifact, Event, Topic`. Added explicit note that out-of-allowlist kinds are dropped, NOT routed through `EntityKind::Other` (Other only via the dedicated constructor when adding a canonical variant isn't yet warranted).

## 🟢 FINDING-5 (minor) — `KindSource` Debug-string serialization is hidden schema lock

**Issue.** §6 used `format!("{:?}", draft.kind_source)` — `Debug` output is not a stable contract, persisting it locks the schema implicitly.

**Fix applied.**
- `KindSource` enum: added `#[derive(Serialize, Deserialize)]` and `#[serde(rename_all = "PascalCase")]`. On-disk strings are now `"Default" | "DictionaryMatch" | "TripleHint" | "EnrichmentLLM"` — explicit serde contract.
- §6 persistence: replaced `format!("{:?}", ...)` with `serde_json::to_value(draft.kind_source).expect("KindSource serialize is infallible")`.
- §`Persistence` section rewritten to call out why this matters (Debug not guaranteed stable across compiler versions or refactors; serde makes the wire format an explicit contract).
- §8 (new) reads `attributes["kind_source"]` via `serde_json::from_value::<KindSource>(...)` for the merge precedence check — round-trip via the same explicit contract.

## Summary

All 5 findings applied as a single atomic edit (9 surgical edits to design.md). Section numbering: §1, §1b (new), §2, §3, §4, §5, §6, §8 (new), §7. §7 follows §8 textually for content cohesion (§8 belongs with §6's persistence story; §7 is unrelated caller cleanup). Cross-references all resolve.

The design is now implementation-ready. The §8 contract makes the GOAL-2.a / GOAL-2.b split clean — A-clean ships the field, B PR adds one `match` arm to `merge_into_canonical` plus tests.
