# Requirements: L4 Rumination (自主思考)

> 日期: 2026-04-18
> 状态: draft
> 特性: 在无外部调用时，定时主动运行 synthesis engine，发现新关联、生成 insight

---

## 背景

当前 engram 的 synthesis engine 只在 `consolidate()` / `sleep_cycle()` 被调用时运行。这意味着只有 heartbeat 或用户触发的 consolidation 才会产生 insight。

Rumination（反刍）模拟人脑清醒时的后台思考：洗澡时突然想通一个问题、散步时把两件事联系起来。关键区别：

- **Consolidation** = 睡眠时记忆巩固（衰减、迁移、去重）— 已有
- **Rumination** = 清醒时主动思考（联想、发现、新 insight）— 本特性

## GOALs

### GOAL-1: 独立于 consolidation 的 synthesis 触发

engramai crate 暴露一个 `ruminate()` 公共方法，只运行 synthesis pipeline（cluster discovery → gate → insight generation），不运行 consolidation（不衰减、不迁移 layer）。调用者可以在任意时机触发 rumination 而不影响记忆强度。

**验收条件：**
- `Memory::ruminate()` 返回 `SynthesisReport`
- 调用 `ruminate()` 前后，所有记忆的 `working_strength` 和 `core_strength` 不变
- 调用 `ruminate()` 前后，所有记忆的 `layer` 不变

### GOAL-2: Rumination 结果写回 engram

`ruminate()` 生成的 insight 和 `sleep_cycle()` 的 insight 使用完全相同的存储路径：
- insight 作为 `is_synthesis: true` 的 MemoryRecord 写入
- provenance 记录源记忆引用
- 源记忆 importance 按 demotion_factor 降权

**验收条件：**
- `ruminate()` 产生的 insight 可以通过 `list_insights()` 查到
- `insight_sources()` 返回正确的 provenance chain
- 源记忆 importance 被降权

### GOAL-3: 与 consolidation 互斥

同一时刻不能同时运行 rumination 和 consolidation，因为两者都需要可变引用 `&mut Storage`。

**验收条件：**
- `ruminate()` 和 `consolidate()` 都需要 `&mut self`，Rust 借用规则自然保证互斥
- 不需要额外锁机制

### GOAL-4: 无 LLM 时优雅降级

与 synthesis engine 现有行为一致：没有 LLM provider 时，rumination 仍然运行 cluster discovery 和 gate check，但跳过 insight generation。

**验收条件：**
- 无 LLM provider 时，`ruminate()` 返回 report，`clusters_synthesized == 0`，无 error
- 有 LLM provider 时，正常生成 insight

### GOAL-5: RustClaw 集成 — 定时触发

RustClaw 的 `main.rs` 新增一个后台定时器，定期调用 `ruminate()`。间隔可配置，默认 2 小时。

**验收条件：**
- RustClaw 启动后，rumination 定时器自动启动
- 默认间隔 2 小时
- 日志记录每次 rumination 结果（clusters found/synthesized/skipped）
- rumination 失败不影响主进程

## GUARDs

### GUARD-1: 不重复 consolidation 的工作

`ruminate()` 绝不能触发记忆衰减、layer 迁移、Hebbian decay。这些是 consolidation 的职责。

### GUARD-2: Budget 控制

`ruminate()` 复用 `SynthesisSettings` 的 `max_llm_calls_per_run` 和 `max_insights_per_consolidation`，防止单次 rumination 消耗过多 LLM 调用。

### GUARD-3: 向后兼容

不改变 `consolidate()` / `sleep_cycle()` 的现有行为。现有测试不受影响。

## Non-Goals

- **不做定时策略的智能化**（比如"最近记忆多的时候更频繁"）— 这属于 L3 InteroceptiveHub 的职责
- **不做跨进程 rumination**（比如多个 agent 共享一个 DB 时的协调）— 超出范围
- **不改 synthesis engine 内部逻辑** — 复用现有 pipeline
