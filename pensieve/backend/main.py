from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
import sqlite3
import os
from typing import List, Dict, Any
import time

app = FastAPI(title="Engram Pensieve API")

# CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Database path from environment or default
DB_PATH = os.getenv("ENGRAM_DB_PATH", "engram_demo.db")

def get_db():
    """Get database connection"""
    if not os.path.exists(DB_PATH):
        # Create demo database if it doesn't exist
        create_demo_database()
    return sqlite3.connect(DB_PATH)

def create_demo_database():
    """Create a demo database with sample data"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    
    # Create tables
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            content TEXT,
            summary TEXT,
            memory_type TEXT,
            layer TEXT,
            created_at REAL,
            working_strength REAL,
            core_strength REAL,
            importance REAL,
            namespace TEXT,
            consolidation_count INTEGER
        )
    """)
    
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS access_log (
            memory_id TEXT,
            accessed_at REAL
        )
    """)
    
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS hebbian_links (
            source_id TEXT,
            target_id TEXT,
            strength REAL,
            coactivation_count INTEGER
        )
    """)
    
    # Insert sample memories
    base_time = time.time() - 86400 * 7  # 7 days ago
    
    sample_memories = [
        ("mem_001", "AutoAlpha uses causal inference for trading strategies", "Causal trading system", "semantic", "core", base_time, 0.3, 0.9, 0.95, "autolpha", 5),
        ("mem_002", "First conversation with potato about Engram design", "Initial Engram discussion", "episodic", "core", base_time + 3600, 0.2, 0.85, 0.8, "development", 4),
        ("mem_003", "How to implement force-directed graphs in D3.js", "D3 force simulation tutorial", "procedural", "working", base_time + 7200, 0.7, 0.2, 0.6, "development", 1),
        ("mem_004", "Causal graph has 84 nodes spanning 7 domains", "System architecture facts", "semantic", "core", base_time + 10800, 0.4, 0.8, 0.9, "autolpha", 3),
        ("mem_005", "When memories decay, working_strength approaches 0", "Memory decay mechanics", "causal", "core", base_time + 14400, 0.35, 0.75, 0.85, "engram", 4),
        ("mem_006", "User requested dark theme with glowing orbs", "Pensieve design requirements", "episodic", "working", base_time + 18000, 0.8, 0.15, 0.7, "development", 2),
        ("mem_007", "Hebbian links strengthen with co-activation", "Association learning principle", "causal", "core", base_time + 21600, 0.25, 0.82, 0.88, "engram", 5),
        ("mem_008", "React 18 + Vite for frontend stack", "Tech stack decision", "semantic", "working", base_time + 25200, 0.65, 0.25, 0.5, "development", 1),
        ("mem_009", "Dumbledore's Pensieve: blue-silver aesthetic", "Visual inspiration", "episodic", "working", base_time + 28800, 0.75, 0.18, 0.6, "development", 2),
        ("mem_010", "Sharpe ratio of 1.27 achieved in Phase 2", "Performance metric", "semantic", "core", base_time + 32400, 0.3, 0.78, 0.82, "autolpha", 3),
        ("mem_011", "FastAPI serves data from SQLite efficiently", "Backend architecture pattern", "procedural", "working", base_time + 36000, 0.72, 0.22, 0.55, "development", 1),
        ("mem_012", "93 tradeable edges from causal engine", "System capabilities", "semantic", "core", base_time + 39600, 0.38, 0.85, 0.92, "autolpha", 4),
        ("mem_013", "Timeline slider replays memory formation", "Feature specification", "episodic", "working", base_time + 43200, 0.82, 0.12, 0.65, "development", 1),
        ("mem_014", "Core strength grows through consolidation", "Memory consolidation rule", "causal", "core", base_time + 46800, 0.28, 0.88, 0.9, "engram", 5),
        ("mem_015", "Node size represents total activation", "Visualization mapping", "semantic", "working", 0.68, 0.28, 0.58, "development", 2),
    ]
    
    cursor.executemany(
        "INSERT OR IGNORE INTO memories VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        sample_memories
    )
    
    # Insert access logs (simulate recalls)
    access_logs = []
    for mem_id, *_ in sample_memories:
        # Each memory accessed 2-5 times
        num_accesses = hash(mem_id) % 4 + 2
        for i in range(num_accesses):
            access_time = base_time + (i + 1) * 10000 + hash(mem_id + str(i)) % 5000
            access_logs.append((mem_id, access_time))
    
    cursor.executemany(
        "INSERT INTO access_log VALUES (?, ?)",
        access_logs
    )
    
    # Insert hebbian links (memories that co-activate)
    hebbian_links = [
        ("mem_001", "mem_004", 0.85, 12),
        ("mem_001", "mem_010", 0.78, 9),
        ("mem_001", "mem_012", 0.92, 15),
        ("mem_004", "mem_012", 0.75, 8),
        ("mem_005", "mem_007", 0.88, 11),
        ("mem_005", "mem_014", 0.95, 14),
        ("mem_007", "mem_014", 0.82, 10),
        ("mem_003", "mem_008", 0.68, 6),
        ("mem_003", "mem_011", 0.72, 7),
        ("mem_008", "mem_011", 0.65, 5),
        ("mem_006", "mem_009", 0.8, 9),
        ("mem_006", "mem_013", 0.75, 7),
        ("mem_002", "mem_006", 0.7, 6),
        ("mem_009", "mem_015", 0.73, 8),
        ("mem_013", "mem_015", 0.7, 6),
    ]
    
    cursor.executemany(
        "INSERT INTO hebbian_links VALUES (?, ?, ?, ?)",
        hebbian_links
    )
    
    conn.commit()
    conn.close()
    print(f"✨ Created demo database at {DB_PATH}")

