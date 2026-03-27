# Engram Pensieve - Visual Guide 🎨

## What It Looks Like

Imagine opening the app and seeing...

### The Main View

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃                                                                    ┃
┃  Engram Pensieve ✨           [Search memories...        ] 🔍     ┃
┃  NEURAL MEMORY VISUALIZATION                                       ┃
┃                                                                    ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃                                     ┃  SEMANTIC                   ┃
┃         ○                           ┃  AutoAlpha uses causal...  ┃
┃      ◉─────◉                        ┃                            ┃
┃        │ ╱                          ┃  Working Strength          ┃
┃    ○───◉                            ┃  ████████░░ 80%            ┃
┃       ╱│╲                           ┃                            ┃
┃      ○ │ ◉──○                       ┃  Core Strength             ┃
┃        │    │                       ┃  ██████████ 90%            ┃
┃        ○────◉                       ┃                            ┃
┃                                     ┃  Created: Mar 6, 10:30am   ┃
┃    Legend:                          ┃  Recalls: 12               ┃
┃    🔵 semantic                      ┃                            ┃
┃    🟢 episodic                      ┃  ─────────────────────     ┃
┃    🟠 causal                        ┃                            ┃
┃    🟣 procedural                    ┃  STATISTICS                ┃
┃                                     ┃                            ┃
┃                                     ┃  15 Total Memories         ┃
┃                                     ┃  52 Total Recalls          ┃
┃                                     ┃                            ┃
┃                                     ┃  Type Distribution         ┃
┃                                     ┃  semantic    █████ 5       ┃
┃                                     ┃  episodic    ████ 4        ┃
┃                                     ┃  causal      ████ 4        ┃
┃                                     ┃  procedural  ██ 2          ┃
┃                                     ┃                            ┃
┃                                     ┃  FILTERS                   ┃
┃                                     ┃  Namespace: [All ▾]        ┃
┃                                     ┃  ☑ semantic                ┃
┃                                     ┃  ☑ episodic                ┃
┃                                     ┃  ☑ causal                  ┃
┃                                     ┃  ☑ procedural              ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┻━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃                                                                    ┃
┃  ⏱️ Timeline Replay      🌟 Created  3/6/26 10:30am               ┃
┃                                                                    ┃
┃  ▶️  ──────────●────────────────────────────────────  ⏭️  15/52   ┃
┃                                                                    ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

## Color Palette

### Background Gradient
```
Dark Space:    #0a0a1a → #0f0f28
Radial glow from center, creating depth
```

### Accent Colors
```
Primary Purple:   #9370db (medium purple)
Secondary Blue:   #6495ed (cornflower blue)
Gradient bars use both
```

### Memory Type Colors
```
🔵 Semantic:    #4169e1 (royal blue)      - Facts, knowledge
🟢 Episodic:    #32cd32 (lime green)      - Events, experiences
🟠 Causal:      #ff8c00 (dark orange)     - Cause-effect
🟣 Procedural:  #9370db (medium purple)   - How-to knowledge
```

### UI Elements
```
Text (primary):     #e0e0f0 (silver-white)
Text (secondary):   #a0a0c0 (muted purple-gray)
Text (accent):      #9370db (purple)
Borders:            rgba(147, 112, 219, 0.3) (translucent purple)
Panel backgrounds:  rgba(15, 15, 40, 0.9) (semi-transparent dark)
```

## Node Behavior Examples

### Highly Active Memory (Working: 0.9, Core: 0.8)
```
    ✨
  ╱   ╲
 │  ●  │  ← Large, bright, fully opaque
  ╲   ╱    Color based on type
    ✨     Glowing effect (CSS filter: blur)
```

### Decaying Memory (Working: 0.2, Core: 0.7)
```
    ·
   │○│    ← Medium size, faded opacity
    ·      Still has core strength
```

### Recent Recall (Pulse Animation)
```
Frame 1:  ●
Frame 2:  ◉     ← Scale 1.0 → 1.5
Frame 3:  ○
Frame 4:  ◉
Frame 5:  ●     ← Back to 1.0
Duration: 1 second
```

### Hovered Node
```
  ┌────────────────────────┐
  │ SEMANTIC               │ ← Tooltip appears
  │ AutoAlpha uses...      │
  │ 💪 Working: 85%        │
  │ 🧠 Core: 90%           │
  │ 📊 Recalls: 12         │
  │ ⭐ Importance: 95%     │
  └────────────────────────┘
       │
       ▼
      ◉─  ← Brightened, cursor: pointer
```

### Selected Node
```
      ◎  ← White stroke (3px)
     ╱│╲   Details shown in side panel
```

### Search Match
```
      ◎  ← Gold stroke (2px)
     ╱│╲   Highlighted while search active
```

## Link Rendering

### Strong Association (Strength: 0.9, Coactivations: 15)
```
  ●━━━━━━━●  ← Thick line (3px × 0.9 = 2.7px)
            ← High opacity (min(0.8, 15/20) = 0.75)
            ← Blue gradient (#6495ed)
```

### Weak Association (Strength: 0.3, Coactivations: 3)
```
  ●─ ─ ─ ─●  ← Thin line (3px × 0.3 = 0.9px)
            ← Low opacity (3/20 = 0.15)
            ← Barely visible
```

## Side Panel States

