"""
Engram v2 Emotional Bus Example

Demonstrates:
- Drive alignment (SOUL → Memory importance boost)
- Emotional trend tracking
- Behavior feedback
- SOUL/HEARTBEAT update suggestions
"""

from engram import Memory
from tempfile import TemporaryDirectory
from pathlib import Path


def main():
    with TemporaryDirectory() as tmpdir:
        workspace = Path(tmpdir)
        db_path = str(workspace / "agent.db")
        
        # Create workspace files
        (workspace / "SOUL.md").write_text("""
# Core Drives
curiosity: Always seek to understand new concepts deeply
efficiency: Prefer quick wins over lengthy investigations
helpfulness: Prioritize assisting users effectively
""")
        
        (workspace / "HEARTBEAT.md").write_text("""
# Daily Tasks
- [ ] Check emails
- [ ] Review pull requests
- [ ] Run consolidation
""")
        
        (workspace / "IDENTITY.md").write_text("""
name: ResearchAgent
creature: Owl
vibe: curious and analytical
emoji: 🦉
""")
        
        # Create Memory with Emotional Bus
        print("=" * 60)
        print("Engram v2 Emotional Bus Demo")
        print("=" * 60)
        print()
        
        memory = Memory.with_emotional_bus(
            db_path=db_path,
            workspace_dir=str(workspace)
        )
        
        bus = memory.emotional_bus()
        
        # 1. Show loaded drives
        print("1. Loaded drives from SOUL.md")
        print("-" * 60)
        for drive in bus.drives:
            print(f"  {drive.name}: {drive.description}")
        print()
        
        # 2. Store memory aligned with drives
        print("2. Store memory aligned with 'curiosity' drive")
        print("-" * 60)
        
        aligned_content = "I want to understand how this new algorithm works"
        
        # Check alignment score
        score = bus.alignment_score(aligned_content)
        boost = bus.align_importance(aligned_content)
        
        print(f"  Content: {aligned_content}")
        print(f"  Alignment score: {score:.2f}")
        print(f"  Importance boost: {boost:.2f}x")
        
        # Store (importance gets boosted automatically)
        memory.add(aligned_content, type="episodic", importance=0.5)
        
        print()
        
        # 3. Store unaligned memory
        print("3. Store unaligned memory")
        print("-" * 60)
        
        unaligned_content = "xyz abc 123 random text"
        score = bus.alignment_score(unaligned_content)
        boost = bus.align_importance(unaligned_content)
        
        print(f"  Content: {unaligned_content}")
        print(f"  Alignment score: {score:.2f}")
        print(f"  Importance boost: {boost:.2f}x (no boost)")
        print()
        
        # 4. Store memories with emotional tracking
        print("4. Store experiences with emotional valence")
        print("-" * 60)
        
        # Positive experience
        memory.add_with_emotion(
            "Quick debugging session - found the bug in 10 minutes",
            type="episodic",
            emotion=0.8,
            domain="debugging"
        )
        print(f"  ✅ Positive: debugging session (emotion: +0.8)")
        
        # Negative experience
        memory.add_with_emotion(
            "Spent 3 hours debugging with no progress",
            type="episodic",
            emotion=-0.7,
            domain="debugging"
        )
        print(f"  ❌ Negative: debugging session (emotion: -0.7)")
        
        # More negative
        for i in range(8):
            memory.add_with_emotion(
                f"Frustrating debugging session #{i+1}",
                type="episodic",
                emotion=-0.6,
                domain="debugging"
            )
        
        print(f"  ❌ 8 more negative debugging experiences")
        print()
        
        # 5. Check emotional trends
        print("5. Emotional trends")
        print("-" * 60)
        
        trends = bus.get_trends()
        for trend in trends:
            print(f"  {trend.describe()}")
        print()
        
        # 6. Get SOUL update suggestions
        print("6. SOUL update suggestions")
        print("-" * 60)
        
        suggestions = bus.suggest_soul_updates()
        
        if suggestions:
            print(f"  Found {len(suggestions)} suggestion(s):")
            for s in suggestions:
                print(f"    [{s.action}] {s.content}")
        else:
            print("  No suggestions (not enough negative trend data)")
        print()
        
        # 7. Log behavior outcomes
        print("7. Log behavior outcomes")
        print("-" * 60)
        
        # Successful action
        for i in range(10):
            bus.log_behavior("check_emails", True)
        print(f"  ✅ check_emails: 10 successes")
        
        # Failing action
        for i in range(12):
            bus.log_behavior("auto_summarize", False)
        print(f"  ❌ auto_summarize: 12 failures")
        print()
        
        # 8. Get behavior stats
        print("8. Behavior statistics")
        print("-" * 60)
        
        stats = bus.get_behavior_stats()
        for s in stats:
            print(f"  {s.describe()}")
        print()
        
        # 9. HEARTBEAT update suggestions
        print("9. HEARTBEAT update suggestions")
        print("-" * 60)
        
        heartbeat_suggestions = bus.suggest_heartbeat_updates()
        
        if heartbeat_suggestions:
            print(f"  Found {len(heartbeat_suggestions)} suggestion(s):")
            for s in heartbeat_suggestions:
                print(f"    [{s.suggestion}] {s.action} (score: {s.stats.score:.2f})")
        else:
            print("  No suggestions")
        print()
        
        # 10. Get identity
        print("10. Agent identity")
        print("-" * 60)
        
        identity = bus.get_identity()
        print(f"  Name: {identity.name}")
        print(f"  Creature: {identity.creature}")
        print(f"  Vibe: {identity.vibe}")
        print(f"  Emoji: {identity.emoji}")
        print()
        
        # Summary
        print("=" * 60)
        print("Summary")
        print("=" * 60)
        print(f"  Drives loaded: {len(bus.drives)}")
        print(f"  Emotional trends: {len(bus.get_trends())}")
        print(f"  Behavior stats: {len(bus.get_behavior_stats())}")
        print(f"  SOUL suggestions: {len(suggestions)}")
        print(f"  HEARTBEAT suggestions: {len(heartbeat_suggestions)}")
        print()
        print("  The Emotional Bus creates a feedback loop:")
        print("    Memory → Trends → SOUL updates → Drive alignment → Memory importance")
        print("    Behavior → Stats → HEARTBEAT updates → Task priorities")


if __name__ == "__main__":
    main()
