# Design: L4 Rumination (自主思考)

> 日期: 2026-04-18
> 依赖: requirements.md
> 状态: draft

---

## 1. Overview

Rumination 是 synthesis-only 的触发路径。代码量极小（~50 行），因为所有零件已在 synthesis engine 中就绪，只需绕过 consolidation 直接调用 synthesize。

**关键设计决策：** `ruminate()` = `synthesize()` 的公共包装，不经过 `consolidate_namespace()`。

## 2. 架构

```
┌─────────────────────────────────────────────────┐
│                  Memory                         │
│                                                 │
│  consolidate()  ──→ consolidate_namespace()     │
│                     ├── decay + migrate         │
│                     └── synthesize() ←──────┐   │
│                                             │   │
│  sleep_cycle()  ──→ consolidate_namespace() │   │
│                     + synthesize()          │   │
│                                             │   │
│  ruminate() NEW ────────────────────────────→┘   │
│    (直接调 synthesize, 跳过 decay/migrate)       │
└─────────────────────────────────────────────────┘
```

## 3. Components

### 3.1 `Memory::ruminate()` — engramai crate 侧

**文件:** `src/memory.rs`

**签名:**
```rust
pub fn ruminate(&mut self) -> Result<SynthesisReport, Box<dyn std::error::Error>>
```

**逻辑:**
1. 检查 `self.synthesis_settings` 是否启用，未启用则返回空 report
2. 调用已有的 `self.synthesize()` 内部方法
3. 返回 `SynthesisReport`

**不做的事:** 不调用 `consolidate_namespace()`、不衰减 Hebbian links、不迁移 layer。

这等于 `sleep_cycle()` 去掉 Phase 1（consolidation），只保留 Phase 2（synthesis）。

### 3.2 RustClaw 定时器 — RustClaw 侧

**文件:** `rustclaw/src/main.rs`（或 `rustclaw/src/memory.rs`）

在现有 auto-consolidation spawn 旁边新增一个 spawn：

```rust
// Start rumination background task (every 2 hours)
let mem_for_rumination = shared_memory.clone();
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(2 * 3600));
    interval.tick().await; // skip first immediate tick
    loop {
        interval.tick().await;
        match mem_for_rumination.ruminate() {
            Ok(report) => {
                if report.clusters_found > 0 {
                    tracing::info!(
                        "Rumination: {} clusters, {} synthesized, {} skipped",
                        report.clusters_found,
                        report.clusters_synthesized,
                        report.clusters_skipped,
                    );
                }
            }
            Err(e) => tracing::warn!("Rumination failed: {}", e),
        }
    }
});
```

### 3.3 `SharedMemory::ruminate()` — RustClaw memory wrapper

**文件:** `rustclaw/src/memory.rs`

```rust
pub fn ruminate(&self) -> anyhow::Result<SynthesisReport> {
    let mut engram = self.engram.lock()
        .map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
    let report = engram.ruminate()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(report)
}
```

## 4. Data Flow

```
Timer tick (every 2h)
  → SharedMemory::ruminate()
    → Mutex lock engram
      → Memory::ruminate()
        → DefaultSynthesisEngine::synthesize()
          → cluster::discover_clusters()
          → gate::check_gate()
          → insight::call_llm() (if gate passes + LLM available)
          → store insight + provenance + demote sources
        → return SynthesisReport
    → release lock
  → log results
```

## 5. Guard Checks

| Guard | 实现方式 |
|-------|---------|
| GUARD-1 不重复 consolidation | `ruminate()` 不调用 `consolidate_namespace()`，只调用 `synthesize()` |
| GUARD-2 Budget 控制 | 复用 `SynthesisSettings.max_llm_calls_per_run` |
| GUARD-3 向后兼容 | `consolidate()` / `sleep_cycle()` 代码不变 |

## 6. Testing

### 6.1 Unit Test: `ruminate()` 不改变 memory strength

```rust
#[test]
fn test_ruminate_does_not_decay() {
    // 1. 创建 Memory, 存几条记忆, 记录初始 strength
    // 2. 调用 ruminate()
    // 3. 断言所有记忆 working_strength 和 core_strength 不变
}
```

### 6.2 Unit Test: `ruminate()` 无 LLM 时优雅降级

```rust
#[test]
fn test_ruminate_no_llm_graceful() {
    // 1. 创建 Memory, 不设 LLM provider
    // 2. 调用 ruminate()
    // 3. 断言 report.clusters_synthesized == 0, 无 error
}
```

### 6.3 Unit Test: `ruminate()` 生成 insight

```rust
#[test]
fn test_ruminate_creates_insight() {
    // 1. 创建 Memory, 存 3 条相关记忆, 建 Hebbian links
    // 2. 设 Mock LLM provider
    // 3. 调用 ruminate()
    // 4. 断言 list_insights() 能查到新 insight
}
```
