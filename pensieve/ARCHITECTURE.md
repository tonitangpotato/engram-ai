# Engram Pensieve - Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     Browser (localhost:3000)                 │
│                                                              │
│  ┌────────────┐  ┌─────────────┐  ┌──────────────────────┐ │
│  │  TopBar    │  │ ForceGraph  │  │     SidePanel        │ │
│  │  Search    │  │ D3.js Graph │  │  Details + Stats     │ │
│  └────────────┘  └─────────────┘  └──────────────────────┘ │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │              TimeSlider                              │   │
│  │         Play/Pause + Scrubber                        │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
│                    React 18 + Vite                           │
└──────────────────────┬───────────────────────────────────────┘
                       │
                       │ HTTP/JSON
                       │
┌──────────────────────▼───────────────────────────────────────┐
│              FastAPI Server (localhost:8000)                 │
│                                                              │
│  GET /api/memories  ─┐                                       │
│  GET /api/links     ─┤                                       │
│  GET /api/timeline  ─┼─► SQLite Queries                     │
│  GET /api/stats     ─┘                                       │
│                                                              │
└──────────────────────┬───────────────────────────────────────┘
                       │
                       │ SQL
                       │
┌──────────────────────▼───────────────────────────────────────┐
│              SQLite Database (engram.db)                     │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │   memories   │  │ access_log   │  │hebbian_links │      │
│  ├──────────────┤  ├──────────────┤  ├──────────────┤      │
│  │ id           │  │ memory_id    │  │ source_id    │      │
│  │ content      │  │ accessed_at  │  │ target_id    │      │
│  │ memory_type  │  └──────────────┘  │ strength     │      │
│  │ strengths    │                    │ coactivation │      │
│  │ timestamps   │                    └──────────────┘      │
│  └──────────────┘                                           │
└──────────────────────────────────────────────────────────────┘
```

## Data Flow

### 1. App Initialization
```
App.jsx
  ├─> fetchData()
  ├─> GET /api/memories  → memories state
  ├─> GET /api/links     → links state
  ├─> GET /api/timeline  → timeline state
  └─> GET /api/stats     → stats state
```

### 2. Graph Rendering
```
ForceGraph.jsx
  ├─> receives memories + links
  ├─> D3.forceSimulation()
  │   ├─> forceLink (Hebbian connections)
  │   ├─> forceManyBody (repulsion)
  │   ├─> forceCenter (centering)
  │   └─> forceCollide (prevent overlap)
  └─> renders SVG with zoom/pan
```

### 3. User Interaction
```
User clicks node
  └─> onMemoryClick(memory)
      └─> setSelectedMemory(memory)
          └─> SidePanel updates with details

User searches
  └─> onSearch(query)
      └─> setSearchQuery(query)
          └─> ForceGraph highlights matches

User scrubs timeline
  └─> onTimeChange(timestamp)
      └─> setCurrentTime(timestamp)
          ├─> Filter visible memories
          └─> Trigger recall animations
```

## Component Hierarchy

```
App.jsx (root state management)
  │
  ├─> TopBar.jsx
  │   └─> Search input
  │
  ├─> Main Content (flex)
  │   ├─> ForceGraph.jsx
  │   │   ├─> SVG canvas
  │   │   ├─> D3 force simulation
  │   │   ├─> Node rendering
  │   │   ├─> Link rendering
  │   │   ├─> Tooltip (conditional)
  │   │   └─> Legend
  │   │
  │   └─> SidePanel.jsx
  │       ├─> Memory detail view
  │       ├─> Stats display
  │       │   ├─> Stat cards
  │       │   ├─> Type breakdown
  │       │   └─> Average strengths
  │       └─> Filters
  │           ├─> Namespace dropdown
  │           └─> Type checkboxes
  │
  └─> TimeSlider.jsx
      ├─> Event info display
      ├─> Play/pause button
      ├─> Range slider
      └─> Event counter
```

## Backend Architecture

### FastAPI Routes

```python
main.py
  │
  ├─> @app.get("/")
  │   └─> Health check / info
  │
  ├─> @app.get("/api/memories")
  │   ├─> JOIN memories + access_log
  │   ├─> COUNT recalls per memory
  │   └─> Return JSON array
  │
  ├─> @app.get("/api/links")
  │   └─> SELECT * FROM hebbian_links
  │
  ├─> @app.get("/api/timeline")
  │   ├─> SELECT created_at → "created" events
  │   ├─> UNION accessed_at → "recalled" events
  │   └─> ORDER BY timestamp ASC
  │
  └─> @app.get("/api/stats")
      ├─> COUNT total memories
      ├─> COUNT total recalls
      ├─> GROUP BY memory_type
      ├─> GROUP BY namespace
      ├─> AVG strengths
      └─> Return summary object
