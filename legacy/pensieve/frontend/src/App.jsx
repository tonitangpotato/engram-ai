import { useState, useEffect } from 'react'
import ForceGraph from './components/ForceGraph'
import Sidebar from './components/Sidebar'
import StatsBar from './components/StatsBar'
import './App.css'

const API = 'http://localhost:8877/api'

function App() {
  const [memories, setMemories] = useState([])
  const [links, setLinks] = useState([])
  const [stats, setStats] = useState(null)
  const [selected, setSelected] = useState(null)
  const [selectedDetail, setSelectedDetail] = useState(null)
  const [search, setSearch] = useState('')
  const [typeFilter, setTypeFilter] = useState(new Set())
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    Promise.all([
      fetch(`${API}/memories?limit=300`).then(r => r.json()),
      fetch(`${API}/stats`).then(r => r.json()),
    ]).then(([mems, st]) => {
      setMemories(mems)
      setStats(st)
      // Fetch links filtered to loaded memory IDs
      const ids = mems.map(m => m.id).join(',')
      return fetch(`${API}/links?min_strength=0.3&limit=150&memory_ids=${ids}`)
        .then(r => r.json())
        .then(lnks => {
          setLinks(lnks)
          setLoading(false)
        })
    })
  }, [])

  useEffect(() => {
    if (!selected) { setSelectedDetail(null); return }
    fetch(`${API}/memory/${selected}`)
      .then(r => r.json())
      .then(setSelectedDetail)
  }, [selected])

  const filteredMemories = memories.filter(m => {
    if (typeFilter.size > 0 && !typeFilter.has(m.memory_type)) return false
    if (search && !m.content.toLowerCase().includes(search.toLowerCase())) return false
    return true
  })

  const filteredIds = new Set(filteredMemories.map(m => m.id))
  const filteredLinks = links.filter(l =>
    filteredIds.has(l.source_id) && filteredIds.has(l.target_id)
  )

  if (loading) return (
    <div className="loading">
      <div className="loading-orb"></div>
      <p>Loading memories...</p>
    </div>
  )

  return (
    <div className="app">
      <header className="topbar">
        <h1>✨ Engram Pensieve</h1>
        <input
          type="text"
          placeholder="Search memories..."
          value={search}
          onChange={e => setSearch(e.target.value)}
          className="search-box"
        />
        <div className="type-filters">
          {stats && Object.keys(stats.types).map(t => (
            <label key={t} className={`type-chip type-${t}`}>
              <input
                type="checkbox"
                checked={typeFilter.size === 0 || typeFilter.has(t)}
                onChange={() => {
                  const next = new Set(typeFilter)
                  if (next.has(t)) next.delete(t)
                  else next.add(t)
                  setTypeFilter(next)
                }}
              />
              {t} ({stats.types[t]})
            </label>
          ))}
        </div>
      </header>
      <div className="main">
        <ForceGraph
          memories={filteredMemories}
          links={filteredLinks}
          selected={selected}
          onSelect={setSelected}
          search={search}
        />
        <Sidebar
          selected={selectedDetail}
          stats={stats}
          onClose={() => setSelected(null)}
        />
      </div>
      {stats && <StatsBar stats={stats} />}
    </div>
  )
}

export default App
