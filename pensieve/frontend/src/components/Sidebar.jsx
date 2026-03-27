export default function Sidebar({ selected, stats, onClose }) {
  if (!selected) return (
    <div className="sidebar">
      <div className="sidebar-empty">
        <p>✨ Click a memory orb to inspect it</p>
        {stats && (
          <div className="sidebar-stats">
            <h3>Memory Types</h3>
            {Object.entries(stats.types).sort((a,b) => b[1] - a[1]).map(([type, count]) => (
              <div key={type} className="stat-row">
                <span className={`type-dot type-${type}`}></span>
                <span className="stat-label">{type}</span>
                <span className="stat-value">{count.toLocaleString()}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )

  const ts = (unix) => unix ? new Date(unix * 1000).toLocaleString() : 'N/A'

  return (
    <div className="sidebar sidebar-active">
      <button className="close-btn" onClick={onClose}>×</button>
      <div className="memory-detail">
        <div className={`memory-type-badge type-${selected.memory_type}`}>
          {selected.memory_type}
        </div>
        <h3>Memory Content</h3>
        <p className="memory-content">{selected.content}</p>
        
        {selected.summary && (
          <>
            <h4>Summary</h4>
            <p className="memory-summary">{selected.summary}</p>
          </>
        )}

        <div className="memory-meta">
          <div className="meta-row">
            <span>Layer</span><span>{selected.layer}</span>
          </div>
          <div className="meta-row">
            <span>Working Strength</span>
            <span>{(selected.working_strength || 0).toFixed(3)}</span>
          </div>
          <div className="meta-row">
            <span>Core Strength</span>
            <span>{(selected.core_strength || 0).toFixed(3)}</span>
          </div>
          <div className="meta-row">
            <span>Importance</span>
            <span>{(selected.importance || 0).toFixed(3)}</span>
          </div>
          <div className="meta-row">
            <span>Created</span><span>{ts(selected.created_at)}</span>
          </div>
          <div className="meta-row">
            <span>Recalls</span>
            <span>{(selected.accesses || []).length}</span>
          </div>
          <div className="meta-row">
            <span>Consolidations</span>
            <span>{selected.consolidation_count || 0}</span>
          </div>
          <div className="meta-row">
            <span>Namespace</span><span>{selected.namespace}</span>
          </div>
        </div>

        {selected.linked_memories && selected.linked_memories.length > 0 && (
          <>
            <h4>🔗 Hebbian Links ({selected.linked_memories.length})</h4>
            <div className="linked-list">
              {selected.linked_memories.map((lm, i) => (
                <div key={i} className="linked-item">
                  <span className={`type-dot type-${lm.memory_type}`}></span>
                  <span className="linked-content">
                    {lm.content.substring(0, 80)}{lm.content.length > 80 ? '...' : ''}
                  </span>
                  <span className="linked-strength">
                    ×{lm.coactivation_count}
                  </span>
                </div>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
