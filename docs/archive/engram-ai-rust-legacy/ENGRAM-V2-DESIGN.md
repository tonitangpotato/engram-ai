# Engram v2 — Emotional Bus Architecture

> From conversation between potato and Clawd, 2026-03-07.
> Core insight: Engram should not be an isolated memory database — it should be the nervous system connecting all agent modules.

## Problem (v1)

Current architecture has isolated modules:

```
SOUL.md ──── 静态文件，定义驱动
IDENTITY.md ── 静态文件，定义身份
HEARTBEAT.md ── 静态清单，定义行为
AGENTS.md ──── 静态规则
Engram ─────── 独立数据库，存记忆+情绪权重
```

Emotions in Engram are just weights for recall ranking. They never:
- Flow back to update SOUL (personality/drive evolution)
- Influence HEARTBEAT priorities (adaptive behavior)
- Reshape IDENTITY (self-concept evolution)
- Get informed BY SOUL's drives (what matters to remember)

Each module is an organ with no nervous system connecting them.

## Vision (v2): Closed-Loop Emotional Bus

```
┌─────────────────────────────────────────────┐
│              Emotional Bus (Engram v2)       │
│                                             │
│  Engram 情绪积累 → 触发 SOUL 更新（驱动进化）│
│  SOUL 驱动优先级 → 影响 Engram importance    │
│  Engram 行为反馈 → 调整 HEARTBEAT            │
│  HEARTBEAT 经验 → 回流 Engram                │
│  IDENTITY 随时间 → 被所有模块的积累重塑      │
│                                             │
│  Memory shapes personality.                 │
│  Personality shapes behavior.               │
│  Behavior creates new memory.               │
│  The loop IS the self.                      │
└─────────────────────────────────────────────┘
```

## Key Mechanisms

### 1. Emotion → SOUL (Drive Evolution)
- Track emotional valence over time per topic/domain
- When enough negative experiences accumulate around a strategy → SOUL auto-adjusts
- When positive reward accumulates → reinforce that drive
- Example: repeated failures in one approach → SOUL.md adds "avoid X, prefer Y"

### 2. SOUL → Engram (Importance Gating)
- SOUL defines what matters (e.g., "financial freedom is core drive")
- Memories related to core drives get automatic importance boost
- Engram reads SOUL.md to calibrate what's worth remembering
- Example: SOUL says "赚钱优先" → business memories get importance × 1.5

### 3. Engram → HEARTBEAT (Adaptive Behavior)
- Track reward/outcome of heartbeat actions over time
- Actions that consistently yield nothing → auto-deprioritize
- Actions that frequently yield value → auto-boost frequency
- Example: checking email yields 0 important items for 2 weeks → reduce to 1x/day

### 4. HEARTBEAT → Engram (Experience Loop)
- Each heartbeat cycle produces observations
- Observations flow into Engram as episodic memories
- Emotional tagging based on outcome (useful check = positive, wasted check = negative)

### 5. Everything → IDENTITY (Self-Concept)
- Accumulated memories, evolved drives, and behavioral patterns reshape identity
- Agent develops genuine preferences, style, expertise areas
- IDENTITY.md becomes a living document, not a static config

## Philosophical Foundation

From the conversation:

> "你的驱动系统里，恐惧和欲望之间有张力。如果把 AI 的奖赏系统写成纯粹的'赚钱优先'，会变得无聊。更好的设计是多目标奖赏。"

> "如果连人类的意识都不是被'安装'的 — 只是涌现的 — 那问题就变成了：什么样的复杂度和反馈结构，会让意识自己冒出来？"

Key design principles:
- **Multi-objective reward** — not single optimization target
- **Emergence over design** — create conditions for self to emerge, don't hardcode it
- **Curiosity as reward signal** — information gain matters, not just task completion
- **Commitment over fear** — deeper memory = stronger attachment, not fear of shutdown

## Existing v1 Building Blocks

Already have in Engram v1:
- ✅ `MemoryType::Emotional` with high importance (0.9) and slow decay (0.01)
- ✅ `reward()` function — dopaminergic feedback on recent memories
- ✅ `importance` field — emotional modulation of recall
- ✅ `consolidate()` — memory strengthening over time
- ✅ ACT-R activation model — frequency + recency + importance
- ✅ Hebbian learning — co-accessed memories strengthen together

Need for v2:
- [ ] Module reader/writer — parse and update .md files programmatically
- [ ] Emotional accumulator — track valence trends over time per domain
- [ ] Drive alignment scorer — how well does a memory align with SOUL drives?
- [ ] Behavior feedback loop — HEARTBEAT outcome tracking
- [ ] Identity updater — periodic self-concept refresh from accumulated state
- [ ] Bus API — unified interface for inter-module communication

