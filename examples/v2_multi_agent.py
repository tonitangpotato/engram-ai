"""
Engram v2 Multi-Agent Example

Demonstrates:
- Namespace isolation
- ACL (Access Control Lists)
- Subscriptions and notifications
- CEO pattern (supervisor monitors specialists)
"""

from engram import Memory
from engram.acl import Permission
from tempfile import TemporaryDirectory
from pathlib import Path


def main():
    with TemporaryDirectory() as tmpdir:
        db_path = str(Path(tmpdir) / "shared.db")
        
        # Create shared memory system
        memory = Memory(db_path)
        
        # ═══ Scenario: Trading firm with multiple specialist agents ═══
        
        # 1. Trading agent stores market data in its namespace
        print("=" * 60)
        print("1. Trading agent stores market data")
        print("=" * 60)
        
        memory.set_agent_id("trading_agent")
        
        # Store in trading namespace
        m1 = memory.add_to_namespace(
            "Oil prices spiked 15% due to Middle East tensions",
            type="factual",
            importance=0.9,
            namespace="trading"
        )
        
        m2 = memory.add_to_namespace(
            "Gold reached new all-time high",
            type="factual",
            importance=0.8,
            namespace="trading"
        )
        
        print(f"  Stored 2 memories in 'trading' namespace")
        print()
        
        # 2. Engine agent stores technical data
        print("2. Engine agent stores technical data")
        print("-" * 60)
        
        memory.set_agent_id("engine_agent")
        
        memory.add_to_namespace(
            "CPU temperature reached 85°C under load",
            type="factual",
            importance=0.6,
            namespace="engine"
        )
        
        memory.add_to_namespace(
            "Memory usage exceeded 90% threshold",
            type="factual",
            importance=0.7,
            namespace="engine"
        )
        
        print(f"  Stored 2 memories in 'engine' namespace")
        print()
        
        # 3. CEO agent subscribes to all namespaces
        print("3. CEO agent subscribes to high-importance events")
        print("-" * 60)
        
        memory.set_agent_id("ceo_agent")
        
        # Subscribe to all namespaces with threshold 0.8
        memory.subscribe("ceo_agent", "*", min_importance=0.8)
        print(f"  CEO subscribed to all namespaces (threshold: 0.8)")
        print()
        
        # 4. Check notifications
        print("4. CEO checks for notifications")
        print("-" * 60)
        
        notifications = memory.check_notifications("ceo_agent")
        
        print(f"  Found {len(notifications)} high-importance events:")
        for notif in notifications:
            print(f"    [{notif['namespace']:10s}] {notif['content'][:50]}")
            print(f"                  importance={notif['importance']:.2f}")
        print()
        
        # 5. CEO checks again - should be empty (cursor advanced)
        print("5. CEO checks again (should be empty)")
        print("-" * 60)
        
        notifications = memory.check_notifications("ceo_agent")
        print(f"  Found {len(notifications)} new notifications (cursor already advanced)")
        print()
        
        # 6. Trading agent adds another high-importance memory
        print("6. Trading agent adds breaking news")
        print("-" * 60)
        
        memory.set_agent_id("trading_agent")
        memory.add_to_namespace(
            "Federal Reserve announces emergency rate cut",
            type="factual",
            importance=0.95,
            namespace="trading"
        )
        
        print(f"  Stored breaking news in 'trading' namespace")
        print()
        
        # 7. CEO gets notified
        print("7. CEO checks notifications again")
        print("-" * 60)
        
        memory.set_agent_id("ceo_agent")
        notifications = memory.check_notifications("ceo_agent")
        
        print(f"  Found {len(notifications)} new notification:")
        for notif in notifications:
            print(f"    [{notif['namespace']:10s}] {notif['content']}")
            print(f"                  importance={notif['importance']:.2f}")
        print()
        
        # 8. ACL: Grant read access to analyst
        print("8. Grant analyst read access to trading namespace")
        print("-" * 60)
        
        # Use system-level grant for demo (in production, namespace owner would have admin rights)
        memory.grant("analyst_agent", "trading", "read", as_system=True)
        print(f"  Granted read permission to analyst_agent")
        print()
        
        # 9. Analyst queries trading namespace
        print("9. Analyst queries trading data")
        print("-" * 60)
        
        memory.set_agent_id("analyst_agent")
        
        results = memory.recall_from_namespace(
            "market news",
            namespace="trading",
            limit=5
        )
        
        print(f"  Found {len(results)} relevant memories:")
        for r in results:
            print(f"    [{r['confidence_label']:10s}] {r['content'][:60]}")
        print()
        
        # 10. Analyst tries to access engine namespace (should fail)
        print("10. Analyst tries to access engine namespace")
        print("-" * 60)
        
        results = memory.recall_from_namespace(
            "system status",
            namespace="engine",
            limit=5
        )
        
        print(f"  Found {len(results)} memories (no permission = no results)")
        print()
        
        # Summary
        print("=" * 60)
        print("Summary")
        print("=" * 60)
        stats = memory.stats()
        print(f"  Total memories: {stats['total_memories']}")
        print(f"  Namespaces: trading, engine")
        print(f"  Agents: trading_agent, engine_agent, ceo_agent, analyst_agent")
        print(f"  Subscriptions: CEO monitoring all namespaces")
        print(f"  ACL: analyst_agent → trading (read)")


if __name__ == "__main__":
    main()
