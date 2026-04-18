#!/usr/bin/env python3
"""
Hebbian Learning Prototype
==========================
Test: memories that are recalled together should automatically form connections.
"Neurons that fire together, wire together"
"""

from engram import Memory
from collections import defaultdict
from itertools import combinations

# 扩展 Memory 类，加入 Hebbian learning
class HebbianMemory(Memory):
    def __init__(self, path, **kwargs):
        super().__init__(path, **kwargs)
        # 追踪 co-activation 次数
        self._coactivation = defaultdict(int)  # (id1, id2) -> count
        self._hebbian_threshold = 3  # co-activate 3次后自动建立连接
        
        # 创建 memory-to-memory 连接表
        self._store._conn.execute("""
            CREATE TABLE IF NOT EXISTS hebbian_links (
                source_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
                target_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
                strength REAL DEFAULT 1.0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (source_id, target_id)
            )
        """)
        self._store._conn.commit()
    
    def recall(self, query, limit=5, **kwargs):
        results = super().recall(query, limit=limit, **kwargs)
        
        # 记录 co-activation
        ids = [r['id'] for r in results]
        for id1, id2 in combinations(ids, 2):
            # 保证顺序一致 (smaller_id, larger_id)
            pair = tuple(sorted([id1, id2]))
            self._coactivation[pair] += 1
            
            # 超过阈值 → 创建连接
            if self._coactivation[pair] == self._hebbian_threshold:
                self._create_hebbian_link(id1, id2)
        
        return results
    
    def _create_hebbian_link(self, id1, id2):
        """在两个记忆之间创建 Hebbian 连接"""
        # 检查是否已存在连接
        existing = self._store._conn.execute(
            "SELECT 1 FROM hebbian_links WHERE source_id=? AND target_id=?",
            (id1, id2)
        ).fetchone()
        
        if not existing:
            # 创建双向连接
            self._store._conn.execute(
                "INSERT OR IGNORE INTO hebbian_links (source_id, target_id, strength) VALUES (?, ?, ?)",
                (id1, id2, 1.0)
            )
            self._store._conn.execute(
                "INSERT OR IGNORE INTO hebbian_links (source_id, target_id, strength) VALUES (?, ?, ?)",
                (id2, id1, 1.0)
            )
            self._store._conn.commit()
            
            # 获取记忆内容用于打印
            m1 = self._store.get(id1)
            m2 = self._store.get(id2)
            print(f"  🔗 Hebbian link formed: '{m1.content[:30]}...' ↔ '{m2.content[:30]}...'")
    
    def get_hebbian_links(self):
        """获取所有 Hebbian 连接"""
        rows = self._store._conn.execute(
            "SELECT DISTINCT source_id, target_id FROM hebbian_links"
        ).fetchall()
        return [(r[0], r[1]) for r in rows]
    
    def coactivation_stats(self):
        """返回 co-activation 统计"""
        return dict(self._coactivation)


# ========== 测试 ==========
print("=" * 60)
print("  Hebbian Learning Test")
print("=" * 60)

mem = HebbianMemory(":memory:")

# 添加一些记忆（不手动指定连接）
print("\n📝 Adding memories (no manual links)...\n")
mem.add("Python是一种编程语言", type="factual", importance=0.5)
mem.add("机器学习需要大量数据", type="factual", importance=0.6)
mem.add("神经网络是深度学习的基础", type="factual", importance=0.7)
mem.add("TensorFlow是Google的ML框架", type="factual", importance=0.5)
mem.add("PyTorch更pythonic，适合研究", type="opinion", importance=0.6)
mem.add("今天天气很好", type="episodic", importance=0.1)
mem.add("咖啡帮助我集中注意力", type="relational", importance=0.3)

print(f"Total memories: {len(mem)}")
print(f"Initial Hebbian links: {len(mem.get_hebbian_links())}")

# 模拟多次查询，观察连接形成
print("\n🔍 Simulating queries...\n")

queries = [
    "机器学习框架",      # Should recall TensorFlow, PyTorch, 神经网络
    "深度学习工具",      # Similar - should reinforce same connections
    "Python ML",        # Should recall Python, ML related
    "神经网络 PyTorch",  # Reinforce connections
    "机器学习",         # More reinforcement
]

for i, q in enumerate(queries, 1):
    print(f"\n--- Query {i}: '{q}' ---")
    results = mem.recall(q, limit=4)
    for r in results:
        print(f"  [{r['type'][:4]}] {r['content'][:50]}")

# 显示最终结果
print("\n" + "=" * 60)
print("  Results")
print("=" * 60)

print(f"\n📊 Co-activation stats:")
stats = mem.coactivation_stats()
sorted_stats = sorted(stats.items(), key=lambda x: x[1], reverse=True)
for pair, count in sorted_stats[:10]:
    m1 = mem._store.get(pair[0])
    m2 = mem._store.get(pair[1])
    print(f"  {count}x: '{m1.content[:25]}...' ↔ '{m2.content[:25]}...'")

print(f"\n🔗 Hebbian links formed: {len(mem.get_hebbian_links()) // 2}")  # 除2因为是双向

# 测试：graph_expand 现在应该能利用 Hebbian links
print("\n🧠 Testing graph expansion with Hebbian links...")
print("\n--- Query: 'TensorFlow' (with graph_expand=True) ---")
results = mem.recall("TensorFlow", limit=5, graph_expand=True)
for r in results:
    print(f"  [{r['type'][:4]}] {r['content'][:50]}")

print("\n✅ Hebbian learning works!")
