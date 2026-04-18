#!/usr/bin/env python3
"""
Engram 交互式Demo — 让你直观感受AI记忆系统

用法: PYTHONPATH=. python3 demo.py
"""

import sys
import os
sys.path.insert(0, os.path.dirname(__file__))

from engram.memory import Memory
import time

DB_PATH = "./demo.db"

# ANSI colors
CYAN = "\033[96m"
GREEN = "\033[92m"
YELLOW = "\033[93m"
RED = "\033[91m"
DIM = "\033[2m"
BOLD = "\033[1m"
RESET = "\033[0m"
BAR_FULL = "█"
BAR_EMPTY = "░"

def bar(value, max_val=1.0, width=20):
    filled = int(value / max_val * width)
    return f"{BAR_FULL * filled}{BAR_EMPTY * (width - filled)}"

def show_memories(mem, label="当前记忆状态"):
    results = mem.recall("", limit=50)
    if not results:
        print(f"\n  {DIM}(没有记忆){RESET}")
        return
    
    print(f"\n  {BOLD}{label}{RESET}")
    print(f"  {'类型':<12} {'层级':<8} {'置信度':<10} {'强度图':<22} 内容")
    print(f"  {'─'*80}")
    for r in results:
        conf = r['confidence']
        label_str = r['confidence_label']
        
        # Color code confidence
        if conf >= 0.8:
            color = GREEN
        elif conf >= 0.5:
            color = YELLOW
        else:
            color = RED
        
        layer_str = r.get('layer', 'working')
        type_str = r['type'][:10]
        content = r['content'][:40]
        
        print(f"  {type_str:<12} {layer_str:<8} {color}{conf:.2f} {label_str:<9}{RESET} |{bar(conf)}| {content}")

