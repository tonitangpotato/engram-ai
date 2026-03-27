# Engram Pensieve - Project Complete ✨

## What Was Built

A complete web-based visualization dashboard for Engram cognitive memory systems with:

### Backend (FastAPI)
- **API Server**: `/backend/main.py` with 4 REST endpoints
  - `GET /api/memories` - All memories with recall counts
  - `GET /api/links` - Hebbian association links
  - `GET /api/timeline` - Chronological event replay
  - `GET /api/stats` - System-wide statistics
- **Demo Database**: Auto-generates sample Engram data if no DB found
- **15 Sample Memories**: Realistic AutoAlpha, Engram, and development memories
- **CORS Enabled**: Works with React frontend
- **SQLite**: Direct database access, configurable via `ENGRAM_DB_PATH` env var

### Frontend (React + D3.js)
- **Force-Directed Graph**: D3.js physics simulation with:
  - Node size = working_strength + core_strength
  - Node color = memory_type (semantic/blue, episodic/green, causal/orange, procedural/purple)
  - Node opacity = working_strength (fading effect)
  - Pulse animations on recent recalls
  - Drag & drop interaction
  - Zoom & pan controls
  
- **Visual Components**:
  - `TopBar.jsx` - Title + search box
  - `ForceGraph.jsx` - Main D3 visualization with tooltips and legend
  - `SidePanel.jsx` - Memory details, stats, filters
  - `TimeSlider.jsx` - Timeline replay with play/pause controls

- **Features**:
  - Click nodes to inspect details
  - Hover for quick tooltips
  - Search to highlight memories
  - Filter by namespace and memory type
  - Timeline scrubber to watch memory formation
  - Auto-play timeline with visual recall flashes
  - Beautiful Pensieve aesthetic (dark theme, glowing orbs, purple gradients)

### Setup Files
- `run.sh` - One-command launcher for both servers
- `README.md` - Complete documentation
- `.gitignore` - Standard Python/Node patterns
- `requirements.txt` - FastAPI + Uvicorn
- `package.json` - React 18 + Vite + D3

## File Count
- **19 files** total (excluding dependencies)
- **Backend**: 1 Python file, 1 requirements.txt
- **Frontend**: 8 JSX components, 6 CSS files, 1 HTML, 2 config files
- **Docs**: 1 README, 1 run script

## Visual Theme
Dumbledore's Pensieve meets neural network:
- Background: `#0a0a1a` (deep space blue)
- Accent: Purple-blue gradients (`#9370db`, `#6495ed`)
- Text: Silver/white (`#e0e0f0`)
- Glowing nodes with blur effects
- Floating orb aesthetic

## Data Model (Engram Schema)
### memories table
- id, content, summary, memory_type, layer
- created_at, working_strength, core_strength
- importance, namespace, consolidation_count

### access_log table
- memory_id, accessed_at

### hebbian_links table  
- source_id, target_id, strength, coactivation_count

## How to Run
```bash
chmod +x run.sh
./run.sh
```

Then open: http://localhost:3000

Backend runs on: http://localhost:8000
API docs: http://localhost:8000/docs

## Demo Data Highlights
The auto-generated demo includes memories about:
- **AutoAlpha**: Causal trading system (84 nodes, Sharpe 1.27, 93 edges)
- **Engram**: Memory mechanics (decay, consolidation, Hebbian links)
- **Development**: D3.js, React, FastAPI, Pensieve design
- **15 memories** spanning 3 namespaces: autolpha, engram, development
- **15 Hebbian links** showing realistic association patterns
- **50+ recall events** for timeline replay

## Tech Decisions
- **No TypeScript**: Plain JSX for simplicity
- **Vite**: Fast dev server, HMR
- **D3.js force simulation**: Classic physics-based layout
- **FastAPI**: Modern Python, auto-generated API docs
- **SQLite**: Zero-config database
- **Mock data**: Works immediately without real Engram instance

## Status: ✅ COMPLETE

All requirements met:
- ✅ Force-directed graph with size/color/opacity mappings
- ✅ Hebbian links visualization
- ✅ Timeline replay with play/pause
- ✅ Side panel with stats, filters, detail view
- ✅ Search and filtering
- ✅ Dark Pensieve aesthetic
- ✅ Hover tooltips and click interactions
- ✅ Demo database with realistic data
- ✅ One-command launcher
- ✅ Complete documentation

## Next Steps (Future Enhancements)
- Connect to real Engram instance
- Export graph as image/video
- 3D force graph option
- Memory strength decay animation
- Consolidation visualization
- Multi-namespace comparison view
- Full-text search with fuzzy matching
- WebSocket for live updates
