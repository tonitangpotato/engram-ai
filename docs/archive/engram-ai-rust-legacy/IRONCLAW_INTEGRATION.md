# IronClaw Integration Guide

This guide shows how to integrate IronClaw-Engram with [IronClaw](https://github.com/nearai/ironclaw), a Rust-based AI agent framework.

## Integration Approaches

### 1. Standalone Crate (Recommended)

Use IronClaw-Engram as a dependency in your IronClaw agent or tool:

```toml
[dependencies]
ironclaw = "0.16"
ironclaw-engram = "0.1"
```

```rust
use ironclaw_engram::{Memory, MemoryConfig, MemoryType};
use ironclaw::Agent;

struct MyAgent {
    memory: Memory,
    // ... other fields
}

impl MyAgent {
    fn new(db_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config = MemoryConfig::personal_assistant();
        let memory = Memory::new(db_path, Some(config))?;
        
        Ok(Self { memory })
    }
    
    async fn process_message(&mut self, msg: &str) -> Result<String, Box<dyn std::error::Error>> {
        // Recall relevant context
        let context = self.memory.recall(msg, 5, None, None)?;
        
        // Use context to inform response...
        let response = self.generate_response(msg, &context).await?;
        
        // Store the interaction
        self.memory.add(
            &format!("User: {} | Response: {}", msg, response),
            MemoryType::Episodic,
            Some(0.5),
            Some("conversation"),
            None,
        )?;
        
        Ok(response)
    }
    
    // Periodic consolidation (call in background task)
    async fn consolidate_memory(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.memory.consolidate(1.0)?;
        Ok(())
    }
}
```

### 2. IronClaw Workspace Integration

IronClaw has a built-in `Workspace` system for agent memory. You can use IronClaw-Engram alongside it:

```rust
use ironclaw::workspace::Workspace;
use ironclaw_engram::Memory;

struct HybridMemory {
    workspace: Workspace,  // IronClaw's file-based memory
    engram: Memory,        // Engram's cognitive model memory
}

impl HybridMemory {
    async fn recall(&mut self, query: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        // Combine results from both systems
        let workspace_results = self.workspace.search(query, 5).await?;
        let engram_results = self.engram.recall(query, 5, None, None)?;
        
        // Merge and deduplicate
        let mut combined = Vec::new();
        for r in engram_results {
            combined.push(r.record.content);
        }
        // ... merge logic
        
        Ok(combined)
    }
}
```

**When to use what:**

- **IronClaw Workspace**: Long-form documents, project notes, structured data
- **IronClaw-Engram**: Conversational memory, short-term context, ACT-R-based retrieval

### 3. Custom IronClaw Tool

Expose Engram as an IronClaw tool that agents can invoke:

```rust
use ironclaw::tools::{Tool, ToolResult};
use ironclaw_engram::Memory;
use serde_json::Value;

pub struct EngramTool {
    memory: Memory,
}

#[async_trait::async_trait]
impl Tool for EngramTool {
    async fn invoke(&mut self, params: Value) -> Result<ToolResult, Box<dyn std::error::Error>> {
        let action = params["action"].as_str().unwrap_or("recall");
        
        match action {
            "recall" => {
                let query = params["query"].as_str().unwrap_or("");
                let results = self.memory.recall(query, 5, None, None)?;
                
                let output = results
                    .into_iter()
                    .map(|r| format!("[{}] {}", r.confidence_label, r.record.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                
                Ok(ToolResult::success(output))
            }
            "add" => {
                let content = params["content"].as_str().unwrap_or("");
                let mem_type = params["type"].as_str().unwrap_or("factual");
                // ... parse and add
                Ok(ToolResult::success("Memory added"))
            }
            "consolidate" => {
                self.memory.consolidate(1.0)?;
                Ok(ToolResult::success("Consolidation complete"))
            }
            _ => Ok(ToolResult::error("Unknown action")),
        }
    }
}
```

## Configuration for IronClaw Agents

### Chatbot Agent

```rust
let config = MemoryConfig::chatbot();
let memory = Memory::new("./chatbot.db", Some(config))?;
```

- Slow decay → remember conversation history
- High replay → don't lose context

### Task Execution Agent

```rust
let config = MemoryConfig::task_agent();
let memory = Memory::new("./task_agent.db", Some(config))?;
```

- Fast decay → focus on current task
- Low replay → don't waste compute on old tasks

### Personal Assistant

```rust
let config = MemoryConfig::personal_assistant();
let memory = Memory::new("./assistant.db", Some(config))?;
```

- Very slow core decay → remember user preferences for months
- Balanced consolidation → transfer important memories to long-term

## Best Practices

### 1. Consolidation Schedule

Run consolidation periodically (not on every message):

```rust
// In your agent's background task
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await; // Every hour
        if let Err(e) = agent.consolidate_memory().await {
            log::error!("Consolidation failed: {}", e);
        }
    }
});
```

### 2. Reward Feedback

Use IronClaw's user feedback to strengthen/suppress memories:

```rust
async fn handle_feedback(&mut self, feedback: &str) {
    if let Err(e) = self.memory.reward(feedback, 3) {
        log::error!("Reward failed: {}", e);
    }
}
```

### 3. Hebbian Links

Leverage co-activation for related memories:

```rust
// After recalling multiple memories, Hebbian links form automatically
let results = self.memory.recall("potato's preferences", 5, None, None)?;

// Later, recalling one memory will boost linked memories via spreading activation
```

### 4. Pin Critical Memories

Pin system instructions or critical facts:

```rust
let id = self.memory.add(
    "Always use respectful language",
    MemoryType::Procedural,
    Some(1.0),
    Some("system"),
    None,
)?;
self.memory.pin(&id)?;
```

## Performance Considerations

### Memory Footprint

IronClaw-Engram is lightweight (~5MB for 10K memories), but consider:

- **SQLite file size**: ~1KB per memory entry
- **Access log**: Grows with every recall (prune periodically if needed)

### Recall Latency

- **FTS search**: ~1-5ms for 10K memories
- **ACT-R activation**: ~0.1ms per candidate
- **Total**: ~5-10ms for typical recall

### Consolidation Cost

- **Per-memory**: ~0.01ms
- **10K memories**: ~100ms total
- **Recommendation**: Run consolidation in background, not on hot path

## Example: Full IronClaw Agent with Engram

```rust
use ironclaw::{Agent, AgentConfig};
use ironclaw_engram::{Memory, MemoryConfig, MemoryType};

pub struct EngramAgent {
    memory: Memory,
    consolidation_counter: usize,
}

impl EngramAgent {
    pub fn new(db_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config = MemoryConfig::personal_assistant();
        let memory = Memory::new(db_path, Some(config))?;
        
        Ok(Self {
            memory,
            consolidation_counter: 0,
        })
    }
    
    pub async fn process(&mut self, input: &str) -> Result<String, Box<dyn std::error::Error>> {
        // 1. Recall relevant context
        let context = self.memory.recall(input, 5, None, Some(0.3))?;
        
        // 2. Generate response (use your LLM here)
        let context_str = context
            .iter()
            .map(|r| format!("- {}", r.record.content))
            .collect::<Vec<_>>()
            .join("\n");
        
        let response = format!(
            "Based on context:\n{}\n\nResponse to: {}",
            context_str, input
        );
        
        // 3. Store interaction
        self.memory.add(
            &format!("Q: {} | A: {}", input, response),
            MemoryType::Episodic,
            Some(0.4),
            Some("conversation"),
            None,
        )?;
        
        // 4. Periodic consolidation (every 10 messages)
        self.consolidation_counter += 1;
        if self.consolidation_counter >= 10 {
            self.memory.consolidate(0.1)?; // Simulated ~2.4 hours
            self.consolidation_counter = 0;
        }
        
        Ok(response)
    }
    
    pub fn stats(&self) -> Result<String, Box<dyn std::error::Error>> {
        let stats = self.memory.stats()?;
        Ok(format!(
            "Memories: {} | Pinned: {} | Uptime: {:.1}h",
            stats.total_memories, stats.pinned, stats.uptime_hours
        ))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut agent = EngramAgent::new("./agent.db")?;
    
    // Example conversation
    let response1 = agent.process("What do I prefer?").await?;
    println!("{}", response1);
    
    agent.memory.add(
        "User prefers action over discussion",
        MemoryType::Relational,
        Some(0.8),
        Some("preference"),
        None,
    )?;
    
    let response2 = agent.process("What do I prefer?").await?;
    println!("{}", response2);
    
    println!("{}", agent.stats()?);
    
    Ok(())
}
```

## Migration from IronClaw Workspace

If you're currently using IronClaw's workspace system, you can migrate to IronClaw-Engram:

```rust
async fn migrate_workspace_to_engram(
    workspace: &Workspace,
    memory: &mut Memory,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get all documents from workspace
    let docs = workspace.list_all().await?;
    
    for doc in docs {
        let content = workspace.read(&doc.path).await?;
        
        // Determine memory type from path/content
        let mem_type = if doc.path.starts_with("daily/") {
            MemoryType::Episodic
        } else if doc.path.contains("preference") {
            MemoryType::Relational
        } else {
            MemoryType::Factual
        };
        
        // Add to Engram
        memory.add(
            &content,
            mem_type,
            Some(0.5),
            Some(&doc.path),
            None,
        )?;
    }
    
    // Run initial consolidation
    memory.consolidate(1.0)?;
    
    Ok(())
}
```

## Troubleshooting

### High Memory Usage

- Run `mem.forget(None, Some(0.01))` periodically to prune weak memories
- Consider archiving old conversations to separate databases

### Slow Recall

- Reduce `limit` parameter in `recall()`
- Increase `min_confidence` to filter low-quality matches
- Consider caching recent query results

### Consolidation Overhead

- Reduce consolidation frequency
- Use smaller `dt_days` values for more frequent, lighter consolidation

## Resources

- [IronClaw-Engram Documentation](https://github.com/tonitangpotato/ironclaw-engram)
- [IronClaw Documentation](https://github.com/nearai/ironclaw)
- [ACT-R Theory](https://act-r.psy.cmu.edu/)
- [Memory Chain Model Paper](https://pubmed.ncbi.nlm.nih.gov/20663993/)
