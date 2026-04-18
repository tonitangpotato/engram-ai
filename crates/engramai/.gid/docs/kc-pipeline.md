# KC (Knowledge Compiler) — Development Pipeline

> 从 requirements → design → graph → implement 的完整执行计划。
> Autopilot 用：每个 task 有精确的输入/输出路径、使用的 skill、完成标准。

## 项目根目录

```
/Users/potato/clawd/projects/engram-ai-rust/
```

## 当前文件结构

```
.gid/
├── docs/
│   └── kc-pipeline.md                                    ← 本文件
├── features/knowledge-compiler/
│   ├── requirements.md                                   ← Master: 6 GUARDs + Feature Index (39 GOALs total)
│   ├── compilation/
│   │   ├── requirements.md                               ← 10 GOALs (comp.1-10)
│   │   └── reviews/
│   │       └── requirements-r1.md                        ← ✅ 完成 (6 Critical + 12 Important + 3 Minor)
│   ├── maintenance/
│   │   ├── requirements.md                               ← 13 GOALs (maint.1-5b, 6-12)
│   │   └── reviews/
│   │       └── requirements-r1.md                        ← ✅ 完成
│   └── platform/
│       ├── requirements.md                               ← 16 GOALs (plat.1-16)
│       └── reviews/
│           └── requirements-r1.md                        ← ✅ 完成 (3 Critical + 6 Important + 4 Minor)
└── graph.yml
```

---

## TASK 1: Requirements Review — maintenance ⬜
- **Skill**: `review-requirements` (standard depth)
- **Input**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/requirements.md`
- **Also read**: `.gid/features/knowledge-compiler/requirements.md` (master GUARDs)
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/requirements-r1.md`
- **Done when**: Review file exists with all 26 checks (standard depth) documented

## TASK 2: Requirements Review — platform ⬜
- **Skill**: `review-requirements` (standard depth)
- **Input**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/requirements.md`
- **Also read**: `.gid/features/knowledge-compiler/requirements.md` (master GUARDs)
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/requirements-r1.md`
- **Done when**: Review file exists with all 26 checks (standard depth) documented

## TASK 3: Apply Review Findings — compilation ⬜
- **Depends on**: (already done — compilation review exists)
- **Skill**: `apply-review`
- **Input review**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/reviews/requirements-r1.md`
- **Target doc**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/requirements.md`
- **Action**: Apply all findings (unless a finding conflicts — skip and note)
- **Done when**: Target doc updated, review file findings marked ✅ Applied

## TASK 4: Apply Review Findings — maintenance ⬜
- **Depends on**: TASK 1
- **Skill**: `apply-review`
- **Input review**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/requirements-r1.md`
- **Target doc**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/requirements.md`
- **Action**: Apply all findings
- **Done when**: Target doc updated, review file findings marked ✅ Applied

## TASK 5: Apply Review Findings — platform ⬜
- **Depends on**: TASK 2
- **Skill**: `apply-review`
- **Input review**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/requirements-r1.md`
- **Target doc**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/requirements.md`
- **Action**: Apply all findings
- **Done when**: Target doc updated, review file findings marked ✅ Applied

## TASK 6: Update Master Requirements (if needed) ⬜
- **Depends on**: TASK 3, 4, 5
- **Input**: All 3 updated feature requirements + master requirements
- **Target**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/requirements.md`
- **Action**: Ensure GUARDs, feature index, cross-reference table are consistent with applied changes
- **Done when**: Master doc consistent, no stale references

---

## TASK 7: Write Design Doc — Master Architecture ✅
- **Depends on**: TASK 6
- **Skill**: `draft-design`
- **Input requirements**:
  - Master: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/requirements.md`
  - All 3 feature requirements (for cross-cutting understanding)
- **Also read**: existing engram source for integration points
  - `/Users/potato/clawd/projects/engram-ai-rust/src/synthesis/` (existing synthesis engine)
  - `/Users/potato/clawd/projects/engram-ai-rust/src/consolidation.rs`
  - `/Users/potato/clawd/projects/engram-ai-rust/src/entities.rs`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
