import { useRef, useEffect, useCallback } from 'react'
import ForceGraph3D from '3d-force-graph'

const TYPE_COLORS = {
  semantic: '#4fc3f7',
  episodic: '#81c784',
  procedural: '#ce93d8',
  causal: '#ffb74d',
  relational: '#4fc3f7',
  factual: '#64b5f6',
  preference: '#f06292',
  observation: '#aed581',
  belief: '#90a4ae',
  opinion: '#ef9a9a',
  default: '#78909c',
}

function hexToRgb(hex) {
  const r = parseInt(hex.slice(1, 3), 16)
  const g = parseInt(hex.slice(3, 5), 16)
  const b = parseInt(hex.slice(5, 7), 16)
  return { r, g, b }
}

function getColor(type) {
  return TYPE_COLORS[type] || TYPE_COLORS.default
}

export default function ForceGraph({ memories, links, selected, onSelect, search }) {
  const containerRef = useRef()
  const graphRef = useRef()

  useEffect(() => {
    if (!containerRef.current) return

    const graph = ForceGraph3D()(containerRef.current)
      .backgroundColor('#06060f')
      .nodeLabel(node => {
        const content = node.content || ''
        const short = content.length > 120 ? content.substring(0, 120) + '...' : content
        return `<div style="background:rgba(10,10,30,0.95);padding:10px 14px;border-radius:8px;border:1px solid rgba(79,195,247,0.3);max-width:350px;font-size:13px;color:#c8cad0;line-height:1.5">
          <strong style="color:${getColor(node.memory_type)};text-transform:uppercase;font-size:11px">${node.memory_type || '?'}</strong><br/>
          ${short}<br/>
          <small style="color:#78909c">Strength: ${(node.strength || 0).toFixed(2)} | Recalls: ${node.recall_count || 0}</small>
        </div>`
      })
      .nodeColor(node => {
        if (search) {
          const lower = search.toLowerCase()
          if (node.content && node.content.toLowerCase().includes(lower)) {
            return '#ffffff'
          }
        }
        if (selected && node.id === selected) return '#ffffff'
        return getColor(node.memory_type)
      })
      .nodeOpacity(0.9)
      .nodeVal(node => {
        const s = (node.working_strength || 0) + (node.core_strength || 0)
        return 0.5 + s * 3
      })
      .nodeResolution(8)
      .linkColor(link => {
        const s = link.strength || 0
        const alpha = Math.min(0.6, 0.1 + s * 0.5)
        return `rgba(79, 195, 247, ${alpha})`
      })
      .linkWidth(link => 0.3 + (link.strength || 0) * 1.5)
      .linkOpacity(0.6)
      .onNodeClick(node => {
        onSelect(node.id)
        // Focus camera on clicked node
        const distance = 120
        const distRatio = 1 + distance / Math.hypot(node.x, node.y, node.z)
        graph.cameraPosition(
          { x: node.x * distRatio, y: node.y * distRatio, z: node.z * distRatio },
          node,
          1500
        )
      })
      .onBackgroundClick(() => onSelect(null))
      .warmupTicks(50)
      .cooldownTicks(100)
      .d3AlphaDecay(0.05)
      .d3VelocityDecay(0.4)

    // Add bloom/glow post-processing
    const renderer = graph.renderer()
    if (renderer) {
      renderer.toneMappingExposure = 1.5
    }

    graphRef.current = graph

    // Handle resize
    const handleResize = () => {
      const container = containerRef.current
      if (container && graph) {
        graph.width(container.clientWidth)
        graph.height(container.clientHeight)
      }
    }
    window.addEventListener('resize', handleResize)
    handleResize()

    return () => {
      window.removeEventListener('resize', handleResize)
      graph._destructor && graph._destructor()
    }
  }, [])

  // Update data when memories/links change
  useEffect(() => {
    if (!graphRef.current || !memories.length) return

    const nodeSet = new Set(memories.map(m => m.id))
    
    const graphData = {
      nodes: memories.map(m => ({
        id: m.id,
        content: m.content,
        memory_type: m.memory_type,
        working_strength: m.working_strength,
        core_strength: m.core_strength,
        strength: (m.working_strength || 0) + (m.core_strength || 0),
        recall_count: m.recall_count || 0,
        importance: m.importance,
      })),
      links: links
        .filter(l => nodeSet.has(l.source_id) && nodeSet.has(l.target_id))
        .map(l => ({
          source: l.source_id,
          target: l.target_id,
          strength: l.strength,
        }))
    }

    graphRef.current.graphData(graphData)
  }, [memories, links])

  // Update colors when search/selected changes
  useEffect(() => {
    if (graphRef.current) {
      graphRef.current.nodeColor(graphRef.current.nodeColor())
    }
  }, [search, selected])

  return (
    <div className="graph-container" ref={containerRef} />
  )
}
