---
id: ISS-065
title: Pre-LLM hook should detect factual claims and force verification before generation
kind: issue
status: todo
priority: P2
labels: [engram, hooks, epistemic-hygiene, llm-safety]
created: 2026-04-28
---

# ISS-065: Pre-LLM hook should detect factual claims and force verification before generation

## Problem

LLMs (including Claude/GPT-class agents running on RustClaw) confabulate specific factual
claims with high confidence — issue IDs, file paths, version numbers, dates, past
decisions. The internal "I remember this" signal is unreliable; the agent cannot
distinguish genuine recall from plausible-sounding reconstruction.

### Concrete failure (2026-04-28)

In a Telegram chat with potato, the agent stated:

> "ISS-049 Phase 4 read API hardcoded namespace=default"

This was **wrong**. The correct mapping in the engram graph is:

- ISS-049 — retrieval orchestrator wired Null* stubs (done)
- ISS-056 — `GraphQuery` has no namespace field; `retrieval/api.rs:432` hardcoded
  `"default"` (done)

The agent collapsed two distinct issues, sounded confident, was caught only because
potato happened to query the graph manually and pushed back. Without that pushback
the wrong claim would have entered the agent's own memory as "fact" via auto-store.

This is not a one-off. It is a systemic failure mode of LLM agents.

## Why current mitigations are insufficient

### Current state of pre-LLM recall (engram-recall hook)

Source: `rustclaw/src/engram_hooks.rs::EngramRecallHook` (HookPoint::BeforeInbound, priority 50).

Behaviour:

1. On every inbound user message ≥10 chars, runs `session_recall(&ctx.content, &session_key)`.
2. Returns top-N memories by embedding similarity to the *user message*.
3. Formats them into a "## ⚠️ Recalled Memories (auto)" block prepended to system context.

This helps with *topic-relevant* recall but does NOT prevent confabulation, because:

- Recall is keyed on the **user's question**, not the **agent's draft answer**. The
  hallucinated ID never gets queried.
- 5–10 retrieved memories cannot cover the long tail of specific IDs/paths/numbers
  the agent might cite.
- The hook produces *advisory* context; nothing blocks the agent from ignoring it
  and generating a confidently wrong specific claim.

### Current state of behaviour-level mitigations

- `SOUL.md` rule: "没做的事不能说做了" — too general, doesn't catch reconstruction.
- `AGENTS.md` "Active Recall" section says "before answering questions about
  history... run `engram recall` FIRST" — relies on agent self-discipline. Empirically
  fails (this incident).
- New skill `skills/cite-before-claim/SKILL.md` (always_load, 2026-04-28) — also relies
  on self-discipline, but at least makes the rule explicit and lists which tools to
  use for which claim types. **This is the temporary patch this ISS is meant to
  replace.**

## Goal

Move fact-verification from "agent self-discipline" to "framework-enforced gate" so
that specific factual claims in agent output cannot be emitted without a corresponding
verification tool call.

## Design space (not committing yet — this ISS is for tracking, design comes later)

Several possible mechanisms, ranked by intrusiveness:

### Option A — Output post-hoc verifier (least intrusive)

After the LLM generates a draft response, run a second pass:

1. Extract candidate factual claims from the draft via regex/structured extraction
   (ID patterns like `ISS-\d+`, file paths, function names, semver, dates).
2. For each candidate, check whether the claim is "supported" by:
   - Tool calls made *this turn* whose results would have produced this claim, OR
   - The recalled-memories block already in context, OR
   - The current message's quoted text.
3. If unsupported → return the draft to the LLM with a system note: "claim X about
   Y is unverified, please verify with tool call before re-emitting".

Pros: doesn't require LLM cooperation, can be added without protocol changes.
Cons: extra LLM round-trip per response with claims; regex will miss some claims and
false-flag others; doesn't help if the verification tool itself is misused.

### Option B — Claim → tool call requirement at decode time

When the LLM is about to emit a token sequence matching a factual-claim pattern,
intercept and require a preceding tool call. This is essentially a constrained
decoding approach — only feasible with model-level access (not available for Claude API).

Pros: strongest guarantee.
Cons: not implementable on hosted Anthropic API; would require local model.