```

### Database Initialization

```python
get_db()
  ├─> Check if DB exists
  ├─> If not → create_demo_database()
  │   ├─> CREATE TABLE memories
  │   ├─> CREATE TABLE access_log
  │   ├─> CREATE TABLE hebbian_links
  │   ├─> INSERT 15 sample memories
  │   ├─> INSERT ~50 access events
  │   └─> INSERT 15 Hebbian links
  └─> Return connection
```

## State Management (React)

```javascript
App.jsx state:
  - memories: Memory[]           // All memories from DB
  - links: HebbianLink[]         // All association links
  - timeline: TimelineEvent[]    // All create/recall events
  - stats: Stats                 // Summary statistics
  - selectedMemory: Memory|null  // Currently selected node
  - searchQuery: string          // Search filter text
  - filters: FilterState         // Namespace + type filters
  - currentTime: number          // Timeline slider position
  - recentRecalls: Set<id>       // Recently recalled (for pulse)
```

## Visual Mappings

### Node Properties
```
size = (working_strength + core_strength) * 20
  → Larger nodes = more active memories

color = MEMORY_TYPE_COLORS[memory_type]
  → semantic: #4169e1 (blue)
  → episodic: #32cd32 (green)
  → causal:   #ff8c00 (orange)
  → procedural: #9370db (purple)

opacity = max(0.3, working_strength)
  → Fading nodes = decaying memories

stroke = highlighted ? white : gold : none
  → Selected or search-matched
```

### Link Properties
```
thickness = strength * 3
  → Thicker lines = stronger associations

opacity = min(0.8, coactivation_count / 20)
  → More transparent = less co-activation
```

### Timeline Events
```
"created" event
  → Node appears in graph
  → Green badge in slider

"recalled" event  
  → Node pulses (scale 1.5x for 0.5s)
  → Blue badge in slider
  → Added to recentRecalls set
```

## Animation System

### D3 Force Simulation
```
Runs at ~60fps
Each tick updates:
  - Node positions (x, y)
  - Link positions (x1, y1, x2, y2)
  - Collision detection
  - Centering forces
```

### React Animations
```
CSS transitions:
  - Node hover brightness
  - Stroke changes on selection
  - Button hover effects
  - Slider thumb scale

D3 transitions:
  - Pulse on recall (500ms)
  - Return to normal (500ms)
```

### Timeline Playback
```
setInterval(200ms)
  ├─> Advance slider +1 event
  ├─> Update currentTime
  ├─> Trigger recall animations
  └─> Stop at end
```

## Performance Considerations

- **Force simulation**: Runs in Web Worker (D3 handles this)
- **React re-renders**: Minimized via useEffect dependencies
- **Large datasets**: Force simulation scales to ~1000 nodes before slowdown
- **Timeline**: Efficient filtering via timestamp comparisons
- **Tooltips**: Conditional rendering (only when hovering)

## File Organization

```
pensieve/
├── backend/
│   ├── main.py              # FastAPI app + demo DB generator
│   ├── requirements.txt     # Python dependencies
│   └── venv/               # Virtual environment
│
├── frontend/
│   ├── src/
│   │   ├── App.jsx          # Root component + state
│   │   ├── App.css          # Global styles
│   │   ├── components/
│   │   │   ├── ForceGraph.jsx + .css    # D3 visualization
│   │   │   ├── SidePanel.jsx + .css     # Details + stats
│   │   │   ├── TimeSlider.jsx + .css    # Timeline controls
│   │   │   └── TopBar.jsx + .css        # Search + title
│   │   ├── main.jsx         # React entry point
│   │   └── index.css        # Base styles
│   ├── index.html           # HTML template
│   ├── package.json         # Node dependencies
│   └── vite.config.js       # Vite config
│
├── run.sh                   # Launch script
├── README.md                # Full documentation
├── QUICKSTART.md            # Quick start guide
├── ARCHITECTURE.md          # This file
└── PROJECT_SUMMARY.md       # Project overview
```

## Technology Choices

| Layer | Tech | Why |
|-------|------|-----|
| Backend | FastAPI | Modern, fast, auto-docs |
| Server | Uvicorn | ASGI, Python 3.7+ |
| Database | SQLite | Zero-config, embedded |
| Frontend | React 18 | Component model, hooks |
| Build | Vite | Fast HMR, modern |
| Viz | D3.js | Industry standard for force graphs |
| Language | JavaScript | Simplicity (no TypeScript overhead) |
| Styling | CSS | Custom styling, full control |

## Extension Points

Want to enhance Pensieve? Key places to hook in:

1. **New visualizations**: Add to `ForceGraph.jsx` or create new component
2. **More stats**: Add endpoints in `backend/main.py`
3. **Live updates**: WebSocket in FastAPI + React
4. **3D graph**: Replace D3 force with Three.js + force-graph-3d
5. **Export**: Add button → canvas.toBlob() → download
6. **Themes**: Add CSS variables for color schemes
7. **Memory editing**: POST endpoints + forms in SidePanel
8. **Agent connection**: Replace demo DB with live Engram instance

---

Built with ✨ for the Engram cognitive memory system.
