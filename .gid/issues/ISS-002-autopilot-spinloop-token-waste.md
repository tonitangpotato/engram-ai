# ISS-002: Autopilot Spin-Loop — Agent Never Completes Write Tasks

## Status: OPEN

## Severity: Critical (resource waste — 150-200万 tokens burned with zero output)

## Summary

Autopilot's inner task loop sends "Continue working on: **{task}**" messages every turn, but writing-heavy tasks (design docs, reviews) require multiple tool calls (read → plan → write_file) within a single LLM request. Each "Continue" message creates a new `process_message` call, which the agent interprets as a fresh instruction — causing it to re-read input files and re-plan from scratch instead of continuing the write. After 60 turns the task is marked SKIPPED, having produced zero output.

## Observed Impact

Three tasks hit this bug on 2026-04-16/17:

| Task | Turns Used | Output | Tokens Wasted (est.) |
|------|-----------|--------|---------------------|
| TASK 8: Write compilation design doc | 60 (max) | Nothing | ~60-80万 |
| TASK 10: Write platform design doc | 60 (max) | Nothing | ~60-80万 |
| TASK 11: Design review R1 | Unknown (error) | Nothing | ~20-30万 |

**Total estimated waste: 150-200万 tokens, zero useful output.**

## Root Cause

The bug is in `autopilot.rs`, inner task loop (line ~230-280):

```rust
let msg = if task_turns == 0 {
    prompt.clone()          // First turn: full task prompt
} else {
    format!(                // Subsequent turns: "Continue" nudge
        "Continue working on: **{}**\nUpdate the checkbox when done.",
        task_desc
    )
};

match runner.process_message(&session_key, &msg, None, None).await {
    // ... check if checkbox updated, increment turn counter
}
```

### Why This Fails for Write Tasks

1. **Turn 1**: Agent receives full task prompt. Calls `read_file` on 2-3 input files (requirements.md, architecture.md, reference design.md). Files total ~40k chars. Agent plans the design.

2. **Turn 2**: Agent's tool calls complete, `process_message` returns the agent's response text. Autopilot checks if checkbox is updated → it isn't (agent only read files, didn't write yet). Loop continues.

3. **Turn 3**: Autopilot sends "Continue working on: **TASK 10: Write platform design doc...**". This is a NEW `process_message` call. The agent sees a new instruction in a conversation where the previous turns contain file-reading outputs. **The agent interprets this as "start over"** — re-reads the same files, re-plans the same design.

4. **Turns 4-60**: Same cycle repeats. Agent never reaches the `write_file` call because each turn's LLM budget is spent on re-reading and re-planning.

### The Fundamental Problem

Autopilot treats each turn as an independent message, but the agent's tool loop (read → analyze → write) needs **multiple tool calls within a single `process_message`**. The agent IS making progress within a turn (it reads files, plans), but that progress is invisible to autopilot because autopilot only checks one thing: "did the checkbox change?"

The "Continue" message doesn't help — it actively hurts by resetting the agent's focus.

### Why Simple Chat Tasks Don't Hit This

Simple tasks (e.g., "update a config value") complete in 1-2 tool calls within a single `process_message`. The agent reads a file, edits it, done. Autopilot's turn-by-turn loop works fine for these. The bug only manifests for tasks requiring:
- Multiple large file reads (>20k chars input)
- Significant synthesis/planning (design from requirements)
- Large file writes (>5k chars output)

## Contributing Factor: No Progress Detection

Autopilot has no way to detect "agent is making progress but hasn't finished yet." It only knows:
- Checkbox changed → done
- Max turns reached → skip
- Agent said "completed" → done
- Agent errored → skip

There's no signal for "agent read 3 files and is building context" vs "agent is stuck in a loop reading the same files." Both look identical: checkbox unchanged, keep sending "Continue."

## Proposed Fix

### Option A: Let the Tool Loop Finish (Minimal Change)

The simplest fix: **don't send "Continue" messages.** Let `process_message` run once with the full prompt. The agent's internal tool loop (controlled by `max_iterations` in AgentRunner) handles multi-step work within a single call.

```rust
// Current: inner loop sends messages per turn
// Proposed: single process_message call per task, rely on agent's tool loop

let response = runner.process_message(&session_key, &prompt, None, None).await?;
// Check checkbox after the full tool loop completes
```

**Caveat**: If the agent's internal tool loop hits its own max_iterations (e.g., 25), the task might not complete in one call. Need to distinguish "agent exhausted iterations" from "agent finished but didn't update checkbox."

### Option B: Progress-Aware Continuation

Keep the turn loop but add progress detection:

```rust
// Track what files the agent has read/written across turns
let mut files_written: HashSet<PathBuf> = HashSet::new();
let mut turns_since_progress = 0;

loop {
    let response = runner.process_message(&session_key, &msg, None, None).await?;
    
    // Parse tool calls from response to detect progress
    let new_writes = extract_file_writes(&response);
    if new_writes.is_empty() && files_written == prev_files_written {
        turns_since_progress += 1;
    } else {
        turns_since_progress = 0;
        files_written.extend(new_writes);
    }
    
    // Kill if stuck (no new writes for 3 turns)
    if turns_since_progress >= 3 {
        mark_task_skipped(&task_file, &task_desc, "no progress for 3 turns");
        break;
    }
}
```

### Option C: Sub-Agent Delegation (Design Pattern Fix)

For write-heavy tasks, autopilot should delegate to a sub-agent via `spawn_specialist` instead of driving the main agent in a loop. The sub-agent runs independently with its own tool loop budget.

This is what worked when the human caught the bug — I spawned sub-agents and they ran to completion without interruption.

```rust
// Heuristic: if task description contains "write", "design", "review"
// → delegate to sub-agent instead of turn-by-turn loop
if is_write_heavy_task(&task_desc) {
    let specialist_prompt = format!(
        "Execute this task completely:\n{}\nWrite all output files. Update checkbox when done.",
        task_desc
    );
    runner.spawn_specialist(&specialist_prompt, /* files */ &input_files).await?;
}
```

### Recommended: Option A + Option B Hybrid

1. **First attempt**: Single `process_message` with high `max_iterations` (50) — let the agent's tool loop handle everything
2. **If checkbox not updated after first attempt**: Send ONE "Continue" message with explicit context: "You read X files in the previous turn. Now write the output file."
3. **If still not done after 2nd attempt**: Mark as stuck with diagnostic info
4. **Max 3 attempts per task** (not 60!)

This caps waste at ~3× the cost of success instead of 60×.

## Verification

After fix:
1. Run autopilot on a task file containing a "write design doc" task
2. Verify: task completes in ≤3 process_message calls (not 60)
3. Verify: output file is actually written
4. Verify: token usage is proportional to task complexity (not 60× inflated)

## Workaround (Immediate)

For write-heavy tasks, the human (or agent in interactive mode) should use `spawn_specialist(wait=false)` instead of autopilot. This bypasses the broken turn loop entirely.

## History

- 2026-04-16: TASK 8 (compilation design doc) hit 60 max turns, zero output
- 2026-04-16: TASK 10 (platform design doc) hit 60 max turns, zero output  
- 2026-04-17: TASK 11 (architecture review) errored/stopped, zero output
- 2026-04-17: Bug identified by potato, root cause analyzed, ISS filed