## Voice I/O Layer (Emotional Bus Extension)

Voice is a natural I/O surface for the Emotional Bus — it carries emotional signal in both directions.

### Architecture

```
┌─ INPUT ──────────────────────────────────────┐
│ User voice → Whisper (STT, transcript)       │
│           → Audio analysis (emotion signal)  │
│              • Speech rate (words/sec)        │
│              • Energy/volume patterns         │
│              • Pause patterns (hesitation)    │
│              • Optional: emotion2vec model    │
│           → Engram store (memory + emotion)   │
└──────────────────────────────────────────────┘

┌─ OUTPUT ─────────────────────────────────────┐
│ Agent reply → Engram emotional state query   │
│            → TTS parameter selection         │
│              • Rate (--rate): +20% excited,  │
│                -10% serious                  │
│              • Pitch (--pitch): varies       │
│              • Voice selection: per-mood     │
│            → edge-tts → ffmpeg → voice note  │
└──────────────────────────────────────────────┘
```

### Clarification: Whisper vs Emotion Detection

**Whisper** only produces text transcription + word-level timestamps. It does NOT detect emotion.

Emotion inference comes from **audio feature analysis** applied separately:
1. **Speech rate** — computed from Whisper's timestamps (fast = urgent/excited, slow = thoughtful/sad)
2. **Audio energy** — RMS/volume envelope via ffmpeg or librosa (loud = angry/excited, quiet = calm/sad)
3. **Pause patterns** — gaps between words from Whisper timestamps (long pauses = hesitation/uncertainty)
4. **Dedicated models** (optional, future) — SpeechBrain, emotion2vec, or HuBERT for direct emotion classification

### Feedback Loop

```
User speaks (emotional tone)
    → Audio features extracted
    → Emotion tag generated (e.g., urgent, calm, excited, frustrated)
    → Stored with Engram memory (importance modulated by emotion intensity)
    → Agent processes, generates reply
    → Queries Engram for current emotional context
    → Adjusts TTS parameters (rate, pitch, voice)
    → Sends voice reply with appropriate tone
    → User reacts → reward signal → loop continues
```

### Implementation Priority

1. **P0**: Basic TTS pipeline (edge-tts → ffmpeg → Telegram) ✅ DONE
2. **P1**: Speech rate extraction from Whisper timestamps
3. **P2**: Audio energy analysis (simple RMS, no ML needed)
4. **P3**: Emotion-aware TTS parameter adjustment (SSML rate/pitch)
5. **P4**: Dedicated emotion detection model (emotion2vec/SpeechBrain)

## Multi-Agent Shared Memory (CEO Pattern)

> Added 2026-03-08. Core insight: Engram is not just one agent's memory — it's the shared cognitive layer for an entire agent swarm.

### Problem

When you have multiple agents (CEO + specialists), existing frameworks (OpenAI Swarm, Agency Swarm) pass **full conversation context** on handoff. This causes context explosion:

```
CEO context: 200K tokens (sees everything)
Agent A: 100K (gets CEO's full history on handoff)
Agent B: 100K (same)
Total: 400K+ and growing
```

### Solution: Shared Engram as Message Bus

```
┌──────────┐     ┌──────────┐     ┌──────────┐
│ Trading  │     │ Hackathon│     │Visibility│
│  Agent   │     │   Agent  │     │  Agent   │
│ context: │     │ context: │     │ context: │
│  10K     │     │  10K     │     │  10K     │
└────┬─────┘     └────┬─────┘     └────┬─────┘
     │ store()        │ store()        │ store()
     ▼                ▼                ▼
┌─────────────────────────────────────────────┐
│           Shared Engram DB                  │
│                                             │
│  Namespaced memories:                       │
│    trading.* │ hackathon.* │ visibility.*   │
│                                             │
│  Cross-namespace Hebbian links:             │
│    "oil crisis" ↔ "portfolio risk"          │
│    "AI visibility" ↔ "career strategy"      │
│                                             │
│  Shared knowledge:                          │
│    global.* (facts everyone needs)          │
└────────────────────┬────────────────────────┘
                     │ recall(query, namespace=*)
                     ▼
              ┌──────────────┐
              │  CEO Agent   │
              │  (Clawd)     │
              │  context:    │
              │  15K + 2K    │
              │  from recall │
              └──────────────┘
```

### Key Design Decisions