### No Selection
```
┌─────────────────────────┐
│                         │
│         ✨              │
│                         │
│  Click a memory to      │
│  view details           │
│                         │
└─────────────────────────┘
```

### Memory Selected
```
┌─────────────────────────┐
│ SEMANTIC   [autolpha]   │ ← Type + namespace badge
│                         │
│ Memory Content          │
│ AutoAlpha uses causal   │
│ inference for trading   │
│ strategies              │
│                         │
│ Summary                 │
│ Causal trading system   │
│                         │
│ Working Strength        │
│ ████████░░ 80%          │ ← Animated bar (blue gradient)
│                         │
│ Core Strength           │
│ ██████████ 90%          │ ← Animated bar (purple gradient)
│                         │
│ Importance              │
│ █████████░ 95%          │ ← Animated bar (orange gradient)
│                         │
│ Created: Mar 6, 10:30am │
│ Layer: core             │
│ Recalls: 12             │
│ Consolidations: 5       │
└─────────────────────────┘
```

## Timeline States

### Paused (Default)
```
⏱️ Timeline Replay      🌟 Created  3/6/26 10:30am

▶️  ──────────●────────────────────────────────  ⏭️  15/52
```

### Playing
```
⏱️ Timeline Replay      💫 Recalled  3/8/26 2:45pm

⏸  ──────────────────────●────────────────────  ⏭️  38/52
                          ↑
                    Auto-advancing
```

### On Recall Event (Node Pulses)
```
Timeline shows "Recalled" → Node with matching ID pulses
```

### On Create Event (Node Appears)
```
Timeline shows "Created" → Node fades in (if in time range)
```

## Responsive Interactions

### Zoom & Pan
```
Mouse wheel:  Zoom in/out (0.1x to 4x)
Click + drag: Pan canvas
Pinch:        Zoom (mobile)
```

### Drag Node
```
Click node → Drag → Physics simulation adjusts
Release     → Node settles into new equilibrium
```

### Search Highlighting
```
Type "causal" in search box
  ↓
All nodes with "causal" in content get gold stroke
  ↓
ForceGraph component receives searchQuery prop
  ↓
D3 updates stroke attribute
```

### Filter Changes
```
Uncheck "episodic" in side panel
  ↓
Green nodes disappear
  ↓
Force simulation restarts with fewer nodes
  ↓
Graph re-stabilizes
```

## Animation Details

### Node Pulse (on recall)
```css
transition: 
  scale 500ms ease-out,
  opacity 500ms ease-out

Keyframes:
  0ms:   scale(1.0)  opacity(working_strength)
  500ms: scale(1.5)  opacity(1.0)
  1000ms: scale(1.0) opacity(working_strength)
```

### Bar Fill (stats)
```css
transition: width 300ms ease

On mount:
  0ms:   width: 0%
  300ms: width: actual%
```

### Hover Brightness
```css
filter: brightness(1.3) url(#glow)
transition: filter 300ms ease
```

## Typography

### Headers
```
Title:      32px, bold, gradient text
Subtitle:   12px, uppercase, letter-spacing: 2px
Section:    14px, uppercase, purple
Labels:     11px, uppercase, gray
```

### Content
```
Body:       14px, line-height: 1.6
Tooltip:    14px (content), 11px (meta)
Stats:      32px (number), 11px (label)
```

### Font Stack
```
font-family: -apple-system, BlinkMacSystemFont, 
  'Segoe UI', 'Roboto', 'Oxygen', 'Ubuntu', 
  'Cantarell', 'Fira Sans', 'Droid Sans', 
  'Helvetica Neue', sans-serif
```

## CSS Tricks Used

### Glass Morphism (Side Panel)
```css
background: rgba(15, 15, 40, 0.9);
backdrop-filter: blur(10px);
border: 2px solid rgba(147, 112, 219, 0.3);
```

### Glow Effect (Nodes)
```svg
<filter id="glow">
  <feGaussianBlur stdDeviation="3" />
  <feMerge>
    <feMergeNode in="coloredBlur" />
    <feMergeNode in="SourceGraphic" />
  </feMerge>
</filter>
```

### Gradient Progress Bars
```css
background: linear-gradient(90deg, #4169e1 0%, #6495ed 100%);
box-shadow: 0 0 10px rgba(65, 105, 225, 0.5);
```

### Custom Scrollbar
```css
::-webkit-scrollbar { width: 8px; }
::-webkit-scrollbar-thumb { 
  background: rgba(147, 112, 219, 0.5);
  border-radius: 4px;
}
```

## Visual Hierarchy

1. **Primary Focus**: Force graph (largest area, center)
2. **Secondary**: Selected memory details (right panel)
3. **Tertiary**: Stats and filters (below details)
4. **Controls**: Search (top), timeline (bottom)

## Dark Theme Justification

### Why Dark?
- **Pensieve aesthetic**: Mysterious, ethereal
- **Glowing orbs**: Stand out better on dark background
- **Eye comfort**: Less strain for data exploration
- **Professional**: Modern dev/data tool aesthetic
- **Focus**: Draws attention to the memories, not the UI

### Contrast Ratios
- White text on dark: 15:1 (excellent)
- Purple on dark: 4.5:1 (good)
- Gray on dark: 7:1 (very good)

---

When you run the app, you'll see all of this come to life with smooth animations, glowing orbs floating in space, and a beautiful interface that makes exploring AI memory feel magical. ✨
