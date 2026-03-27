# Engram Pensieve - Quick Start ⚡

## TL;DR

```bash
cd /Users/potato/clawd/projects/agent-memory-prototype/pensieve
./run.sh
```

Open http://localhost:3000 in your browser. Done!

---

## What You'll See

🔮 **A beautiful force-directed graph** where each glowing orb is a memory from Engram's cognitive system.

### Visual Guide

**Node Size** = How active the memory is (working + core strength)
**Node Color** = Memory type:
- 🔵 Blue = Semantic (facts, knowledge)
- 🟢 Green = Episodic (events, experiences)  
- 🟠 Orange = Causal (cause-effect relationships)
- 🟣 Purple = Procedural (how-to knowledge)

**Node Opacity** = Working memory strength (fading = decaying)
**Lines** = Hebbian associations (thicker = stronger link)

### Interactions

- **Click** a node → See detailed memory info in right panel
- **Drag** a node → Rearrange the graph
- **Hover** → Quick tooltip with memory content
- **Search** → Highlight matching memories (top bar)
- **Filter** → Show/hide by type or namespace (right panel)
- **Timeline slider** → Scrub through memory formation history
- **Play button** → Auto-replay with recall animations

---

## Demo Data

The app auto-generates 15 sample memories about:
- AutoAlpha causal trading system
- Engram memory architecture  
- D3.js visualization development

These memories have realistic:
- Decay patterns (working strength fading over time)
- Consolidation (core strength increasing)
- Association links (memories that co-activate)
- Recall history (50+ events)

---

## Using Your Own Engram Database

```bash
export ENGRAM_DB_PATH=/path/to/your/engram.db
./run.sh
```

The backend expects these SQLite tables:
- `memories` (id, content, summary, memory_type, layer, strengths, timestamps...)
- `access_log` (memory_id, accessed_at)
- `hebbian_links` (source_id, target_id, strength, coactivation_count)

See `README.md` for full schema details.

---

## Troubleshooting

**Port already in use?**
```bash
# Backend (default 8000)
PORT=8080 python backend/main.py

# Frontend (edit vite.config.js to change from 3000)
```

**Dependencies not installed?**
```bash
# Backend
cd backend
source venv/bin/activate
pip install -r requirements.txt

# Frontend  
cd frontend
npm install
```

**Want to rebuild?**
```bash
cd frontend
npm run build
```

---

## Manual Start (without run.sh)

**Terminal 1 - Backend:**
```bash
cd backend
source venv/bin/activate
python main.py
```

**Terminal 2 - Frontend:**
```bash
cd frontend
npm run dev
```

---

## What Makes It Beautiful

✨ **Pensieve Aesthetic**
- Deep space background (#0a0a1a)
- Glowing orbs with soft blur
- Purple-blue gradient theme
- Silver/white UI elements
- Memories look like luminous thought-spheres

🎯 **Design Principles**
- Visual impact > feature count
- Every visual element maps to data
- Smooth animations and transitions
- Hover states reveal details progressively
- Color coding for instant categorization

---

## Next Steps

1. **Explore** - Click around, drag nodes, watch the physics
2. **Search** - Type keywords to highlight matching memories
3. **Filter** - Toggle memory types to see patterns
4. **Timeline** - Hit play and watch memories form over time
5. **Connect Real Data** - Point to your Engram DB and visualize actual agent memory

---

Enjoy your Pensieve! ✨

Questions? See `README.md` for full documentation.
