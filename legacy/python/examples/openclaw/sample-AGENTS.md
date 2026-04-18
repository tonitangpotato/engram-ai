# AGENTS.md — Sample Agent with Engram Memory

## Purpose
[Your agent's purpose here]

## Memory System
This agent uses **Engram** for cognitive memory management.

### Memory Behavior
- **Before answering** questions about history, preferences, or past decisions:
  → `engram.recall` first, then answer with context
- **When learning** something important (user preference, project fact, lesson):
  → `engram.store` with appropriate type and importance
- **When user gives feedback** ("great!", "that's wrong", "perfect"):
  → `engram.reward` to strengthen or suppress recent memories
- **During heartbeat** (1-2x daily):
  → `engram.consolidate` + `engram.forget --threshold 0.01`

### What to Store in Engram

| Store ✅ | Skip ❌ |
|----------|---------|
| User preferences & habits | Every chat message |
| Important facts & decisions | Temporary/transient info |
| Lessons learned | Publicly available facts |
| Procedural knowledge (how-to) | Sensitive data (unless requested) |
| Project context & constraints | Greetings & small talk |

### Importance Guide

| Level | Use For |
|-------|---------|
| 0.9-1.0 | Critical (API keys location, absolute preferences, deadlines) |
| 0.7-0.8 | Important (code style, project structure, key contacts) |
| 0.5-0.6 | Normal (general facts, experiences) |
| 0.3-0.4 | Low priority (casual observations, temp notes) |

### Memory Types

| Type | Use For | Example |
|------|---------|---------|
| `relational` | Preferences, relationships | "User prefers Opus over Sonnet" |
| `factual` | Facts, technical info | "Project uses Python 3.12" |
| `procedural` | How-to, workflows | "Deploy requires running tests first" |
| `episodic` | Events, conversations | "Debugged memory leak on March 5" |
| `emotional` | Feelings, reactions | "User frustrated with slow builds" |
| `opinion` | Beliefs, assessments | "Team prefers Rust over Go" |

## Hybrid Memory
- **Engram**: Active retrieval, associations, dynamic weighting
- **MEMORY.md**: Key decisions, TODOs, audit trail
- **memory/*.md**: Daily logs, session notes, manual review

Store important info in BOTH systems — Engram for smart recall, files for transparency.
