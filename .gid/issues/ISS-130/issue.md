---
id: ISS-130
title: 'Retire v0.2 KC: delete 19 unused modules in compiler/, preserve 2 (intake/import, manual_edit) for v0.4 substrate writer re-integration'
status: blocked
priority: P3
severity: minor
created: 2026-05-15
depends_on: engram-cli-knowledge-migration-decision
relates_to:
- ISS-106
labels:
- substrate
- v04
- cleanup
- v02-kc
- retirement
---

# Problem

`crates/engramai/src/compiler/` is the v0.2 KnowledgeCompiler. It compiles
but is **dead code in production**:

- `KnowledgeCompiler::new` has **zero call sites** outside `compiler/`
  itself (verified 2026-05-12 + re-verified 2026-05-15 via
  `grep -rn 'KnowledgeCompiler::new' crates/engramai/src/ | grep -v compiler/`).
- `Memory::compile_knowledge` (`crates/engramai/src/memory.rs:6670`)
  fully routes to v0.3 via `crate::knowledge_compile::compile` (sees
  doc-comment chain at lines 6640-6666).
- `compiler/` ≈ 21 modules. Used only by tests / integration scaffolding,
  not by any production consumer.

It's been a mausoleum since the v0.3 KC landed. Carrying it forward
through Phase E/F is unnecessary risk surface — anyone reading the code
has to figure out which compiler is the real one.

This task is **T60 part 2** in `v04-unified-substrate/design.md` §8.14.
Part 1 (the verification) is now ticked.

# Module inventory

21 files in `crates/engramai/src/compiler/`:

```
api.rs              compilation.rs      config.rs           conflict.rs
decay.rs            degradation.rs      discovery.rs        export.rs
feedback.rs         health.rs           import.rs           intake.rs
llm.rs              lock.rs             manual_edit.rs      mod.rs
privacy.rs          storage.rs          topic_lifecycle.rs  types.rs
watcher.rs
```

# Disposition per design §4.16.3

**Delete (19 modules):**
- `api.rs`, `compilation.rs`, `config.rs`, `conflict.rs`, `decay.rs`,
  `degradation.rs`, `discovery.rs`, `export.rs`, `feedback.rs`,
  `health.rs`, `llm.rs`, `lock.rs`, `privacy.rs`, `storage.rs`,
  `topic_lifecycle.rs`, `types.rs`, `watcher.rs`
- Plus `mod.rs` once the module is empty.
- The clustering/summarization concepts these encode are already
  re-implemented in v0.3 `knowledge_compile/`. Where v0.3's version
  is missing a feature (e.g. degradation), file a targeted ISS — do
  not preserve the v0.2 implementation as a fallback.

**Preserve, but re-home (2 modules):**
- `intake.rs` / `import.rs`: external-content intake + import pipeline.
  These describe **substrate writers** (write external knowledge into
  the graph) and belong in the v0.4 unified writer queue (§6.1 `WriteOp`).
  Move into `substrate/` or `writer/` and adapt their data flow to the
  new `WriteOp` API once T61–T68 land.
- `manual_edit.rs`: human-in-the-loop edit pipeline. Same story —
  belongs in the writer queue, not in a parallel "knowledge compiler"
  module tree.

# Acceptance

1. The 19 deletion-candidates are removed from `crates/engramai/src/compiler/`.
2. `mod.rs` is either deleted (if 0 modules remain) or trimmed to only
   `pub mod intake;` / `pub mod import;` / `pub mod manual_edit;` (or
   their new module paths post-rehome).
3. Engramai crate compiles without the 19 removed modules. Any test or
   binary that referenced them is either updated or deleted.
4. 1900+ lib tests still pass.
5. `Memory::compile_knowledge` continues to route through v0.3
   (regression check: existing knowledge_compile tests stay green).
6. The 2 preserved modules either (a) move to their new substrate
   home, or (b) get a `// TODO(v04): rehome into substrate::writer
   per ISS-130` comment if the writer queue isn't ready yet — and the
   issue is updated to track the re-home as a sub-task.

# Why this is blocked / not immediately actionable

