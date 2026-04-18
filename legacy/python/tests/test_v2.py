"""
Tests for Engram v2 features:
- Namespace isolation
- ACL (Access Control)
- Emotional Bus
- Cross-agent subscriptions
"""

import pytest
import sqlite3
from pathlib import Path
from tempfile import TemporaryDirectory

from engram.acl import AclManager, Permission
from engram.subscriptions import SubscriptionManager
from engram.bus import (
    EmotionalAccumulator,
    BehaviorFeedback,
    EmotionalBus,
    parse_soul,
    parse_heartbeat,
    parse_identity,
    Drive,
)
from engram.store import SQLiteStore


class TestACL:
    def test_grant_revoke(self):
        conn = sqlite3.connect(":memory:")
        acl = AclManager(conn)

        # Grant read permission
        acl.grant("agent1", "trading", Permission.READ)
        assert acl.check_permission("agent1", "trading", Permission.READ)
        assert not acl.check_permission("agent1", "trading", Permission.WRITE)

        # Upgrade to write
        acl.grant("agent1", "trading", Permission.WRITE)
        assert acl.check_permission("agent1", "trading", Permission.READ)
        assert acl.check_permission("agent1", "trading", Permission.WRITE)

        # Revoke
        assert acl.revoke("agent1", "trading")
        assert not acl.check_permission("agent1", "trading", Permission.READ)

    def test_wildcard_permission(self):
        conn = sqlite3.connect(":memory:")
        acl = AclManager(conn)

        # Grant wildcard admin
        acl.grant("admin_agent", "*", Permission.ADMIN)

        # Should work for any namespace
        assert acl.check_permission("admin_agent", "trading", Permission.READ)
        assert acl.check_permission("admin_agent", "engine", Permission.WRITE)
        assert acl.check_permission("admin_agent", "anything", Permission.ADMIN)

    def test_permission_hierarchy(self):
        conn = sqlite3.connect(":memory:")
        acl = AclManager(conn)

        acl.grant("agent1", "ns1", Permission.WRITE)

        # Write includes read
        assert acl.check_permission("agent1", "ns1", Permission.READ)
        assert acl.check_permission("agent1", "ns1", Permission.WRITE)
        # But not admin
        assert not acl.check_permission("agent1", "ns1", Permission.ADMIN)


