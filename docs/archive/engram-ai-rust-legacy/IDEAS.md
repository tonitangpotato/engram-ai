
## IDEA-20260418-01: KC 自动生成 Issue 文档
- **Date**: 2026-04-18
- **Source**: potato 讨论
- **Category**: engram / KC

### 想法
KC 聚类编译后的 insight，如果内容是"未解决的技术问题/改进方向"，可以自动生成 issue 草稿。

### 需要补的能力
1. **意图分类** — 判断 cluster 是讨论记录、技术决策、还是未解决问题
2. **输出格式映射** — CompiledKnowledge → issue 模板（描述、现状、目标、阶段、收益）
3. **去重检查** — 生成前检查 `.gid/issues/` 是否已有类似 issue
4. **触发时机** — consolidate 时自动？还是主动判断？

### 推荐方案
KC 编译 insight → 分类 → actionable item → 生成 issue 草稿 → **通知人确认**（不自动创建）

### Status: 💡 待详细讨论
---
