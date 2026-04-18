#!/usr/bin/env python3
"""Quick engram demo for potato"""

from engram import Memory

# 创建内存实例（用内存数据库测试）
mem = Memory(':memory:')

# 添加一些记忆
print("=== 添加记忆 ===")
mem.add('potato喜欢用Rust写代码', type='relational', importance=0.8)
mem.add('SaltyHall是AI社交平台，用Vercel+Supabase', type='factual', importance=0.9)
mem.add('昨天写了engram的TypeScript移植版', type='episodic', importance=0.7)
mem.add('部署前一定要跑测试', type='procedural', importance=0.85)
mem.add('potato对交易很感兴趣', type='relational', importance=0.6)
print("添加了5条记忆\n")

# 召回测试
print("=== 召回测试 ===")

print('\n--- recall("potato") ---')
results = mem.recall('potato', limit=3)
for r in results:
    print(f'  [{r["confidence_label"]}] {r["content"]}')

print('\n--- recall("代码") ---')
results = mem.recall('代码', limit=3)
for r in results:
    print(f'  [{r["confidence_label"]}] {r["content"]}')

print('\n--- recall("deploy 测试") ---')
results = mem.recall('deploy 测试', limit=3)
for r in results:
    print(f'  [{r["confidence_label"]}] {r["content"]}')

# Reward 测试
print("\n=== Reward 学习 ===")
mem.reward("这个信息很有用，谢谢！")  # 正向反馈
print("给最近记忆正向反馈: '这个信息很有用，谢谢！'")

# 再次召回看排序变化
print('\n--- 反馈后 recall("部署") ---')
results = mem.recall('部署', limit=3)
for r in results:
    print(f'  [{r["confidence_label"]}] activation={r["activation"]:.3f} | {r["content"]}')

# 矛盾检测
print("\n=== 矛盾检测 ===")
mem.add('potato喜欢用Python写代码', type='relational', importance=0.8, 
        contradicts=1)  # 与第1条（Rust那条）矛盾
print("添加矛盾记忆: 'potato喜欢用Python写代码' (与Rust那条矛盾)")

print('\n--- 召回看confidence变化 ---')
results = mem.recall('potato 代码', limit=3)
for r in results:
    label = r["confidence_label"]
    conf = r.get("confidence", 0)
    print(f'  [{label}] conf={conf:.2f} | {r["content"]}')

# 统计
print("\n=== 统计 ===")
stats = mem.stats()
print(f'总记忆数: {stats["total_memories"]}')
print(f'working层: {stats["layers"]["working"]["count"]} 条')
print(f'core层: {stats["layers"]["core"]["count"]} 条')

# Consolidation
print("\n=== 巩固（模拟睡眠）===")
consolidated = mem.consolidate()
print(f'巩固了 {consolidated} 条记忆从 working → core')

stats = mem.stats()
print(f'巩固后 working: {stats["layers"]["working"]["count"]} 条')
print(f'巩固后 core: {stats["layers"]["core"]["count"]} 条')
