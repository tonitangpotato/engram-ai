---
title: CLI store/recall/reward: positional content with leading dash crashes clap (needs flag-based content arg)
status: open
priority: P1
severity: medium
tags: [cli, ergonomics, ingest, locomo]
relates_to: [ISS-081]
---

# ISS-082: CLI positional `<CONTENT>` crashes on leading-dash payloads

## Summary

`engram store <CONTENT>` (and `recall <QUERY>`, `reward <FEEDBACK>`) take their primary
payload as a clap **positional** argument. When the payload starts with `-`
(e.g. `-0`, `-50`, `--bullet point`), clap parses it as an unknown flag and
crashes with rc=2 before reaching command logic — even though the payload was
positionally placed last on the command line.

This is fundamentally a clap ergonomics issue: positional args that follow optional
flags can't safely accept arbitrary user-generated text without a `--` sentinel
separator, and even with `--`, downstream callers that don't know to insert it
will hit silent corruption.

## Reproduction

```
$ engram --database /tmp/x.db --workspace /tmp store --ns t --type episodic \
    --importance 0.6 --extractor anthropic --auth-token sk-... --oauth -0
error: unexpected argument '-0' found

  tip: to pass '-0' as a value, use '-- -0'

Usage: engram store [OPTIONS] <CONTENT>
```

The `-- ` sentinel form works:
```
$ engram --database /tmp/x.db --workspace /tmp store ... -- -0
✓ stored
```

But this is a footgun: callers must remember to add `--` *unconditionally*
before any user-controlled content, even when the content currently doesn't
start with `-`.

## Discovery context

Found 2026-04-30 during cogmembench LoCoMo conv-26 ingest. The
`engram_adapter.py` originally passed content as the last positional arg.
At turn ~92 (session 5/6), an LLM-extracted memory or a pre-formatted
"Speaker: text" string evidently produced content starting with `-0`,
crashing the entire ingest. Adapter was patched to add `--` sentinel
(cogmembench fix) — that addresses the *caller* side, but the CLI still
has the same trap for any other consumer.

Related: ISS-081 (LoCoMo run-71/199 regression — different root cause:
stderr-substring guard in adapter masking clap exit codes).

## Proposed fix

Add a flag-based alternative for content/query/feedback so callers can
opt out of positional parsing entirely:

```
engram store --content "<text>" [other flags...]      # NEW preferred form
engram store [other flags...] -- "<text>"             # current (still works)
engram store [other flags...] "<text>"                # current (footgun if text starts with -)
```

Same treatment for `recall --query` and `reward --feedback`.

### Implementation sketch

In `cli/store.rs` (and recall/reward equivalents):

```rust
#[derive(clap::Args)]
struct StoreArgs {
    /// Memory content (preferred form — flag-friendly)
    #[arg(long, conflicts_with = "content_pos")]
    content: Option<String>,

    /// Memory content (positional — backward compatible; requires `--` if leading dash)
    #[arg(value_name = "CONTENT")]
    content_pos: Option<String>,

    // ...other flags...
}

impl StoreArgs {
    fn resolved_content(&self) -> Result<&str> {
        match (&self.content, &self.content_pos) {
            (Some(c), None) | (None, Some(c)) => Ok(c.as_str()),
            (Some(_), Some(_)) => Err(anyhow!("--content and positional CONTENT are mutually exclusive")),
            (None, None) => Err(anyhow!("content required (use --content or positional)")),
        }
    }
}
```

`clap` handles the mutual exclusion via `conflicts_with`.

## Acceptance criteria

- [ ] `engram store --content "-0"` succeeds (no `--` needed)
- [ ] `engram store -- "-0"` still succeeds (back-compat)
- [ ] `engram store "-0"` still fails as before (back-compat — explicit positional behavior unchanged)
- [ ] `engram store --content "x" "y"` errors with mutual-exclusion message
- [ ] Same flags added for `recall --query` and `reward --feedback`
- [ ] Docs (`docs/cli.md` or equivalent) updated to recommend `--content` form for programmatic callers
- [ ] cogmembench `engram_adapter.py` migrated to `--content` form (drop `--` sentinel)

## Out of scope

- Changing the default to *require* `--content` (breaks every existing script)
- Stdin-based content (`engram store --content - < file`) — separate feature, low priority

## Notes

- This is a P1 ergonomics issue, not a P0 correctness bug — current `--` workaround
  unblocks LoCoMo. But every new caller will hit the same trap.
- The positional form is convenient for humans typing one-off commands; keep it.
  This issue is about giving programmatic callers a safer interface.
