# IronClaw-Engram Quick Start

## Installation

### Local Development

```bash
cd /Users/potato/clawd/projects/ironclaw-engram
cargo build --release
```

### Use in Your IronClaw Project

Add to `Cargo.toml`:

```toml
[dependencies]
ironclaw-engram = { path = "../ironclaw-engram" }
```

## Basic Usage

```rust
use ironclaw_engram::{Memory, MemoryType, MemoryConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create memory system (use preset for your agent type)
    let config = MemoryConfig::personal_assistant();
    let mut mem = Memory::new("./agent.db", Some(config))?;

    // Add a memory
    mem.add(
        "potato prefers action over discussion",
        MemoryType::Relational,
        Some(0.7),  // importance (0-1)
        None,       // source
        None,       // metadata
    )?;

    // Recall with ACT-R activation
    let results = mem.recall(
        "what does potato prefer?",
        5,      // limit
        None,   // context keywords
        None,   // min_confidence
    )?;

    for r in results {
        println!("[{}] {}", r.confidence_label, r.record.content);
    }

    // Consolidate (run "sleep" cycle)
    mem.consolidate(1.0)?;  // 1.0 = 1 day

    // Reward learning
    mem.reward("good job!", 3)?;

    Ok(())
}
```

## Run the Example

```bash
cargo run --release --example basic_usage
```

## Run Tests

```bash
cargo test --release
```

## Configuration Presets

Choose the preset that matches your agent's needs:

```rust
// Chatbot: slow decay, remember conversation history
let config = MemoryConfig::chatbot();

// Task agent: fast decay, focus on current task
let config = MemoryConfig::task_agent();

// Personal assistant: very slow core decay, remember for months
let config = MemoryConfig::personal_assistant();

// Researcher: minimal forgetting, everything might be relevant
let config = MemoryConfig::researcher();
```

## IronClaw Integration

See `IRONCLAW_INTEGRATION.md` for detailed integration patterns:

1. **Standalone crate** (recommended)
2. **Hybrid with IronClaw Workspace**
3. **Custom IronClaw tool**

### Quick IronClaw Agent Example

```rust
use ironclaw_engram::{Memory, MemoryConfig, MemoryType};

struct MyAgent {
    memory: Memory,
}

impl MyAgent {
    fn new(db_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config = MemoryConfig::personal_assistant();
        let memory = Memory::new(db_path, Some(config))?;
        Ok(Self { memory })
    }
    
    async fn process_message(&mut self, msg: &str) -> Result<String, Box<dyn std::error::Error>> {
        // 1. Recall relevant context
        let context = self.memory.recall(msg, 5, None, Some(0.3))?;
        
        // 2. Use context for LLM prompt...
        let response = self.generate_response(msg, &context).await?;
        
        // 3. Store interaction
        self.memory.add(
            &format!("User: {} | Response: {}", msg, response),
            MemoryType::Episodic,
            Some(0.4),
            Some("conversation"),
            None,
        )?;
        
        // 4. Periodic consolidation (every 10 messages)
        if self.message_count % 10 == 0 {
            self.memory.consolidate(0.1)?;
        }
        
        Ok(response)
    }
}
```

## API Reference

### Core Methods

- `Memory::new(path, config)` — Create or open database
- `mem.add(content, type, importance, source, metadata)` — Store a memory
- `mem.recall(query, limit, context, min_confidence)` — Retrieve with ACT-R
- `mem.consolidate(days)` — Run consolidation cycle
- `mem.forget(memory_id, threshold)` — Prune weak memories
- `mem.reward(feedback, recent_n)` — Apply dopaminergic feedback
- `mem.pin(memory_id)` / `mem.unpin(memory_id)` — Pin/unpin memories
- `mem.stats()` — Memory system statistics
- `mem.hebbian_links(memory_id)` — Get associative neighbors

### Memory Types

- `Factual` — Facts and world knowledge
- `Episodic` — Events and experiences
- `Relational` — Knowledge about people/entities
- `Emotional` — Emotionally significant memories
- `Procedural` — How-to knowledge
- `Opinion` — Subjective beliefs
- `Causal` — Cause-effect relationships

## Performance

- **Recall latency**: ~1-5ms for 10K memories
- **Memory footprint**: ~5MB for 10K memories
- **Consolidation**: ~100ms for 10K memories

## Documentation

- `README.md` — Full documentation with cognitive model details
- `IRONCLAW_INTEGRATION.md` — Integration guide with 3 approaches
- `PROJECT_SUMMARY.md` — Technical implementation summary

## Project Structure

```
/Users/potato/clawd/projects/ironclaw-engram/
├── src/
│   ├── lib.rs              # Public API
│   ├── memory.rs           # Memory struct and methods
│   ├── storage.rs          # SQLite backend
│   ├── types.rs            # Core data types
│   ├── config.rs           # Configuration presets
│   └── models/             # Cognitive models
│       ├── actr.rs         # ACT-R activation
│       ├── ebbinghaus.rs   # Forgetting curves
│       ├── consolidation.rs # Memory Chain Model
│       └── hebbian.rs      # Hebbian learning
├── examples/
│   └── basic_usage.rs      # Complete demo
└── tests/
    └── integration_test.rs # Integration tests
```

## License

Dual-licensed under MIT OR Apache-2.0.