- **Content**: Architecture overview, cross-cutting concerns, data flow between features, shared types, feature index
- **Rule**: ≤8 components, no per-feature details (those go in feature design docs)
- **Done when**: File written with §1-§5 per draft-design skill

## TASK 8: Write Design Doc — compilation ⬜
- **Depends on**: TASK 7 (needs architecture context)
- **Skill**: `draft-design`
- **Input requirements**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/requirements.md`
- **Input architecture**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/design.md`
- **Content**: Topic discovery, compilation pipeline, incremental triggers, merge/split, user feedback
- **Rule**: ≤8 components, reference architecture.md for shared types
- **Done when**: File written, every GOAL-comp.* traced to at least one component

## TASK 9: Write Design Doc — maintenance ✅
- **Depends on**: TASK 7
- **Skill**: `draft-design`
- **Input requirements**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/requirements.md`
- **Input architecture**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/design.md`
- **Content**: Decay, conflict detection, health reports, export, CLI, API, privacy
- **Rule**: ≤8 components
- **Done when**: File written, every GOAL-maint.* traced

## TASK 10: Write Design Doc — platform ⬜
- **Depends on**: TASK 7
- **Skill**: `draft-design`
- **Input requirements**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/requirements.md`
- **Input architecture**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/design.md`
- **Content**: LLM provider abstraction, config, graceful degradation, setup, import, intake
- **Rule**: ≤8 components
- **Done when**: File written, every GOAL-plat.* traced

---

## TASK 11: Design Review R1 — architecture ⬜
- **Depends on**: TASK 7
- **Skill**: `review-design` (if exists) or manual structured review
- **Input**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/reviews/architecture-r1.md`
- **Done when**: Review file written with findings

## TASK 12: Design Review R1 — compilation ⬜
- **Depends on**: TASK 8
- **Input**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/design.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/reviews/design-r1.md`

## TASK 13: Design Review R1 — maintenance ⬜
- **Depends on**: TASK 9
- **Input**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/design.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/design-r1.md`

## TASK 14: Design Review R1 — platform ⬜
- **Depends on**: TASK 10
- **Input**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/design.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/design-r1.md`

## TASK 15: Apply Design R1 Findings — all 4 docs ⬜
- **Depends on**: TASK 11, 12, 13, 14
- **Skill**: `apply-review`
- **Action**: Apply all findings from R1 to each design doc
- **Done when**: All 4 design docs updated, R1 findings marked ✅

## TASK 16: Design Review R2 — architecture ⬜
- **Depends on**: TASK 15
- **Input**: Updated `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/reviews/architecture-r2.md`

## TASK 17: Design Review R2 — compilation ⬜
- **Depends on**: TASK 15
- **Input**: Updated compilation/design.md
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/reviews/design-r2.md`

## TASK 18: Design Review R2 — maintenance ⬜
- **Depends on**: TASK 15
- **Input**: Updated maintenance/design.md
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/design-r2.md`

## TASK 19: Design Review R2 — platform ⬜
- **Depends on**: TASK 15
- **Input**: Updated platform/design.md
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/design-r2.md`

## TASK 20: Apply Design R2 Findings ⬜
- **Depends on**: TASK 16, 17, 18, 19
- **Skill**: `apply-review`
- **Action**: Apply R2 findings, lock design docs
- **Done when**: Design docs finalized, no open Critical findings

---

## TASK 21: Generate GID Graph ⬜
- **Depends on**: TASK 20
- **Tool**: `gid_design` on each design doc
- **Input**:
  - `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`
  - `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/design.md`
  - `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/design.md`
  - `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/design.md`
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/graph.yml`
- **Content**: Component nodes, task nodes (with design_ref, satisfies, depends_on, status: todo)
- **Done when**: Graph generated, `gid_validate` passes (no cycles, no orphans)

## TASK 22: Graph Review ⬜
- **Depends on**: TASK 21
- **Tools**: `gid_validate`, `gid_advise`, manual inspection
- **Checks**:
  - Every GOAL-* has at least one task with `satisfies` pointing to it
  - Task dependencies form a valid DAG (no cycles)
  - Task granularity is right (each task = 1-3 hours of work)
  - No orphan tasks (every task reachable from a feature node)
  - `gid_advise` recommendations addressed
