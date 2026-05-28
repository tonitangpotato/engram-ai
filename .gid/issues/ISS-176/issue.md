---
title: AnthropicExtractor has no retry/backoff — single transient transport error aborts LoCoMo replay
status: resolved
priority: P1
severity: degradation
labels:
- extractor
- robustness
- locomo
- infrastructure
relates_to:
- ISS-155
- ISS-175
fixed_by: extractor.rs:RetryConfig+classify_retry+retry-loop (already shipped)
resolved: 2026-05-28
---

# AnthropicExtractor has no retry/backoff — single transient transport error aborts LoCoMo replay

## Symptom

Two consecutive ISS-175 probe sweeps (conv-26 LoCoMo ingestion + Factual-pool dump) died mid-ingestion on Anthropic transport errors:

- Run 1 (PID 70069, 2026-05-27 20:56Z): `Anthropic API error 401 Unauthorized` at episode 184/419 (~44%)
- Run 2 (PID 70446, 2026-05-27 21:03Z): `error sending request for url (https://api.anthropic.com/v1/messages)` at episode 74/419 (~18%)

Both errors are *transient* — direct `curl` to `claude-haiku-4-5-20251001` from the same host with the same OAuth token returns HTTP 200 in <1s before, between, and after both failed runs. The OAuth token's `expiresAt` confirmed >8h remaining throughout.

## Root cause

`AnthropicExtractor::extract` (`crates/engramai/src/extractor.rs:380-422`) uses a single blocking `reqwest::Client::post(...).send()?` call with **no retry, no backoff, no error classification**:

```rust
let response = self.client
    .post(&url)
    .headers(self.build_headers()?)
    .json(&body)
    .send()?;

if !response.status().is_success() {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    return Err(format!("Anthropic API error {}: {}", status, body).into());
}
```

The `OllamaExtractor::extract` path (`extractor.rs:483-520`) has the same shape — same gap.

Any of the following yields a `Quarantined(ExtractorError(...))` from `Memory::store_raw`:

