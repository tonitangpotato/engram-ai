"""Engram Pensieve — Backend API (read-only access to Engram SQLite)"""
import os
import sqlite3
import math
from fastapi import FastAPI, Query
from fastapi.middleware.cors import CORSMiddleware

DB_PATH = os.environ.get("ENGRAM_DB", "/Users/potato/clawd/engram-memory.db")

app = FastAPI(title="Engram Pensieve API")
app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"])


def get_db():
    conn = sqlite3.connect(f"file:{DB_PATH}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    return conn


@app.get("/api/stats")
def stats():
    db = get_db()
    memories = db.execute("SELECT count(*) c FROM memories").fetchone()["c"]
    links = db.execute("SELECT count(*) c FROM hebbian_links").fetchone()["c"]
    accesses = db.execute("SELECT count(*) c FROM access_log").fetchone()["c"]
    types = db.execute("SELECT memory_type, count(*) c FROM memories GROUP BY memory_type").fetchall()
    namespaces = db.execute("SELECT DISTINCT namespace FROM memories").fetchall()
    db.close()
    return {
        "memories": memories,
        "hebbian_links": links,
        "accesses": accesses,
        "types": {r["memory_type"]: r["c"] for r in types},
        "namespaces": [r["namespace"] for r in namespaces],
    }


@app.get("/api/memories")
def memories(
    namespace: str = Query(None),
    memory_type: str = Query(None),
    limit: int = Query(500),
    offset: int = Query(0),
):
    db = get_db()
    where = []
    params = []
    if namespace:
        where.append("m.namespace = ?")
        params.append(namespace)
    if memory_type:
        where.append("m.memory_type = ?")
        params.append(memory_type)
    where_sql = ("WHERE " + " AND ".join(where)) if where else ""

    rows = db.execute(f"""
        SELECT m.id, m.content, m.summary, m.memory_type, m.layer,
               m.created_at, m.working_strength, m.core_strength,
               m.importance, m.namespace, m.consolidation_count,
               (SELECT count(*) FROM access_log a WHERE a.memory_id = m.id) as recall_count,
               (SELECT max(a.accessed_at) FROM access_log a WHERE a.memory_id = m.id) as last_accessed
        FROM memories m
        {where_sql}
        ORDER BY m.working_strength + m.core_strength DESC
        LIMIT ? OFFSET ?
    """, params + [limit, offset]).fetchall()
    db.close()
    return [dict(r) for r in rows]


@app.get("/api/links")
def links(min_strength: float = Query(0.01), limit: int = Query(500), memory_ids: str = Query(None)):
    db = get_db()
    if memory_ids:
        # Return links where BOTH endpoints are in the provided memory set
        ids = memory_ids.split(",")
        placeholders = ",".join("?" for _ in ids)
        rows = db.execute(f"""
            SELECT source_id, target_id, strength, coactivation_count
            FROM hebbian_links
            WHERE source_id IN ({placeholders})
              AND target_id IN ({placeholders})
              AND strength >= ?
            ORDER BY strength DESC
            LIMIT ?
        """, ids + ids + [min_strength, limit]).fetchall()
    else:
        rows = db.execute("""
            SELECT source_id, target_id, strength, coactivation_count
            FROM hebbian_links
            WHERE strength >= ?
            ORDER BY strength DESC
            LIMIT ?
        """, [min_strength, limit]).fetchall()
    db.close()
    return [dict(r) for r in rows]


@app.get("/api/timeline")
def timeline(memory_id: str = Query(None), limit: int = Query(1000)):
    db = get_db()
    if memory_id:
        rows = db.execute("""
            SELECT memory_id, accessed_at FROM access_log
            WHERE memory_id = ?
            ORDER BY accessed_at DESC LIMIT ?
        """, [memory_id, limit]).fetchall()
    else:
        rows = db.execute("""
            SELECT memory_id, accessed_at FROM access_log
            ORDER BY accessed_at DESC LIMIT ?
        """, [limit]).fetchall()
    db.close()
    return [dict(r) for r in rows]


@app.get("/api/memory/{memory_id}")
def memory_detail(memory_id: str):
    db = get_db()
    mem = db.execute("SELECT * FROM memories WHERE id = ?", [memory_id]).fetchone()
    if not mem:
        db.close()
        return {"error": "not found"}
    accesses = db.execute(
        "SELECT accessed_at FROM access_log WHERE memory_id = ? ORDER BY accessed_at",
        [memory_id]
    ).fetchall()
    linked = db.execute("""
        SELECT h.target_id as linked_id, h.strength, h.coactivation_count,
               m.content, m.memory_type
        FROM hebbian_links h
        JOIN memories m ON m.id = h.target_id
        WHERE h.source_id = ?
        UNION
        SELECT h.source_id as linked_id, h.strength, h.coactivation_count,
               m.content, m.memory_type
        FROM hebbian_links h
        JOIN memories m ON m.id = h.source_id
        WHERE h.target_id = ?
        ORDER BY strength DESC LIMIT 20
    """, [memory_id, memory_id]).fetchall()
    db.close()
    return {
        **dict(mem),
        "accesses": [r["accessed_at"] for r in accesses],
        "linked_memories": [dict(r) for r in linked],
    }


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8877)