- **Output**: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/reviews/graph-r1.md`
- **Done when**: Graph clean, review notes addressed, ready for implementation

---

## TASK 23: Implementation ⬜
- **Depends on**: TASK 22
- **Tool**: `gid_tasks` for topological order
- **Workspace**: `/Users/potato/clawd/projects/engram-ai-rust/`
- **Process per task**:
  1. `gid_tasks --status todo` → pick next ready task (all deps done)
  2. `gid_update_task <id> --status in_progress`
  3. Read task's `design_ref` section from design doc for implementation context
  4. Implement in Rust under `src/compiler/` (or wherever design specifies)
  5. Write tests covering the GOAL's pass/fail criteria
  6. `cargo test` — all pass
  7. `gid_complete <id>` → see what's unblocked
  8. Repeat
- **Test requirement**: Each GOAL must have at least one test that verifies its acceptance criteria
- **Done when**: All tasks in graph are `done`, `cargo test` all pass, `gid_tasks --status todo` returns empty

---

## Dependency Graph (visual)

```
TASK 1 (review maint) ──→ TASK 4 (apply maint) ──┐
TASK 2 (review plat)  ──→ TASK 5 (apply plat)  ──┤
(compilation review done) → TASK 3 (apply comp) ──┤
                                                   ├──→ TASK 6 (update master)
                                                   │
                                                   ▼
                                            TASK 7 (arch design)
                                           ╱    │    ╲
                                    TASK 8    TASK 9   TASK 10
                                  (comp)    (maint)   (plat)
                                     │         │        │
                                  TASK 12   TASK 13   TASK 14  ← R1 reviews
                                     └────┬────┘────┬──┘
                                       TASK 11 (arch R1)
                                          │
                                       TASK 15 (apply R1)
                                     ╱    │    ╲    ╲
                               TASK 16  TASK 17  TASK 18  TASK 19 ← R2 reviews
                                     └────┬────┘────┬──┘
                                       TASK 20 (apply R2)
                                          │
                                       TASK 21 (gen graph)
                                          │
                                       TASK 22 (graph review)
                                          │
                                       TASK 23 (implement)