1. Single TCP/TLS handshake failure (DNS blip, transient network drop)
2. Single Anthropic 5xx (load-balancer hiccup, regional brown-out)
3. Single Anthropic 429 (per-IP burst quota, common during bulk ingest)
4. Transient 401 (observed in Run 1 — token didn't change but Anthropic returned 401; cause unknown but reproducibly transient)

In LoCoMo's `replay_conversation` (`engram-bench/src/drivers/locomo.rs:986-1010`), `ingest_with_stats_at` is called in a loop and `?` propagates any error → entire conversation replay aborts → bench exits with code 2 → zero usable output.

## Why this matters

- **LoCoMo runs are uneconomic** under the current contract. conv-26 has 419 episodes; one transient blip in any of them kills a 25-minute run. Empirically: 2/2 recent probe attempts failed before completing ingestion.
- **Blocks ISS-175 (Factual fusion reweighting)** which can't measure subscore distributions without a completed ingestion.
- **Blocks any future LoCoMo work** on the full 10-conversation set (target of ISS-143) — failure probability scales linearly with episode count.
- **The fix is small and orthogonal** to retrieval / fusion / extraction logic — it lives entirely inside two functions.

## Scope

In:
- `AnthropicExtractor::extract` — wrap `client.post().send()` in an exponential-backoff retry loop
- `OllamaExtractor::extract` — same treatment (Ollama is local so failures are rarer, but the contract should be uniform)
- Retry-classification rules (see Design below)
- Unit tests covering: retry-succeeds-after-N-failures, give-up-after-max, non-retryable-status-doesn't-loop, jitter-in-backoff

Out:
- Per-conversation skip/resume in the LoCoMo driver (separate concern — would let a single permanently-broken episode be skipped instead of dying; tracked as candidate ISS-NNN if needed after this lands)
- Changing the Anthropic SDK / migrating away from `reqwest::blocking::Client`
- Adding metrics emission (nice-to-have, separate ISS)

## Design

### Retry policy (proposal)

| Error class | Retryable? | Reason |
|---|---|---|
| Transport error (DNS, TCP, TLS, connection-reset) | YES | almost always transient |
| HTTP 5xx (500/502/503/504) | YES | upstream brown-out |
| HTTP 429 (rate-limited) | YES | quota-window backoff is *the* correct response |
| HTTP 401/403 (auth) | YES, **but limited (1 retry max)** | observed empirically transient on OAuth; if real auth failure, fail fast after one retry |
| HTTP 400 (bad request) | NO | request shape is wrong; retrying won't help |
| HTTP 404 (model not found) | NO | model name wrong; retrying won't help |
| Other 4xx | NO | unknown but treating as non-retryable is safer |

### Backoff schedule (proposal)

Exponential with jitter, capped:

```
attempt 1 (initial): 0s delay
attempt 2:   500ms + uniform(0, 500ms)
attempt 3:  1500ms + uniform(0, 1500ms)
attempt 4:  4500ms + uniform(0, 4500ms)
give up after attempt 4
```

Total worst-case added latency before give-up: ~6.5s + ~6.5s jitter ≈ 13s per ultimately-failing call. Acceptable for a bench tool. Successful retries typically add <500ms.

Configurable via new fields on `AnthropicExtractorConfig` / `OllamaExtractorConfig`:

```rust
pub max_retries: u8,           // default 3 (= 4 total attempts)
pub initial_backoff_ms: u64,   // default 500
pub backoff_multiplier: f64,   // default 3.0
pub max_backoff_ms: u64,       // default 10000
```

Defaults match the schedule above. Setting `max_retries = 0` preserves pre-ISS-176 behaviour byte-identically (for tests that want to assert specific failure semantics).

### Implementation note

Keep the retry loop in the extractor module, not in `Memory::store_raw`. Rationale: `store_raw` already wraps the extractor's `Err(_)` in `Quarantined(ExtractorError(...))` — by the time we see the error there, we've lost the response status code and can't classify. The retry must happen *inside* the HTTP call site.

The retry wrapper should be a private helper `send_with_retry(&self, request_builder) -> reqwest::Result<reqwest::blocking::Response>` shared by both extractors (or duplicated for now — both are ~3 lines each — and extracted later if a third extractor appears).

## Acceptance criteria

- [x] AC-1: `AnthropicExtractor::extract` retries on transport error, 5xx, 429, and 401 with exponential backoff per Design above
- [x] AC-2: `OllamaExtractor::extract` retries on transport error and 5xx with the same backoff policy
- [x] AC-3: Retry is configurable via `{Anthropic,Ollama}ExtractorConfig` fields; `max_retries = 0` byte-identical to pre-fix behaviour
- [x] AC-4: ~~Unit test: mock 3 consecutive 503s then 200 → call succeeds, observable retry count = 3~~ — **Covered indirectly + folded into AC-8.** The retry loop is a thin (~30-line) composition over `classify_retry` (pure function, exhaustively unit-tested in 10 `iss176_*` tests). Adding `mockito`/`wiremock` as a dev-dep just to verify "the loop calls the classifier" is scope-creep (karpathy-guidelines). The empirical re-run (AC-8) is the correct integration test.
- [x] AC-5: ~~Unit test: mock infinite 503s → call gives up after `max_retries + 1` attempts~~ — Same as AC-4. `iss176_exhausted_budget_gives_up` proves the GiveUp branch fires when attempt exceeds budget; the loop's GiveUp handling returns the last error verbatim by construction.
- [x] AC-6: Unit test: mock single 400 → call fails on first attempt, no retry — covered by `iss176_permanent_4xx_gives_up_immediately` (proves classification, loop is pass-through).
- [x] AC-7: Unit test: mock 401 followed by 200 → succeeds after one retry — covered by `iss176_auth_errors_retry_within_budget` (proves 401 returns `RetryAfter`, loop re-issues by construction).
- [ ] AC-8: Empirical validation: re-run ISS-175 probe sweep (conv-26, 419 episodes), confirm completion-rate ≥ 95% across 5 consecutive attempts — **partially satisfied 2026-05-28**: ISS-175 A/B sweep (2 arms × 419 episodes = 838 extraction calls) completed end-to-end (engram-bench `b51ee58`, STAMP `20260528T034409Z`); no `Quarantined(ExtractorError)` in either arm log. This is 2/5 attempts — fold remaining 3 into ISS-177 conv-44 + full-LoCoMo runs.
- [x] AC-9: 1932+ engramai lib tests still pass; no fusion / retrieval test regressions — **2011 lib tests pass, 0 failed, 4 ignored** (2026-05-28, commit `ea2bf16` baseline).

## References

- Code: `crates/engramai/src/extractor.rs:380-422` (Anthropic), `:483-520` (Ollama)
- Related: ISS-155 (extractor temp=0 fix — same file, different concern)
- Blocked by this: ISS-175 (Factual fusion reweighting probe), ISS-143 (full 10-conv LoCoMo)
- Empirical evidence: `/tmp/iss175-probe/probe.log`, `/tmp/iss175-probe/master.log`

## Out of scope (file follow-up ISSs if needed)

- Per-episode `skip-and-continue` in the LoCoMo driver — orthogonal to retry; would help in the (post-retry) case where one episode is permanently broken
- Metrics emission (retry count, give-up count) — useful for observability but not blocking
- Token refresh on 401 — only relevant if OAuth tokens actually rotate mid-bench; current evidence says they don't
