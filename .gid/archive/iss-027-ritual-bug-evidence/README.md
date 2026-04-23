# Evidence for ISS-027 (ritual workspace guard bug)

These 3 ritual state files were created in `engram-ai-rust/.gid/rituals/` on 2026-04-22, **~2.5 to ~5 hours after** ISS-023 consolidation made `engram/` the canonical repo at 18:30 EDT.

## Timeline

| File | UTC timestamp | EDT | Gap from consolidation |
|---|---|---|---|
| `r-e9410e.json` | 2026-04-23T01:16:50Z | 2026-04-22 21:16 EDT | +2h 46m |
| `r-5fd66f.json` | 2026-04-23T03:26:21Z | 2026-04-22 23:26 EDT | +4h 56m |
| `r-5ff35a.json` | 2026-04-23T03:26:29Z | 2026-04-22 23:26 EDT | +4h 56m |

The ritual launcher accepted the deprecated repo as `target_root` without any validation. No deprecation marker, no dirty-tree refusal, no remote check — nothing.

## Why these are evidence

Each of these rituals wrote to `engram-ai-rust/` (graph updates, phase state transitions) while `engram/` was the canonical working tree. All of the work they produced is now either:
- Ported to monorepo (see ISS-025)
- Orphaned (dead work, since no one will build the deprecated repo)

## Expected fix (per ISS-027)

The launcher should refuse to start a ritual if:
1. `<workspace>/.gid/DEPRECATED_DO_NOT_RITUAL` exists, OR
2. `git remote get-url origin` returns a deprecated-style URL (e.g., `deprecated-origin`), OR
3. `<workspace>/DEPRECATED.md` exists

See ISS-027 for the full guard design.