@app.get("/")
def root():
    return {"message": "Engram Pensieve API", "version": "1.0"}

@app.get("/api/memories")
def get_memories():
    """Get all memories with their properties"""
    conn = get_db()
    cursor = conn.cursor()
    
    cursor.execute("""
        SELECT 
            m.id,
            m.content,
            m.summary,
            m.memory_type,
            m.layer,
            m.created_at,
            m.working_strength,
            m.core_strength,
            m.importance,
            m.namespace,
            m.consolidation_count,
            COUNT(a.accessed_at) as recall_count
        FROM memories m
        LEFT JOIN access_log a ON m.id = a.memory_id
        GROUP BY m.id
    """)
    
    columns = [desc[0] for desc in cursor.description]
    memories = [dict(zip(columns, row)) for row in cursor.fetchall()]
    
    conn.close()
    return memories

@app.get("/api/links")
def get_links():
    """Get all hebbian links"""
    conn = get_db()
    cursor = conn.cursor()
    
    cursor.execute("SELECT * FROM hebbian_links")
    columns = [desc[0] for desc in cursor.description]
    links = [dict(zip(columns, row)) for row in cursor.fetchall()]
    
    conn.close()
    return links

@app.get("/api/timeline")
def get_timeline():
    """Get chronological access log for timeline replay"""
    conn = get_db()
    cursor = conn.cursor()
    
    # Get all memory creation times
    cursor.execute("""
        SELECT id, created_at as timestamp, 'created' as event_type
        FROM memories
        UNION ALL
        SELECT memory_id as id, accessed_at as timestamp, 'recalled' as event_type
        FROM access_log
        ORDER BY timestamp ASC
    """)
    
    columns = [desc[0] for desc in cursor.description]
    timeline = [dict(zip(columns, row)) for row in cursor.fetchall()]
    
    conn.close()
    return timeline

@app.get("/api/stats")
def get_stats():
    """Get summary statistics"""
    conn = get_db()
    cursor = conn.cursor()
    
    # Total memories
    cursor.execute("SELECT COUNT(*) FROM memories")
    total_memories = cursor.fetchone()[0]
    
    # Total recalls
    cursor.execute("SELECT COUNT(*) FROM access_log")
    total_recalls = cursor.fetchone()[0]
    
    # Type breakdown
    cursor.execute("""
        SELECT memory_type, COUNT(*) as count
        FROM memories
        GROUP BY memory_type
    """)
    type_breakdown = {row[0]: row[1] for row in cursor.fetchall()}
    
    # Namespace breakdown
    cursor.execute("""
        SELECT namespace, COUNT(*) as count
        FROM memories
        GROUP BY namespace
    """)
    namespace_breakdown = {row[0]: row[1] for row in cursor.fetchall()}
    
    # Layer breakdown
    cursor.execute("""
        SELECT layer, COUNT(*) as count
        FROM memories
        GROUP BY layer
    """)
    layer_breakdown = {row[0]: row[1] for row in cursor.fetchall()}
    
    # Average strengths
    cursor.execute("""
        SELECT 
            AVG(working_strength) as avg_working,
            AVG(core_strength) as avg_core,
            AVG(importance) as avg_importance
        FROM memories
    """)
    row = cursor.fetchone()
    
    conn.close()
    
    return {
        "total_memories": total_memories,
        "total_recalls": total_recalls,
        "type_breakdown": type_breakdown,
        "namespace_breakdown": namespace_breakdown,
        "layer_breakdown": layer_breakdown,
        "avg_working_strength": row[0],
        "avg_core_strength": row[1],
        "avg_importance": row[2]
    }

if __name__ == "__main__":
    import uvicorn
    port = int(os.getenv("PORT", "8000"))
    uvicorn.run(app, host="0.0.0.0", port=port)