**Soft block on ISS-111.** v0.3 KC has a known degeneration on
single-domain corpora (LoCoMo conv-26 evidence in RUN-0026, −22pp J-score
vs RUN-0025 baseline). If ISS-111 ends up requiring a fallback that
borrows from v0.2 patterns (unlikely but possible), the deletion gets
harder. Per design §8.14: "Block on ISS-111 being either fixed OR
confirmed orthogonal to retirement."

Conservative reading: do the deletion **after** ISS-111 is closed (fix
or wontfix), so we have one less variable in play during the v0.4
phase D → E transition.

# Scope

In scope: delete 19 modules, fix compilation, run tests, possibly move 2
modules.

Out of scope:
- Implementing the v0.4 writer queue (T61–T68). The 2 preserved modules
  may need to wait for that work to land before they can be properly
  re-homed.
- Removing entries from `mod.rs` of the parent (`lib.rs` re-exports) if
  they're public API — that's a breaking change requiring a minor version
  bump. Audit during execution.
- Migrating any external bench / integration test that depends on
  `compiler::*` (we'll fix what we break).

# Discovery context

## 2026-05-15 — Audit found CLI dependency, scope reassessment needed

Re-audited `engramai::compiler::*` consumers across the full workspace
(not just `crates/engramai/src/`). Found two consumers, not zero:

1. **`crates/engramai/tests/kc_integration_test.rs`** — v0.2 KC's own
   integration tests. Test-only, can be deleted with the module.

2. **`crates/engram-cli/src/main.rs`** — **active CLI surface**:
   - Top-level `engram knowledge` subcommand (line 591)
   - 11 sub-subcommands: `query`, `inspect`, `export`, `import`,
     `health`, `decay`, `conflicts`, `audit`, `privacy`, `compile`,
     `list` (lines 831-944)
   - Implementation block at line 2259-end uses
     `MaintenanceApi`, `TopicDiscovery`, `DecayEngine`,
     `ConflictDetector`, `HealthAuditor`, `ImportPipeline`,
     `ExportEngine`, `PrivacyGuard`, `SqliteKnowledgeStore`,
     `KcConfig`, `NoopProvider`, `TopicId`, `ConflictScope`,
     `ConflictType`, `DecayAction`, …

The original Problem section ("zero call sites outside compiler/")
only grep'd `crates/engramai/src/` and missed `engram-cli`.

### Implications

This is **not** a simple "delete 19 dead modules, fix 1 test file"
job. v0.2 KC is the implementation behind 11 user-facing CLI
subcommands.

v0.3 KC currently exposes only `compile` and `list_topics` — there
is **no replacement** for export, import, health, decay, conflicts,
audit, or privacy surfaces. Deleting `compiler/` without addressing
the CLI would break the CLI; keeping the CLI without `compiler/`
requires re-implementing those 11 surfaces on top of v0.3.

### Options (need potato's call)

- **(X) Re-scope this ISS to "v0.2 KC retirement is blocked on
  engram-cli rewrite".** Keep compiler/ frozen. File a new ISS for
  the CLI migration work (large, design-required). This issue stays
  open with depends_on updated.

- **(Y) Two-step retirement.** First migrate the 11 CLI subcommands
  to v0.3 KC (adding missing v0.3 surfaces for export/import/health/
  decay/etc.) as a separate feature. Then delete compiler/. This is
  the most work but preserves user-facing surface.

- **(Z) Delete CLI surface together with compiler/.** Acknowledge
  v0.2 KC CLI is unused dead surface in practice. Single PR drops
  both. Major version bump for engram-cli. Requires confirmation
  that no one (including potato) actively uses `engram knowledge *`.

### Recommendation

Need potato to weigh in on (X) / (Y) / (Z) before any code change.
Defaulting to (X) for now: change ISS-130 scope to capture the CLI
dependency, leave compiler/ alone until CLI question is answered.

### What ISS-111 closing changed

ISS-111 (clusterer collapse) was the original soft-block. It's now
done — the algorithmic concern is gone. But this audit reveals a
**hard** block (active CLI consumer) that the original ISS body did
not catch. Scope must expand or split before this ISS is actionable.

### Original framing (now superseded)

T60 verification (design §8.14) said v0.2 KC had zero production
call sites. That was based on grep'ing `crates/engramai/src/` only.
With engram-cli included, the call-site count is non-zero. The T60
verification status in design.md may need a correction note.
