# Requirements: Multi-Signal Hebbian Link Formation

**Status**: Implemented — `synthesis/cluster.rs` runs 4 signals (Hebbian + entity Jaccard + embedding cosine + temporal). ISS-015 (clustering upgrade → Infomap) and ISS-016 (LLM triple extraction) both closed. Ongoing tuning via ISS-012 importance calibration (closed).
**Last reviewed**: 2026-04-20

## Overview

The Hebbian link system currently forms associations only through co-recall events (two memories returned together in a query). This feature enables proactive association discovery at memory write-time using multiple signals: entity overlap, embedding similarity, and temporal proximity. New memories will immediately gain connections to related existing memories, improving discoverability and reducing the cold-start problem for newly stored content.

## Priority Levels

- **P0**: Core — required for the system to function at all
- **P1**: Important — needed for production-quality operation
- **P2**: Enhancement — improves efficiency, UX, or observability

## Guard Severity

- **hard**: Violation = system is broken, execution must stop
- **soft**: Violation = degraded quality, should warn but can continue

## Goals

### Write-Time Association Discovery

- **GOAL-1** [P0]: When a new memory is stored, the system evaluates it against existing memories and creates Hebbian links before the write operation completes *(ref: multi-signal-hebbian.md, §3.3 Write-time Association Discovery)*

- **GOAL-2** [P1]: Write-time association discovery can be enabled or disabled via configuration without code changes *(ref: multi-signal-hebbian.md, §3.5 Signal Weights Configuration)*

- **GOAL-3** [P1]: The number of candidate memories evaluated for association is bounded and configurable to prevent unbounded computation *(ref: multi-signal-hebbian.md, §3.4 Candidate Selection Strategy)*

### Multi-Signal Scoring

- **GOAL-4** [P0]: Association strength between two memories is computed using three signals: entity overlap (comparing extracted entities), embedding similarity (vector cosine distance), and temporal proximity (time between memory creation) *(ref: multi-signal-hebbian.md, §3.3 Write-time Association Discovery)*

- **GOAL-5** [P1]: Each signal's contribution to the final association score is independently configurable via weights *(ref: multi-signal-hebbian.md, §3.5 Signal Weights Configuration)*

- **GOAL-6** [P1]: Links are only created when the combined signal score exceeds a configurable threshold *(ref: multi-signal-hebbian.md, §3.3 Write-time Association Discovery)*

### Link Formation and Management

- **GOAL-7** [P0]: Each memory has a configurable maximum number of write-time links to prevent link explosion *(ref: multi-signal-hebbian.md, §3.3 Write-time Association Discovery)*

- **GOAL-8** [P0]: When the link budget is exceeded, only the strongest associations (highest combined scores) are retained *(ref: multi-signal-hebbian.md, §3.3 Write-time Association Discovery)*

- **GOAL-9** [P1]: Each Hebbian link records which signal(s) caused its formation (co-recall, entity overlap, embedding similarity, temporal proximity, or multi-signal combination) *(ref: multi-signal-hebbian.md, §3.2 Schema Changes)*

- **GOAL-10** [P1]: Each Hebbian link records the individual signal scores that contributed to its formation for analysis and debugging *(ref: multi-signal-hebbian.md, §3.2 Schema Changes)*

### Coexistence with Co-Recall

- **GOAL-11** [P0]: Write-time discovered links coexist with co-recall links — the system supports both link types simultaneously *(ref: multi-signal-hebbian.md, §3.6 Relationship with co-recall)*

- **GOAL-12** [P0]: When two memories already have a write-time link and are subsequently co-recalled, the link strength increases rather than creating a duplicate link *(ref: multi-signal-hebbian.md, §3.6 Relationship with co-recall)*

### Differential Decay

- **GOAL-13** [P1]: Hebbian links decay at different rates based on their signal source — co-recall links decay slowest, single-signal write-time links decay fastest *(ref: multi-signal-hebbian.md, §3.7 Decay Strategy Adjustment)*

### Performance

- **GOAL-14** [P1]: Write-time association discovery adds no more than 50ms of latency to memory write operations (measured as p95 latency increase) *(ref: multi-signal-hebbian.md, §3.8 Performance Considerations)*

## Guards

- **GUARD-1** [hard]: Hebbian link operations never create duplicate links between the same pair of memories — each (source_id, target_id) pair appears at most once *(ref: multi-signal-hebbian.md, §3.6 Relationship with co-recall)*

- **GUARD-2** [soft]: All Hebbian link signal scores and metadata are persisted — no signal provenance is lost during link formation or updates *(ref: multi-signal-hebbian.md, §3.2 Schema Changes)*

- **GUARD-3** [soft]: Hebbian link operations never block memory write operations for more than the configured timeout (default 100ms) — if association discovery times out, the write completes without new links *(ref: multi-signal-hebbian.md, §3.8 Performance Considerations)*

## Out of Scope

- Cross-namespace write-time associations — only same-namespace links are discovered at write-time (co-recall cross-namespace links remain as-is)
- LLM-based association signals — only zero-LLM signals (entity, embedding, temporal) are used
- Retroactive backfill of write-time links for historical memories — existing memories retain only their co-recall links until re-indexed
- Real-time link strength updates based on user feedback — link strength only changes via co-recall reinforcement or time-based decay

## Dependencies

- Entity extraction system (`entities.rs`) — required to compute entity overlap signal
- Embedding generation system (`hybrid_search.rs`) — required to compute embedding similarity signal
- FTS (Full-Text Search) — required for efficient candidate selection
- SQLite schema migration support — required to add new columns to `hebbian_links` table without data loss

---

**14 GOALs** (6 P0 / 8 P1 / 0 P2) + **3 GUARDs** (1 hard / 2 soft)