class TestSubscriptions:
    def test_subscribe_unsubscribe(self):
        conn = sqlite3.connect(":memory:")
        # Create memories table for testing
        conn.execute("""
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                importance REAL NOT NULL,
                created_at REAL NOT NULL,
                namespace TEXT NOT NULL DEFAULT 'default'
            )
        """)

        mgr = SubscriptionManager(conn)

        # Subscribe
        mgr.subscribe("ceo", "trading", 0.8)
        subs = mgr.list_subscriptions("ceo")
        assert len(subs) == 1
        assert subs[0].namespace == "trading"
        assert abs(subs[0].min_importance - 0.8) < 0.01

        # Unsubscribe
        assert mgr.unsubscribe("ceo", "trading")
        subs = mgr.list_subscriptions("ceo")
        assert len(subs) == 0

    def test_notifications_basic(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("""
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                importance REAL NOT NULL,
                created_at REAL NOT NULL,
                namespace TEXT NOT NULL DEFAULT 'default'
            )
        """)

        mgr = SubscriptionManager(conn)

        # Subscribe
        mgr.subscribe("ceo", "trading", 0.7)

        # Add a high-importance memory
        from datetime import datetime
        conn.execute(
            "INSERT INTO memories (id, content, importance, created_at, namespace) VALUES (?, ?, ?, ?, ?)",
            ("m1", "Oil price spike", 0.9, datetime.now().timestamp(), "trading")
        )
        conn.commit()

        # Check notifications
        notifs = mgr.check_notifications("ceo")
        assert len(notifs) == 1
        assert notifs[0].memory_id == "m1"
        assert notifs[0].namespace == "trading"

        # Check again - should be empty (cursor updated)
        notifs = mgr.check_notifications("ceo")
        assert len(notifs) == 0

    def test_notifications_wildcard(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("""
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                importance REAL NOT NULL,
                created_at REAL NOT NULL,
                namespace TEXT NOT NULL DEFAULT 'default'
            )
        """)

        mgr = SubscriptionManager(conn)

        # Subscribe to all namespaces
        mgr.subscribe("ceo", "*", 0.8)

        # Add memories to different namespaces
        from datetime import datetime
        now = datetime.now().timestamp()
        conn.execute(
            "INSERT INTO memories VALUES (?, ?, ?, ?, ?)",
            ("m1", "Trading alert", 0.9, now, "trading")
        )
        conn.execute(
            "INSERT INTO memories VALUES (?, ?, ?, ?, ?)",
            ("m2", "Engine alert", 0.85, now, "engine")
        )
        conn.commit()

        notifs = mgr.check_notifications("ceo")
        assert len(notifs) == 2


class TestEmotionalAccumulator:
    def test_record_emotion(self):
        conn = sqlite3.connect(":memory:")
        acc = EmotionalAccumulator(conn)

        # Record some emotions
        acc.record_emotion("coding", 0.8)
        acc.record_emotion("coding", 0.6)
        acc.record_emotion("coding", 0.4)

        trend = acc.get_trend("coding")
        assert trend is not None
        assert trend.count == 3
        # Average of 0.8, 0.6, 0.4 = 0.6
        assert abs(trend.valence - 0.6) < 0.01

    def test_negative_trend_flags_update(self):
        conn = sqlite3.connect(":memory:")
        acc = EmotionalAccumulator(conn)

        # Record many negative emotions
        for _ in range(12):
            acc.record_emotion("debugging", -0.7)

        trend = acc.get_trend("debugging")
        assert trend.needs_soul_update()
        assert trend.valence < -0.5

    def test_decay_trends(self):
        conn = sqlite3.connect(":memory:")
        acc = EmotionalAccumulator(conn)

        acc.record_emotion("test", 0.8)
        acc.decay_trends(0.9)

        trend = acc.get_trend("test")
        assert abs(trend.valence - 0.72) < 0.01  # 0.8 * 0.9


class TestBehaviorFeedback:
    def test_log_outcome(self):
        conn = sqlite3.connect(":memory:")
        feedback = BehaviorFeedback(conn)

        # Log some outcomes
        feedback.log_outcome("check_email", True)
        feedback.log_outcome("check_email", True)
        feedback.log_outcome("check_email", False)
        feedback.log_outcome("check_email", True)

        score = feedback.get_action_score("check_email")
        assert score is not None
        # 3 positive out of 4 = 0.75
        assert abs(score - 0.75) < 0.01

    def test_deprioritize_suggestion(self):
        conn = sqlite3.connect(":memory:")
        feedback = BehaviorFeedback(conn)

        # Log many failures
        for _ in range(12):
            feedback.log_outcome("bad_action", False)

        stats = feedback.get_action_stats("bad_action")
        assert stats.should_deprioritize()
        assert stats.score < 0.2

    def test_successful_actions(self):
        conn = sqlite3.connect(":memory:")
        feedback = BehaviorFeedback(conn)

        # Log many successes
        for _ in range(12):
            feedback.log_outcome("good_action", True)

        successful = feedback.get_successful_actions(0.8)
        assert len(successful) == 1
        assert successful[0].action == "good_action"


class TestWorkspaceIO:
    def test_parse_soul(self):
        content = """
# Core Drives
curiosity: Always seek to understand more
helpfulness: Assist the user effectively

# Secondary
patience: Wait for the right moment
"""
        drives = parse_soul(content)
        assert len(drives) == 3
        assert any(d.name == "curiosity" for d in drives)
        assert any("understand" in d.description for d in drives)

    def test_parse_heartbeat(self):
        content = """
# Daily Tasks
- [ ] Check emails
- [x] Review calendar
- [ ] Run consolidation
"""
        tasks = parse_heartbeat(content)
        assert len(tasks) == 3
        assert not tasks[0].completed
        assert tasks[1].completed
        assert tasks[0].description == "Check emails"

    def test_parse_identity(self):
        content = """
name: Clawd
creature: Cat
vibe: curious and playful
emoji: 🐱
"""
        identity = parse_identity(content)
        assert identity.name == "Clawd"
        assert identity.creature == "Cat"
        assert identity.emoji == "🐱"

    def test_drive_alignment(self):
        from engram.bus import score_alignment, calculate_importance_boost

        drives = [
            Drive(name="curiosity", description="Always seek to understand new concepts"),
        ]
        drives[0].keywords = drives[0].extract_keywords()

        # Aligned content
        aligned = "I want to understand these new concepts"
        score = score_alignment(aligned, drives)
        assert score > 0.5

        boost = calculate_importance_boost(aligned, drives)
        assert boost > 1.0

        # Unaligned content
        unaligned = "xyz abc 123"
        score = score_alignment(unaligned, drives)
        assert score == 0.0

        boost = calculate_importance_boost(unaligned, drives)
        assert boost == 1.0


class TestEmotionalBus:
    def test_bus_creation(self):
        with TemporaryDirectory() as tmpdir:
            workspace = Path(tmpdir)

            # Create SOUL.md
            (workspace / "SOUL.md").write_text("""
# Core Drives
curiosity: Always seek to understand new things
helpfulness: Assist the user effectively
""")

            conn = sqlite3.connect(":memory:")
            bus = EmotionalBus(str(workspace), conn)

            assert len(bus.drives) >= 2
            assert any(d.name == "curiosity" for d in bus.drives)

    def test_importance_alignment(self):
        with TemporaryDirectory() as tmpdir:
            workspace = Path(tmpdir)

            (workspace / "SOUL.md").write_text("""
curiosity: Always seek to understand and learn new things
""")

            conn = sqlite3.connect(":memory:")
            bus = EmotionalBus(str(workspace), conn)

            # Content aligned with "curiosity"
            aligned = "I want to understand and learn new things"
            boost = bus.align_importance(aligned)
            assert boost > 1.0

            # Unaligned content
            unaligned = "xyz 123 abc"
            boost = bus.align_importance(unaligned)
            assert boost == 1.0

    def test_process_interaction(self):
        with TemporaryDirectory() as tmpdir:
            workspace = Path(tmpdir)
            (workspace / "SOUL.md").write_text("")

            conn = sqlite3.connect(":memory:")
            bus = EmotionalBus(str(workspace), conn)

            # Record interactions
            bus.process_interaction("test content", 0.8, "coding")
            bus.process_interaction("test content", 0.6, "coding")

            trends = bus.get_trends()
            assert len(trends) == 1
            assert trends[0].domain == "coding"
            assert abs(trends[0].valence - 0.7) < 0.01

    def test_suggest_soul_updates(self):
        with TemporaryDirectory() as tmpdir:
            workspace = Path(tmpdir)
            (workspace / "SOUL.md").write_text("")

            conn = sqlite3.connect(":memory:")
            bus = EmotionalBus(str(workspace), conn)

            # Record many negative interactions
            for _ in range(15):
                bus.process_interaction("bad experience", -0.8, "debugging")

            suggestions = bus.suggest_soul_updates()
            assert len(suggestions) > 0
            assert any(s.domain == "debugging" for s in suggestions)

    def test_suggest_heartbeat_updates(self):
        with TemporaryDirectory() as tmpdir:
            workspace = Path(tmpdir)
            (workspace / "SOUL.md").write_text("")

            conn = sqlite3.connect(":memory:")
            bus = EmotionalBus(str(workspace), conn)

            # Log many failures
            for _ in range(15):
                bus.log_behavior("useless_check", False)

            suggestions = bus.suggest_heartbeat_updates()
            assert len(suggestions) > 0
            assert any(s.action == "useless_check" for s in suggestions)
            assert any(s.suggestion == "deprioritize" for s in suggestions)


class TestNamespaceIsolation:
    def test_namespace_storage(self):
        store = SQLiteStore(":memory:")

        # Add to different namespaces
        from engram.core import MemoryType
        m1 = store.add("Trading data", MemoryType.FACTUAL, namespace="trading")
        m2 = store.add("Engine data", MemoryType.FACTUAL, namespace="engine")
        m3 = store.add("Default data", MemoryType.FACTUAL, namespace="default")

        # Query by namespace
        trading_mems = store.all_in_namespace("trading")
        assert len(trading_mems) == 1
        assert trading_mems[0].id == m1.id

        # Query all
        all_mems = store.all_in_namespace("*")
        assert len(all_mems) == 3

    def test_namespace_search(self):
        store = SQLiteStore(":memory:")

        from engram.core import MemoryType
        store.add("Oil price spike", MemoryType.FACTUAL, namespace="trading")
        store.add("Engine temperature high", MemoryType.FACTUAL, namespace="engine")
        store.add("Oil leak detected", MemoryType.FACTUAL, namespace="engine")

        # Search in trading namespace
        results = store.search_fts_ns("oil", limit=10, namespace="trading")
        assert len(results) == 1

        # Search in engine namespace
        results = store.search_fts_ns("oil", limit=10, namespace="engine")
        assert len(results) == 1

        # Search all namespaces
        results = store.search_fts_ns("oil", limit=10, namespace="*")
        assert len(results) == 2


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
