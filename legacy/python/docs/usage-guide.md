# neuromemory-ai Usage Guide

A comprehensive guide to using neuromemory-ai for building AI agents with cognitive memory.

## Quick Start

### Installation

```bash
pip install neuromemory-ai
```

### Basic Usage

```python
from engram import Memory

# Create memory system (stores in SQLite file)
mem = Memory("./agent.db")

# Add memories
mem.add("User's name is Alice", type="relational", importance=0.9)
mem.add("Alice prefers Python over JavaScript", type="relational", importance=0.6)
mem.add("Had a great conversation about AI ethics", type="episodic", importance=0.5)

# Recall memories
results = mem.recall("What programming language does the user like", limit=3)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")

# Run consolidation (do this periodically, like "sleep")
mem.consolidate()

# Check stats
print(mem.stats())
```

## Memory Types

neuromemory-ai supports different memory types, each with distinct characteristics:

| Type | Description | Decay Rate | Use Case |
|------|-------------|------------|----------|
| `factual` | Objective facts | Medium | "The capital of France is Paris" |
| `episodic` | Personal experiences | Fast | "Had lunch at noon today" |
| `relational` | Information about people/relationships | Slow | "User's name is Alice" |
| `emotional` | Emotionally significant events | Very slow | "User was upset about the bug" |
| `procedural` | How-to knowledge | Very slow | "To deploy: run `git push`" |
| `opinion` | Subjective views | Medium | "User thinks React is overrated" |

```python
# Use appropriate types for different information
mem.add("User's birthday is March 15", type="relational", importance=0.9)
mem.add("Helped debug async issue today", type="episodic", importance=0.5)
mem.add("SSH to server: ssh user@host", type="procedural", importance=0.7)
```

## Importance Scoring

Importance (0.0 - 1.0) affects how memories are prioritized:

```python
# Critical information - high importance
mem.add("User is allergic to peanuts", type="relational", importance=1.0)

# Nice to know - medium importance
mem.add("User likes hiking on weekends", type="relational", importance=0.5)

# Transient - low importance
mem.add("Weather was sunny today", type="episodic", importance=0.2)
```

High importance memories:
- Consolidate faster (move to long-term storage)
- Decay slower
- Rank higher in retrieval

## Configuration Presets

neuromemory-ai includes optimized presets for different agent types:

### Chatbot (Long Conversations)
```python
from engram.config import MemoryConfig

mem = Memory("./chatbot.db", config=MemoryConfig.chatbot())
```
- High replay ratio (remembers conversation context)
- Slow decay (retains information longer)
- Strong context matching

### Task Agent (Short-lived Tasks)
```python
mem = Memory("./task.db", config=MemoryConfig.task_agent())
```
- Fast decay (forgets completed tasks)
- Minimal replay
- Focus on recent procedural knowledge

### Personal Assistant (Long-term)
```python
mem = Memory("./assistant.db", config=MemoryConfig.personal_assistant())
```
- Very slow core decay (remembers for months)
- Importance-weighted (prioritizes user preferences)
- High trust in stored facts

### Researcher (Archive Everything)
```python
mem = Memory("./research.db", config=MemoryConfig.researcher())
```
- Minimal forgetting
- Heavy replay (all knowledge matters)
- Easy promotion to long-term storage

### Custom Configuration
```python
config = MemoryConfig(
    mu1=0.1,              # Working memory decay rate
    mu2=0.005,            # Core memory decay rate  
    alpha=0.1,            # Consolidation rate
    hebbian_enabled=True, # Enable Hebbian learning
    hebbian_threshold=3,  # Co-activations before link forms
    forget_threshold=0.01 # Minimum strength to retain
)
mem = Memory("./custom.db", config=config)
```

## Cognitive Features

### ACT-R Activation

Memories are retrieved based on activation, not just keyword matching:

```python
# Add memories at different times
mem.add("Project deadline is Friday", type="episodic", importance=0.8)
time.sleep(1)
mem.add("Weather is nice today", type="episodic", importance=0.2)

# Recent memories have higher base activation
results = mem.recall("deadline", limit=5)  
# Friday deadline ranks high due to recency + importance
```

Activation combines:
- **Base-level**: Recency and frequency of access
- **Spreading**: Context from current query
- **Importance**: Emotional/motivational weight

### Hebbian Learning

"Neurons that fire together wire together" - memories recalled together form links:

```python
# Enable Hebbian learning (default: enabled)
config = MemoryConfig(hebbian_enabled=True, hebbian_threshold=3)
mem = Memory("./agent.db", config=config)

# Add related memories
mem.add("Python is great for ML", type="factual")
mem.add("TensorFlow is a popular ML framework", type="factual")
mem.add("PyTorch is easier to debug", type="opinion")

# Query that retrieves them together
for _ in range(3):
    mem.recall("machine learning Python", limit=3)

# Now they're linked! Recalling one primes the others
results = mem.recall("TensorFlow", limit=5)
# PyTorch and Python memories also surface due to Hebbian links
```

### Consolidation (Sleep Cycles)

Run consolidation periodically to:
- Transfer working memory to long-term storage
- Replay important memories to strengthen them
- Decay unused memories

```python
# Run consolidation (simulates "sleep")
mem.consolidate(days=1)  # Simulate 1 day passing

# Check consolidation stats
stats = mem.stats()
print(f"Working memories: {stats['layers'].get('working', 0)}")
print(f"Core memories: {stats['layers'].get('core', 0)}")
```

### Forgetting

Adaptive forgetting improves retrieval quality:

```python
# Manually forget a specific memory
mem.forget(memory_id="abc123")

# Prune all weak memories below threshold
mem.forget(threshold=0.01)

# Pin important memories to prevent forgetting
mem.pin("critical_memory_id")
```

