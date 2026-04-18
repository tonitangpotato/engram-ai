# Design Review: Entity Indexing (R1)

**Target:** `.gid/features/entity-indexing/design.md`
**Reviewer:** Self (RustClaw)
**Date:** 2026-04-09
**Depth:** Full (Phase 0-7)

---

## 🔴 Critical (blocks implementation)

### FINDING-1: `EntityRole` (Subject/Object) 无法由 regex 可靠判断 ✅ Applied

**[Check #21]** ~~设计定义了 `EntityRole::Subject | Object | Mention`~~ → Removed EntityRole entirely. All links use "mention". Removed dead `span` field too.

---

### FINDING-2: `EntityExtractor` 缺少 `entity_config` 的传入机制 ✅ Applied

**[Check #22]** → Added §3.1.1 with full `EntityExtractor::new(config)` constructor. Uses `aho_corasick::AhoCorasick` for known entity lists (O(n) scan) + regex for structural patterns.

---

### FINDING-3: 权重之和 ≠ 1.0 → 旧 config 用户 silent 降级 ✅ Applied

**[Check #12, #15]** → Added runtime normalization: `total_weight = fts + emb + actr + entity; each /= total`. Any config combination now produces correct 0.0-1.0 scores.

---

## 🟡 Important (should fix before starting)

### FINDING-4: entities 表缺少 `(name, entity_type, namespace)` UNIQUE 约束 ✅ Applied

**[Check #6]** → Added `CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_unique ON entities(name, entity_type, namespace)` to §11 Migration.

---

### FINDING-5: `sha256(...)[..12]` 碰撞风险 + 12 个什么？ ✅ Applied

**[Check #9]** → Changed to `hex(sha256(...))[..16]` (16 hex chars = 64 bits). Explicitly stated "hex characters". ~4B entities before 50% collision.

---

### FINDING-6: Co-occurrence relation O(n²) 问题——§9 Edge Case #18 提到了 cap 但 §5 代码没有 ✅ Applied

**[Check #7]** → Added `.min(10)` cap in both §5 (add_raw) and §7 (backfill).

---

### FINDING-7: `find_entities()` prefix match + exact match 语义不明确 ✅ Applied

**[Check #21]** → Changed to exact match only (`WHERE name = ?`). Entity recall is for precise concept jumps; fuzzy is already covered by embedding search.

---

### FINDING-8: `EntityStore` trait 在 §2 提到但 §4 用 `Storage` 直接实现 ✅ Applied

**[Check #3]** → Removed EntityStore trait from §2. Methods added directly to Storage (consistent with §4).

---

## 🟢 Minor (can fix during implementation)

### FINDING-9: `updated_at` 在 upsert 时应该更新 ✅ Applied

**[Check #6]** → Added `ON CONFLICT(id) DO UPDATE SET updated_at = ?` to §11 Migration SQL.

---

### FINDING-10: `entity_relations` 的 `confidence` 和 `source` 字段未使用 ✅ Applied

**[Check #3]** → Added co-occurrence count → confidence mapping in §11: each repeated co-occurrence bumps confidence by 0.1, capped at 1.0.

---

### FINDING-11: `backfill_entities()` namespace 取法有问题 ✅ Applied

**[Check #6]** → Changed `get_memories_without_entities()` to return `Vec<(MemoryRecord, String)>` with namespace from SQL query.

---

### FINDING-12: 中文 entity 提取完全缺失 ✅ Applied

**[Check #8]** → Added to Non-Goals: "Chinese entity extraction — deferred to LLM extraction phase; known entity lists can cover specific Chinese terms."

---

### FINDING-13: `span: (usize, usize)` 当前设计未被使用 ✅ Applied

**[Check #3]** → Removed `span` field from `ExtractedEntity` (removed together with EntityRole in FINDING-1).

---

## ✅ Passed Checks

| Phase | Check | Status |
|---|---|---|
| 0 | Document size | ✅ 12 sections, well under limit |
| 1 | Type definitions | ✅ All types fully defined with field lists |
| 1 | Reference resolution | ✅ All internal references resolve |
| 1 | Naming consistency | ✅ Consistent naming throughout |
| 2 | Data flow | ✅ Entity extracted → stored → linked → recalled |
| 2 | Error handling | ✅ §5 says "log warning, don't fail add_raw()" |
| 4 | Separation of concerns | ✅ Extraction separate from storage separate from recall |
| 4 | Configuration | ✅ EntityConfig for all tunable values |
| 4 | API surface | ✅ Minimal public API (8 storage methods) |
| 5 | Goals/non-goals | ✅ Both explicit |
| 5 | Trade-offs | ✅ regex vs LLM trade-off documented |
| 5 | Abstraction level | ✅ Good mix of prose + pseudocode |
| 6 | Migration path | ✅ §11 confirms no migration needed |
| 6 | Testability | ✅ §9 comprehensive test plan |
| 7 | API compatibility | ✅ Additive-only changes, no breaking |
| 7 | Existing code alignment | ✅ Schema matches existing tables |

---

## 📊 Summary

| Severity | Count |
|---|---|
| 🔴 Critical | 3 |
| 🟡 Important | 5 |
| 🟢 Minor | 5 |
| ✅ Passed | 16 |

### Key Issues:
- **FINDING-1** (Critical): EntityRole Subject/Object can't be determined by regex → remove
- **FINDING-2** (Critical): EntityExtractor construction from config not specified → add Aho-Corasick
- **FINDING-3** (Critical): Weight sum > 1.0 for existing users → runtime normalization
- **FINDING-4** (Important): Missing UNIQUE index on entities table
- **FINDING-5** (Important): Hash truncation ambiguity + collision risk
- **FINDING-6** (Important): O(n²) cap mentioned in tests but missing from code

### Recommendation: **Needs fixes before implementation** — all 3 critical findings are easy to fix in the design doc. Once fixed, design is solid and implementation-ready.