```

---

## Autopilot Task List

> `/autopilot` reads this section. Each `- [ ]` is one task for the agent.

- [x] TASK 3: Apply review findings to compilation/requirements.md. Input review: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/reviews/requirements-r1.md`. Target: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/requirements.md`. Use apply-review skill. Apply all findings unless a finding conflicts — skip and note. Done when: target doc updated, findings marked ✅ Applied.
- [x] TASK 1: Review maintenance requirements using review-requirements skill (standard depth, Phases 0-5). Input: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/requirements.md`. Also read master GUARDs from `.gid/features/knowledge-compiler/requirements.md`. Write review to `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/requirements-r1.md`. Create reviews/ dir if needed. Done when: review file exists with all 26 checks documented.
- [x] TASK 2: Review platform requirements using review-requirements skill (standard depth, Phases 0-5). Input: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/requirements.md`. Also read master GUARDs from `.gid/features/knowledge-compiler/requirements.md`. Write review to `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/requirements-r1.md`. Create reviews/ dir if needed. Done when: review file exists with all 26 checks documented.
- [x] TASK 4: Apply review findings to maintenance/requirements.md. Input review: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/requirements-r1.md`. Target: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/requirements.md`. Use apply-review skill. Apply all findings. Done when: target doc updated, findings marked ✅ Applied.
- [x] TASK 5: Apply review findings to platform/requirements.md. Input review: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/requirements-r1.md`. Target: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/requirements.md`. Use apply-review skill. Apply all findings. Done when: target doc updated, findings marked ✅ Applied.
- [x] TASK 6: Update master requirements for consistency. Read all 3 updated feature requirements + master. Target: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/requirements.md`. Ensure GUARDs, feature index, cross-reference table are consistent with applied changes. Done when: master doc consistent, no stale references.
- [x] TASK 7: Write master architecture design doc using draft-design skill. Input requirements: master + all 3 feature requirements. Also read existing engram source at `src/synthesis/`, `src/consolidation.rs`, `src/entities.rs`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`. Content: architecture overview, cross-cutting concerns, data flow between features, shared types, feature index. Rule: ≤8 components. Done when: file written with §1-§5 per draft-design skill.
- [ ] TASK 8: Write compilation design doc using draft-design skill. Input requirements: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/requirements.md`. Input architecture: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/design.md`. Content: topic discovery, compilation pipeline, incremental triggers, merge/split, user feedback. Rule: ≤8 components. Done when: file written, every GOAL-comp.* traced. ⚠️ SKIPPED: hit max turns (60)
- [x] TASK 9: Write maintenance design doc using draft-design skill. Input requirements: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/requirements.md`. Input architecture: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/design.md`. Content: decay, conflict detection, health reports, export, CLI, API, privacy. Rule: ≤8 components. Done when: file written, every GOAL-maint.* traced.
- [x] TASK 10: Write platform design doc. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/design.md`. ✅ 752 lines, 6 components (§2.1-§2.6), all 16 GOAL-plat.* traced
- [x] TASK 11: Design review R1 — architecture. Use review-design skill (standard depth). Input: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/reviews/architecture-r1.md`. Done when: review file written with findings. ✅ 17 findings (3 critical, 8 important, 6 minor)
- [ ] TASK 12: Design review R1 — compilation. Use review-design skill (standard depth). Input: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/design.md`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/compilation/reviews/design-r1.md`.
- [ ] TASK 13: Design review R1 — maintenance. Use review-design skill (standard depth). Input: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/design.md`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/maintenance/reviews/design-r1.md`.
- [ ] TASK 14: Design review R1 — platform. Use review-design skill (standard depth). Input: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/design.md`. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/features/knowledge-compiler/platform/reviews/design-r1.md`.
- [ ] TASK 15: Apply design R1 findings to all 4 docs. Use apply-review skill on each design doc with its corresponding R1 review. Done when: all 4 design docs updated, R1 findings marked ✅.
- [ ] TASK 16: Design review R2 — architecture. Input: updated architecture.md. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/reviews/architecture-r2.md`.
- [ ] TASK 17: Design review R2 — compilation. Input: updated compilation/design.md. Output: compilation/reviews/design-r2.md.
- [ ] TASK 18: Design review R2 — maintenance. Input: updated maintenance/design.md. Output: maintenance/reviews/design-r2.md.
- [ ] TASK 19: Design review R2 — platform. Input: updated platform/design.md. Output: platform/reviews/design-r2.md.
- [ ] TASK 20: Apply design R2 findings. Use apply-review on all 4 docs. Done when: design docs finalized, no open Critical findings.
- [ ] TASK 21: Generate GID graph from all design docs using gid_design. Input: architecture.md + 3 feature design docs. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/graph.yml`. Done when: graph generated, gid_validate passes.
- [ ] TASK 22: Graph review. Run gid_validate, gid_advise. Check: every GOAL has satisfies task, DAG is valid, task granularity right, no orphans. Output: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/reviews/graph-r1.md`. Done when: graph clean, ready for implementation.
- [ ] TASK 23: Implementation. Use gid_tasks for topological order. Workspace: `/Users/potato/clawd/projects/engram-ai-rust/`. For each task: pick next ready, update status, implement in Rust, write tests, cargo test, gid_complete. Done when: all tasks done, cargo test passes.

---

## 决策记录

| 日期 | 决策 | 原因 |
|---|---|---|
| 2026-04-17 | Requirements 拆成 3 个 feature docs | 超过 15 GOAL 上限 |
| 2026-04-17 | Design review 两轮 | 确保设计质量，减少实现返工 |
| 2026-04-17 | Graph review 一轮 | task 分解质量直接影响实现效率 |
| 2026-04-17 | 写 architecture.md 在 feature designs 之前 | 先定边界再细化 |
| 2026-04-17 | Implementation 在 src/compiler/ | GUARD-1 要求 engram-native |