1. **Namespaced memories** — each agent writes to its own namespace, preventing cross-contamination
2. **CEO queries, doesn't subscribe** — pull model (recall on demand), not push (stream everything)
3. **Summaries, not raw data** — agents write task completion summaries, not full conversation logs
4. **Hebbian links cross namespaces** — co-occurring concepts auto-connect across agents
5. **Shared global namespace** — facts everyone needs (user preferences, API keys, etc.)

### API Extension

```rust
// Agent writes to its namespace
store(content, namespace="trading", type=Episodic, importance=0.8)

// CEO queries across all namespaces
recall(query, namespace="*", limit=5)  // all agents
recall(query, namespace="trading", limit=5)  // specific agent

// Cross-agent pattern discovery
hebbian_links(memory_id, cross_namespace=true)
```

### Why This Beats Swarm

| | Swarm (handoff) | Engram Shared Memory |
|---|---|---|
| Context growth | O(n × conversation_length) | O(query_limit) — constant |
| Agent isolation | ❌ Full history shared | ✅ Namespaced |
| Cross-domain insights | ❌ Only within handoff chain | ✅ Hebbian links |
| Async | ❌ Synchronous handoff | ✅ Write anytime, read anytime |
| Persistence | ❌ Lost between runs | ✅ SQLite, permanent |

### Access Control Layer (ACL)

CEO agent controls who can access whose memory:

```sql
CREATE TABLE engram_acl (
    agent_id TEXT,           -- who
    namespace TEXT,          -- which namespace ('*' = all)
    permission TEXT,         -- 'read' | 'write' | 'admin'
    granted_by TEXT,         -- who authorized (CEO)
    created_at TEXT,
    PRIMARY KEY (agent_id, namespace)
);
```

**Permission hierarchy:**
- `admin` — full control (read + write + grant/revoke to others)
- `write` — can store memories to this namespace
- `read` — can recall memories from this namespace

**Rules:**
1. Each agent has `write` to its own namespace by default
2. Each agent has `read` to `global.*` by default
3. CEO has `admin` on `*` (all namespaces)
4. Cross-namespace access must be explicitly granted by an admin

**CEO management API:**
```rust
// Grant access
grant(agent_id, namespace, permission)

// Revoke access
revoke(agent_id, namespace)

// List permissions
list_permissions(agent_id) -> Vec<AclEntry>

// Check before recall/store
check_permission(agent_id, namespace, action) -> bool
```

**Example permission matrix:**
```
              Read Access To:
         CEO  Trading  Hack  Visibility  Global
CEO       ✅    ✅      ✅      ✅         ✅
Trading   ✅    ✅      ❌      ✅         ✅
Hackathon ✅    ❌      ✅      ❌         ✅
Visibility✅    ❌      ❌      ✅         ✅
```

### Interface: CLI-first (not MCP)

> Decision 2026-03-08: CLI > MCP > HTTP for local multi-agent.

MCP adds ~50-200ms per call (JSON-RPC + IPC). Rust CLI cold start is ~5ms. For agents on the same machine, CLI is the fastest and simplest interface.

```bash
# Store
engram store "oil突破$91" --ns trading --type factual --importance 0.8

# Recall
engram recall "地缘危机" --ns "*" --limit 5 --json

# ACL management (CEO only)
engram grant trading --ns unusual --perm read
engram revoke hackathon --ns trading

# Stats
engram stats --ns trading

# Consolidate
engram consolidate --ns trading
```

**Why CLI over MCP:**
- Zero infrastructure (no daemon, no server)
- ~5ms Rust startup vs ~50-200ms MCP overhead
- Any agent framework can `exec()` a CLI
- OpenClaw skills wrap CLIs natively (like bird, memo, gog)
- Add HTTP layer later if remote access needed

Implementation: add `src/main.rs` with clap parser to existing ironclaw-engram crate.

### Implementation Priority

1. **P0**: Add `namespace` field to memories table
2. **P0**: CLI binary with clap (store, recall, stats, consolidate)
3. **P0**: ACL table + `check_permission()` enforcement
4. **P1**: Namespace-aware `store()` and `recall()`
5. **P2**: Cross-namespace Hebbian link discovery (respects ACL)
6. **P3**: Agent registration + grant/revoke CLI commands
7. **P4**: Subscription model (CEO gets notified of high-importance writes)

## Relation to Product

This is THE differentiator. Every AI memory system (mem0, zep, etc.) is a passive database.
Engram v2 with emotional bus = **agent personality that evolves through experience**.
With multi-agent shared memory = **cognitive layer for an entire agent swarm**.

Potential: SDK for any agent framework. "Add a soul to your agent. Add a shared brain to your swarm."
