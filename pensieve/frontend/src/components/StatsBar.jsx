export default function StatsBar({ stats }) {
  return (
    <div className="stats-bar">
      <div className="stat-pill">
        <span className="stat-icon">🧠</span>
        <span>{stats.memories.toLocaleString()} memories</span>
      </div>
      <div className="stat-pill">
        <span className="stat-icon">🔗</span>
        <span>{stats.hebbian_links.toLocaleString()} associations</span>
      </div>
      <div className="stat-pill">
        <span className="stat-icon">👁</span>
        <span>{stats.accesses.toLocaleString()} recalls</span>
      </div>
    </div>
  )
}
