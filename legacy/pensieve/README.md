# ✨ Engram Pensieve

> *"I use the Pensieve. One simply siphons the excess thoughts from one's mind, pours them into the basin, and examines them at one's leisure."* — Albus Dumbledore

A visual explorer for Engram cognitive memory. See what your AI agent remembers, watch memories fade, and explore how thoughts connect.

## Quick Start

```bash
./run.sh
```

Then open http://localhost:5173

## Features

- **Force-directed memory graph** — each orb is a memory, connected by Hebbian associations
- **Visual decay** — fading memories become transparent, active ones glow bright
- **Color-coded types** — semantic (blue), episodic (green), causal (orange), procedural (purple)
- **Click to inspect** — see full content, recall history, linked memories
- **Search & filter** — find specific memories, filter by type
- **Real-time stats** — total memories, associations, recalls

## Architecture

```
SQLite DB → FastAPI (read-only) → React + D3.js force graph
```

## Configuration

Set `ENGRAM_DB` environment variable to point to your Engram database:

```bash
ENGRAM_DB=/path/to/your/engram.db ./run.sh
```
