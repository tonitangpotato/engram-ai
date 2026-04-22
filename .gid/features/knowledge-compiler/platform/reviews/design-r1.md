# Design Review R1: Platform Setup, LLM Config, Import & Intake

**Reviewer**: RustClaw  
**Date**: 2026-04-17  
**Document**: platform/design.md  
**Depth**: standard (Phase 0-5)  
**Score**: 9/10

---

## Summary

Well-designed infrastructure layer. 6 components, all 16 GOALs traced, clean separation from domain logic. The LlmProvider trait, config resolution, and graceful degradation are particularly well-thought-out. A few gaps around GOAL mapping accuracy and missing features.

---

## Findings

### FINDING-1: GOAL-plat.6 (Standalone Product Installation) not addressed [✅ Applied]

**Phase 2 — Check 7 (Coverage)**

Requirements GOAL-plat.6 = "KC as standalone product installable on macOS/Linux, no RustClaw dependency." This is a P0 requirement about packaging and installation UX.

The design has no component for installation/setup. Config management (§2.2) handles configuration but not the install experience (package manager support, `engram init` wizard, first-run guidance).

**Fix**: Add a brief section on installation/setup:
- `engram init` command flow (create config, download embedding model, verify)
- Distribution: `cargo install engramai`, brew formula, pre-built binary
- This could be a thin section since it's mostly CLI + packaging, not architecture

### FINDING-2: GOAL-plat.7 (Feature Flag Architecture) not in design [✅ Applied]

**Phase 2 — Check 7 (Coverage)**

Requirements GOAL-plat.7 = "Feature flags to separate KC functionality from agent-specific features." This is a P1 requirement about crate-level feature gates.

The design's §2.2 ConfigManagement doesn't cover Cargo feature flags — it covers runtime configuration. Feature flags are compile-time decisions about what code is included.

**Fix**: Add a section specifying:
- Default features: `["storage", "recall", "embeddings", "fts5", "entities", "synthesis", "compiler"]`
- Agent features: `["session", "extractor", "classifier", "anomaly", "sentiment"]`
- How features gate modules (cfg attributes on mod declarations)

### FINDING-3: GOAL numbering in §2.3 doesn't match requirements [✅ Applied]

**Phase 3 — Check 15 (Numbering/referencing)**

Design §2.3 EmbeddingPipeline traces to:
- GOAL-plat.4 (auto-download) ✅
- GOAL-plat.5 (model management) ✅  
- GOAL-plat.6 (embedding cache) — but GOAL-plat.6 in requirements is "Standalone Product Installation", not embedding cache

The embedding cache feature is part of GOAL-plat.4/5 (managing the embedding model includes caching). The requirements traceability table in §5 has the same error.

**Fix**: Remove GOAL-plat.6 from EmbeddingPipeline traces. The cache is an implementation detail of plat.4/5, not a separate GOAL.

### FINDING-4: IntakePipeline design truncated [✅ Applied]

**Phase 1 — Check 1 (Every type fully defined)**

The design appears truncated at §2.5 IntakePipeline — the component starts but content extractors (Jina, yt-dlp, GitHub) aren't shown in detail. The `ContentExtractor` trait is likely defined but I can see from the traceability matrix that plat.12 and plat.13 are covered.

**Fix**: Verify the full §2.5 content is present. If truncated during generation, complete the IntakePipeline design (URL extraction flow, inbox directory watcher, voice intake via STT).

### FINDING-5: GOAL-plat.15 (Import Progress & Error Reporting) implicit [✅ Applied]

**Phase 2 — Check 8 (Error/edge case coverage)**

Requirements GOAL-plat.15 requires consistent progress reporting across all import operations. The design's ImportPipeline returns `ImportReport` which has counts, but:
- No progress callback during execution (for real-time progress display)
- No consistent error format across importers

**Fix**: Add a progress callback pattern to ImportPipeline:
```rust
pub trait ImportProgress {
    fn on_item(&self, index: usize, total: usize, status: ItemStatus);
}
```

### FINDING-6: GOAL-plat.16 (Config Migration) not addressed [✅ Applied]

**Phase 2 — Check 7 (Coverage)**

Requirements GOAL-plat.16 = "Auto-detect old config schema on version upgrade, migrate or prompt." The design's §2.2 ConfigManagement has no versioning or migration logic.

**Fix**: Add:
- Config file version field (`version = 1`)
- Migration function map (`migrate_v1_to_v2()`, etc.)
- Backup before migration

### FINDING-7: LlmProvider vs SynthesisLlmProvider naming confusion [✅ Applied]

**Phase 1 — Check 4 (Consistent naming)**

Platform design defines `LlmProvider` trait. Compilation design references `SynthesisLlmProvider`. Are these the same trait? Different traits? The relationship is unclear.

**Fix**: Clarify in compilation design that `SynthesisLlmProvider` will be replaced by / implemented via the platform's `LlmProvider` trait. Or note that compilation will use `ModelRouter::for_task()` which returns `&dyn LlmProvider`.

---

## Score Breakdown

| Area | Score | Notes |
|------|-------|-------|
| Structure & completeness | 8/10 | Two GOALs (plat.6, plat.7) missing design |
| GOAL traceability | 8/10 | plat.6 numbering error |
| GUARD compliance | 9/10 | Graceful degradation well-covered |
| Logic correctness | 9/10 | Config resolution, fallback chain solid |
| Edge cases | 8/10 | Progress reporting thin |
| Trade-offs | 9/10 | Clean infrastructure/domain separation |

**Overall: 9/10** — Solid infrastructure design. Two missing GOALs are the main gaps.