def main():
    # 清除旧demo数据
    if os.path.exists(DB_PATH):
        os.remove(DB_PATH)
    
    mem = Memory(DB_PATH)
    
    print(f"""
{BOLD}{CYAN}╔══════════════════════════════════════════════════════════╗
║          🧠 Engram — 交互式记忆Demo                     ║
╚══════════════════════════════════════════════════════════╝{RESET}

{DIM}这个demo让你亲手操作一个AI的记忆系统。
你会看到记忆如何存储、巩固、衰减和遗忘。{RESET}
""")
    
    input(f"{CYAN}按 Enter 开始...{RESET}")
    
    # === Phase 1: 存入记忆 ===
    print(f"\n{BOLD}═══ 第1步：存入记忆 ═══{RESET}")
    print(f"\n  存入5条不同类型和重要性的记忆...\n")
    
    memories = [
        ("potato喜欢用Opus写代码", "relational", 0.7),
        ("SaltyHall今天上线了20个agent", "episodic", 0.5),
        ("moltbook.com要用www前缀，否则auth会丢", "procedural", 0.9),
        ("今天看了一条关于天气的推文", "episodic", 0.1),
        ("potato说'我还挺喜欢你的'", "emotional", 0.95),
    ]
    
    ids = []
    for content, mtype, imp in memories:
        mid = mem.add(content, type=mtype, importance=imp)
        ids.append(mid)
        emoji = {"relational": "👤", "episodic": "📅", "procedural": "🔧", "emotional": "❤️"}
        print(f"  {emoji.get(mtype, '📝')} [{mtype:<12}] imp={imp:.2f} | {content}")
    
    show_memories(mem, "刚存入 — 所有记忆都是 certain (1.00)")
    input(f"\n{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 2: 模拟时间流逝 ===
    print(f"\n{BOLD}═══ 第2步：模拟7天过去（每天巩固一次）═══{RESET}")
    print(f"\n  {DIM}就像人类睡觉时大脑在整理记忆...{RESET}\n")
    
    for day in range(1, 8):
        mem.consolidate(days=1.0)
        if day in [1, 3, 7]:
            show_memories(mem, f"第{day}天后")
    
    print(f"""
  {YELLOW}注意观察:{RESET}
  • emotional记忆(❤️)衰减最慢 → 因为importance=0.95
  • 天气推文衰减最快 → 因为importance=0.1
  • procedural记忆在巩固 → core_strength在增长
  • 有的记忆已经从working转到core或archive层
""")
    input(f"{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 3: 搜索 ===
    print(f"\n{BOLD}═══ 第3步：搜索记忆 ═══{RESET}")
    print(f"\n  试试不同的搜索词...\n")
    
    for query in ["potato", "moltbook", "天气"]:
        results = mem.recall(query, limit=3)
        print(f"  🔍 搜索: \"{query}\"")
        for r in results:
            color = GREEN if r['confidence'] >= 0.7 else (YELLOW if r['confidence'] >= 0.4 else RED)
            print(f"     {color}{r['confidence']:.2f}{RESET} | {r['content'][:50]}")
        print()
    
    input(f"{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 4: 再过23天 ===
    print(f"\n{BOLD}═══ 第4步：再过23天（总共30天）═══{RESET}")
    print(f"\n  {DIM}一个月后，记忆发生了什么？{RESET}\n")
    
    for day in range(23):
        mem.consolidate(days=1.0)
    
    show_memories(mem, "30天后 — 记忆大幅衰减")
    
    print(f"""
  {YELLOW}关键变化:{RESET}
  • emotional记忆还在 → 情感记忆最持久（像人一样）
  • 天气推文几乎消失 → 不重要的事自然遗忘
  • procedural记忆因为高importance衰减较慢
""")
    input(f"{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 5: 奖赏学习 ===
    print(f"\n{BOLD}═══ 第5步：奖赏学习 ═══{RESET}")
    print(f"\n  模拟master说'好的，不错'...\n")
    
    show_memories(mem, "奖赏前")
    mem.reward("好的，不错", recent_n=3)
    show_memories(mem, "奖赏后 — 最近的记忆被加强了")
    
    input(f"{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 6: 突触缩放 ===
    print(f"\n{BOLD}═══ 第6步：突触缩放 ═══{RESET}")
    print(f"\n  {DIM}全局降低所有权重，让重要和不重要的记忆差距更明显...{RESET}\n")
    
    mem.downscale(factor=0.9)
    show_memories(mem, "缩放后 — 相对排序不变，绝对值降低")
    
    input(f"{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 7: 手动遗忘 ===
    print(f"\n{BOLD}═══ 第7步：遗忘弱记忆 ═══{RESET}")
    
    stats_before = mem.stats()
    mem.forget(threshold=0.08)
    stats_after = mem.stats()
    
    forgotten = stats_before['total_memories'] - stats_after['total_memories']
    print(f"\n  清除了 {forgotten} 条太弱的记忆")
    show_memories(mem, "遗忘后 — 只剩下重要的")
    
    input(f"{CYAN}按 Enter 继续...{RESET}")
    
    # === Phase 8: 导出 ===
    print(f"\n{BOLD}═══ 第8步：导出记忆 ═══{RESET}")
    
    export_path = "./demo_export.db"
    mem.export(export_path)
    size = os.path.getsize(export_path)
    print(f"\n  📦 导出到 {export_path} ({size:,} bytes)")
    print(f"  这个文件就是agent的完整记忆 — 可以带走、备份、迁移")
    
    # 验证
    mem2 = Memory(export_path)
    r = mem2.recall("potato")
    print(f"  ✅ 从导出文件恢复: 搜到 {len(r)} 条关于potato的记忆")
    mem2.close()
    os.remove(export_path)
    
    # === 最终统计 ===
    print(f"""
{BOLD}{CYAN}╔══════════════════════════════════════════════════════════╗
║          🧠 Demo 完成！                                  ║
╚══════════════════════════════════════════════════════════╝{RESET}

{BOLD}你刚才体验了:{RESET}
  1. 记忆存储 — 不同类型和重要性
  2. 巩固 — Memory Chain Model（工作记忆→核心记忆）
  3. 衰减 — Ebbinghaus遗忘曲线（情感记忆最持久）
  4. 检索 — FTS5 + ACT-R activation排序
  5. 奖赏学习 — 从反馈中调整权重
  6. 突触缩放 — 防止权重膨胀
  7. 遗忘 — 清除弱记忆
  8. 导出 — 一个.db文件 = 完整的agent记忆

{DIM}这些机制基于真实的计算神经科学模型，
不是简单的timestamp + cosine similarity。{RESET}
""")
    
    mem.close()
    os.remove(DB_PATH)

if __name__ == "__main__":
    main()
