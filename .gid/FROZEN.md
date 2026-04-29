# engram/.gid/graph.db is FROZEN (read-only) as of 2026-04-28

## Decision

This graph (the engram main issue tracker) is **read-only**. All future engram
v0.3 development uses `.gid-v03-context/graph.db` instead.

## Why

- Mixed-case IDs (ISS-NNN coexisting with iss-NNN) from multiple backfill passes
- Dangling edge refs (ISS-047, ISS-046, iss-044, iss-049) from incomplete migrations
- gid-core validator has known false-positive bug on case-mismatch (gid-rs ISS-061)
- Continuing to patch this graph adds friction without value

## Rule for agents

**For ANY engram work** — v0.3 build, v0.3 issues, v0.3 features, design,
implementation, retrieval, ingestion, anything:

```
graph_path: /Users/potato/clawd/projects/engram/.gid-v03-context/graph.db
```

NEVER:

```
graph_path: /Users/potato/clawd/projects/engram/.gid/graph.db    # frozen
project: engram                                                   # ambiguous
```

## How to thaw (only if absolutely needed)

```
chmod u+w /Users/potato/clawd/projects/engram/.gid/graph.db
```

But ask potato first. The freeze is intentional.

## What's still alive in `.gid/`

- Issue **artifacts** (`.gid/issues/ISS-NNN/issue.md`) — still the source of truth
  for issue text, frontmatter, status. These are NOT the graph.
- Use `gid_artifact_show` / `gid_artifact_new` / `gid_artifact_update` on those —
  they edit markdown files, not the graph DB.

## Last state before freeze

- 28 issue nodes (ISS-001 through ISS-065 with gaps)
- Last backfill: 2026-04-28 (added ISS-030, 031, 036, 040, 041, 042, 043, 062, 065)
- Validation: 16 issues, mostly pre-existing dangling refs + 1 validator bug
