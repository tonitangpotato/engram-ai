# ISS-026: ~~Redo Clippy Lint Cleanup in Monorepo~~ (SUPERSEDED by ISS-025)

**Status:** closed — wontfix
**Superseded by:** ISS-025

## Why Closed

ISS-026 originally proposed rerunning clippy on the monorepo **instead of** porting `71f3654` from the deprecated repo. That plan was wrong — it would have dropped the real code fixes in `71f3654`:

- `src/memory.rs`: redundant `if/else` branch removed (both arms produced `limit * 3`)
- `src/memory.rs`: struct update syntax for `RetryReport::default()` init
- `src/metacognition.rs`: `Iterator::flatten` instead of `for + if let Ok`
- `src/enriched.rs`: rustdoc indentation fix

These are genuine improvements, not just `#[allow(...)]` annotations.

**ISS-025 v3 does the correct thing:** port `71f3654` via `git format-patch` + `sed` path rewrite + `git am --3way`. This preserves the commit message, author, timestamp, and all real fixes.

See ISS-025 Step 2 for the port procedure.