## CLI Usage

neuromemory-ai includes a CLI for quick testing:

```bash
# Add a memory
neuromem add "User prefers dark mode" --type relational --importance 0.7

# Recall memories
neuromem recall "user preferences"

# Show stats
neuromem stats

# Run consolidation
neuromem consolidate --days 1

# Export database
neuromem export ./backup.db
```

## Integration Patterns

### With LangChain

```python
from langchain.memory import BaseMemory
from engram import Memory

class NeuromemoryMemory(BaseMemory):
    def __init__(self, db_path: str):
        self.mem = Memory(db_path)
    
    @property
    def memory_variables(self) -> list[str]:
        return ["history"]
    
    def load_memory_variables(self, inputs: dict) -> dict:
        query = inputs.get("input", "")
        results = self.mem.recall(query, limit=5)
        history = "\n".join(r["content"] for r in results)
        return {"history": history}
    
    def save_context(self, inputs: dict, outputs: dict) -> None:
        self.mem.add(
            f"User: {inputs.get('input', '')}\nAssistant: {outputs.get('output', '')}",
            type="episodic"
        )
    
    def clear(self) -> None:
        pass  # Memories naturally decay
```

### With Raw OpenAI/Anthropic

```python
import openai
from engram import Memory

mem = Memory("./agent.db")

def chat(user_message: str) -> str:
    # Recall relevant context
    context = mem.recall(user_message, limit=5)
    context_str = "\n".join(f"- {r['content']}" for r in context)
    
    # Build prompt with memory
    messages = [
        {"role": "system", "content": f"You have these memories:\n{context_str}"},
        {"role": "user", "content": user_message}
    ]
    
    response = openai.chat.completions.create(
        model="gpt-4",
        messages=messages
    )
    
    assistant_message = response.choices[0].message.content
    
    # Store the interaction
    mem.add(f"User asked: {user_message}", type="episodic", importance=0.4)
    mem.add(f"I responded: {assistant_message[:100]}...", type="episodic", importance=0.3)
    
    return assistant_message
```

### With Clawdbot

neuromemory-ai includes a Clawdbot skill. Install it:

```bash
# Copy skill to Clawdbot skills directory
cp -r ~/.local/lib/python3.x/site-packages/engram/clawdbot-skill ~/.clawdbot/skills/neuromemory-ai
```

Then use in AGENTS.md or SOUL.md:
```markdown
## Memory
Use neuromemory-ai for persistent memory:
- `neuromem add "content" --type relational` to store
- `neuromem recall "query"` to retrieve
- `neuromem consolidate` during idle time
```

## MCP Server

Run neuromemory-ai as an MCP server for Claude Desktop or other MCP clients:

```bash
# Run MCP server
python -m engram.mcp_server

# Or with custom DB path
ENGRAM_DB_PATH=./my-agent.db python -m engram.mcp_server
```

Add to MCP config (Claude Desktop / Clawdbot):
```json
{
  "mcpServers": {
    "neuromemory": {
      "command": "python",
      "args": ["-m", "engram.mcp_server"],
      "env": {"ENGRAM_DB_PATH": "./agent.db"}
    }
  }
}
```

Available MCP tools:
- `engram.store` - Add a memory
- `engram.recall` - Retrieve memories
- `engram.consolidate` - Run consolidation
- `engram.forget` - Remove memories
- `engram.reward` - Apply feedback
- `engram.stats` - Get statistics
- `engram.hebbian_links` - Get Hebbian associations
- `engram.pin` / `engram.unpin` - Manage pinned memories

## Advanced Usage

### Custom Storage Backend

```python
from engram.stores.supabase import SupabaseStore

# Use Supabase for cloud storage
store = SupabaseStore(
    supabase_url="https://xxx.supabase.co",
    supabase_key="your-key",
    agent_id="my-agent"
)
mem = Memory(store=store)
```

### Multi-Agent Shared Memory

```python
# Each agent has an agent_id for isolation
agent1_mem = Memory("./shared.db", agent_id="agent-1")
agent2_mem = Memory("./shared.db", agent_id="agent-2")

# They share the same DB but have separate memory spaces
agent1_mem.add("I am Agent 1", type="factual")
agent2_mem.add("I am Agent 2", type="factual")

# Each only recalls their own memories
agent1_mem.recall("who am I")  # Returns "I am Agent 1"
```

### Reward Learning

Feedback strengthens/weakens recent memories:

```python
# User gives positive feedback
mem.reward("Great answer!")  # Strengthens recent memories

# User gives negative feedback  
mem.reward("That was wrong")  # Weakens recent memories

# The system detects sentiment automatically
```

### Exporting and Importing

```python
# Export to file
mem.export("./backup.db")

# Import from file (creates new Memory instance)
imported = Memory("./backup.db")
```

## Best Practices

1. **Choose appropriate memory types** - Procedural knowledge persists longer than episodic
2. **Set importance thoughtfully** - Critical info should be high importance
3. **Run consolidation regularly** - Like sleep, it helps memory health
4. **Use presets as starting points** - Then tune parameters for your use case
5. **Let Hebbian links form naturally** - Don't manually link, let co-activation work
6. **Don't fear forgetting** - It improves retrieval by reducing noise

## Troubleshooting

### Memories not being recalled
- Check if the query words match memory content (FTS5 uses keywords)
- Try increasing `limit` parameter
- Check if memory was pruned by forgetting (use `mem.stats()`)

### Too many irrelevant memories
- Increase `min_confidence` in recall
- Run consolidation to prune weak memories
- Lower importance of less relevant memories

### Hebbian links not forming
- Ensure `hebbian_enabled=True` in config
- Need at least `hebbian_threshold` co-activations (default: 3)
- Check with `mem.hebbian_links(memory_id)`