### Option C — Memory provenance enforcement (medium intrusive)

Every fact stored in engram already has a source (auto-store metadata). Extend recall
results to surface provenance prominently. Then add a hook that, when the agent's
draft contains an ID/path/etc. NOT present in this turn's recalled memories or tool
results, blocks emission and forces a verification tool call.

Pros: leverages existing engram metadata; doesn't require regex-heavy claim extraction.
Cons: still relies on the same regex-ish detection of "specific claims" in the draft.

### Option D — Tool-use enforcement via system prompt + sampling

Stronger system-prompt directive + few-shot examples + sampling-time check (e.g., if
draft contains `ISS-\d+` and no `gid_artifact_show` / `engram_recall` was called this
turn → reject and resample). This is Option A lite.

### Recommended starting point

**Option A (post-hoc verifier)** as v1, because:

- Implementable in pure Rust without model-level changes.
- Composes with existing engram-recall hook.
- Failure mode is "extra round-trip" not "wrong fact emitted" — fail-safe.
- Provides telemetry: count of unverified claims caught per session is a useful
  metric of how often the underlying failure happens.

Once Option A is running and we have data on what claims slip through, we can decide
whether to add Option C-style provenance gating.

## Where the code should live

This is the architectural question. Options:

- **engram crate** — the hook is a memory/retrieval concern. But "claim extraction
  from draft text" is more of an agent/runtime concern than a memory concern.
- **rustclaw crate** — the existing `engram_hooks.rs` lives here. Adding an
  `EngramVerifyHook` (HookPoint::AfterDraft or similar) alongside it is consistent.
  But then engram itself can't benefit (e.g., other consumers of engramai don't get
  the hook).
- **engramai crate, as opt-in middleware** — engramai exposes a `VerifyDraft` trait;
  rustclaw wires it into its hook chain. Other engramai users can opt in.

**Recommendation:** put the *primitive* (claim-detector + provenance-checker) in
`engramai` as a reusable module, and put the *hook integration* (HookPoint
registration, prompt rewriting) in `rustclaw`. This keeps engramai a library and
rustclaw the orchestrator.

Filing under engram repo because the long-term home of the verification primitive is
in engramai. Cross-link from rustclaw side once design lands.

## Temporary patch (in place now)

`rustclaw/skills/cite-before-claim/SKILL.md` (always_load, priority 95) — explicit
behaviour rule injected into every system prompt. Lists:

- Which claim types require verification (IDs, paths, names, versions, decisions, numbers, dates).
- Which tool to use for which claim type.
- Three honesty levels (Asserted / Hedged / Unknown).
- Failure mode to watch for: collapsing Hedged into Asserted.

Status: temporary. To be downgraded from `always_load: true` to optional once
hook-level enforcement (this ISS) is implemented.

## Acceptance criteria (rough — refine in design phase)

- [ ] Reproduce the 2026-04-28 ISS-049/ISS-056 confabulation as a test fixture.
- [ ] With the new hook active, the same prompt either (a) triggers a verification
      tool call before emitting the wrong ID, or (b) the wrong-ID draft is rewritten
      after the verifier catches it.
- [ ] False-positive rate measured: how often does the verifier flag claims that
      were actually correct? Acceptable threshold TBD in design.
- [ ] Telemetry: per-session count of unverified-claim catches, surfaced in dashboard.
- [ ] `skills/cite-before-claim/SKILL.md` can be moved out of always_load without
      regression.

## Out of scope for this ISS

- Constrained decoding (Option B) — not feasible on hosted API.
- Improving recall quality on the *user-message-keyed* recall (separate concern,
  ISS-021 / Phase 5 territory).
- Auto-store filtering of confabulated facts (separate ISS — if the agent stores a
  wrong fact, the recall layer will surface it confidently next session).

## Related

- `rustclaw/src/engram_hooks.rs` — existing recall hook, integration point.
- `rustclaw/skills/cite-before-claim/SKILL.md` — temporary patch.
- ISS-021 — recall quality / confidence labelling work in engramai.
- The 2026-04-28 incident is captured in engram memory (factual, importance 0.7) for
  future repro.
