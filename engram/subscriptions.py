"""
Subscription and Notification Model for Cross-Agent Intelligence.

Agents can subscribe to namespaces to receive notifications when new
high-importance memories are stored. This enables the CEO pattern where
a supervisor agent monitors all specialist agents without polling.

Example: CEO subscribes to all namespaces with min_importance=0.8
→ Gets notified of high-importance events from any service agent.
"""

from dataclasses import dataclass
from datetime import datetime
from typing import Optional
import sqlite3


@dataclass
class Notification:
    """A notification about a new memory that exceeded a subscription threshold."""
    memory_id: str
    namespace: str
    content: str
    importance: float
    created_at: datetime
    subscription_namespace: str
    threshold: float


@dataclass
class Subscription:
    """A subscription entry."""
    subscriber_id: str
    namespace: str          # "*" = all namespaces
    min_importance: float
    created_at: datetime


class SubscriptionManager:
    """Manages subscriptions and notifications."""

    def __init__(self, conn: sqlite3.Connection):
        """
        Create a new SubscriptionManager.

        Args:
            conn: SQLite database connection (shared with Memory)
        """
        self.conn = conn
        self._ensure_tables()

    def _ensure_tables(self):
        """Initialize subscription tables."""
        self.conn.executescript("""
            CREATE TABLE IF NOT EXISTS subscriptions (
                subscriber_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                min_importance REAL NOT NULL,
                created_at REAL NOT NULL,
                PRIMARY KEY (subscriber_id, namespace)
            );
            
            CREATE TABLE IF NOT EXISTS notification_cursor (
                agent_id TEXT PRIMARY KEY,
                last_checked REAL NOT NULL
            );
            
            CREATE INDEX IF NOT EXISTS idx_subscriptions_ns ON subscriptions(namespace);
        """)
        self.conn.commit()

    def subscribe(
        self,
        agent_id: str,
        namespace: str,
        min_importance: float,
    ) -> None:
        """
        Subscribe an agent to a namespace.

        Args:
            agent_id: The subscribing agent's ID
            namespace: Namespace to watch ("*" for all)
            min_importance: Minimum importance threshold (0.0-1.0)
        """
        clamped = max(0.0, min(1.0, min_importance))

        self.conn.execute(
            """
            INSERT OR REPLACE INTO subscriptions (subscriber_id, namespace, min_importance, created_at)
            VALUES (?, ?, ?, ?)
            """,
            (agent_id, namespace, clamped, datetime.now().timestamp())
        )
        self.conn.commit()

    def unsubscribe(self, agent_id: str, namespace: str) -> bool:
        """
        Unsubscribe an agent from a namespace.

        Args:
            agent_id: The agent ID to unsubscribe
            namespace: Namespace to unsubscribe from

        Returns:
            True if subscription was removed, False if no subscription existed
        """
        cursor = self.conn.execute(
            "DELETE FROM subscriptions WHERE subscriber_id = ? AND namespace = ?",
            (agent_id, namespace)
        )
        self.conn.commit()
        return cursor.rowcount > 0

    def list_subscriptions(self, agent_id: str) -> list[Subscription]:
        """
        List all subscriptions for an agent.

        Args:
            agent_id: The agent ID to list subscriptions for

        Returns:
            List of subscriptions
        """
        cursor = self.conn.execute(
            """
            SELECT subscriber_id, namespace, min_importance, created_at
            FROM subscriptions WHERE subscriber_id = ?
            """,
            (agent_id,)
        )

        return [
            Subscription(
                subscriber_id=row[0],
                namespace=row[1],
                min_importance=row[2],
                created_at=datetime.fromtimestamp(row[3])
            )
            for row in cursor.fetchall()
        ]

    def _query_notifications_for_sub(
        self,
        sub: Subscription,
        since: Optional[datetime] = None,
    ) -> list[Notification]:
        """Helper to query notifications for a subscription."""
        notifications = []

        if sub.namespace == "*":
            # All namespaces
            if since:
                cursor = self.conn.execute(
                    """
                    SELECT id, namespace, content, importance, created_at
                    FROM memories
                    WHERE created_at > ? AND importance >= ?
                    """,
                    (since.timestamp(), sub.min_importance)
                )
            else:
                cursor = self.conn.execute(
                    """
                    SELECT id, namespace, content, importance, created_at
                    FROM memories
                    WHERE importance >= ?
                    """,
                    (sub.min_importance,)
                )
        else:
            # Specific namespace
            if since:
                cursor = self.conn.execute(
                    """
                    SELECT id, namespace, content, importance, created_at
                    FROM memories
                    WHERE created_at > ? AND importance >= ? AND namespace = ?
                    """,
                    (since.timestamp(), sub.min_importance, sub.namespace)
                )
            else:
                cursor = self.conn.execute(
                    """
                    SELECT id, namespace, content, importance, created_at
                    FROM memories
                    WHERE importance >= ? AND namespace = ?
                    """,
                    (sub.min_importance, sub.namespace)
                )

        for row in cursor.fetchall():
            notifications.append(
                Notification(
                    memory_id=row[0],
                    namespace=row[1],
                    content=row[2],
                    importance=row[3],
                    created_at=datetime.fromtimestamp(row[4]),
                    subscription_namespace=sub.namespace,
                    threshold=sub.min_importance,
                )
            )

        return notifications

    def check_notifications(self, agent_id: str) -> list[Notification]:
        """
        Check for notifications since last check.

        Returns new memories that exceed the subscription thresholds.
        Updates the cursor so the same notifications aren't returned twice.

        Args:
            agent_id: The agent ID to check notifications for

        Returns:
            List of new notifications
        """
        # Get last checked timestamp
        cursor = self.conn.execute(
            "SELECT last_checked FROM notification_cursor WHERE agent_id = ?",
            (agent_id,)
        )
        row = cursor.fetchone()
        last_checked = datetime.fromtimestamp(row[0]) if row else None

        # Get agent's subscriptions
        subscriptions = self.list_subscriptions(agent_id)

        if not subscriptions:
            return []

        notifications = []
        now = datetime.now()

        for sub in subscriptions:
            sub_notifs = self._query_notifications_for_sub(sub, last_checked)
            notifications.extend(sub_notifs)

        # Update cursor
        self.conn.execute(
            "INSERT OR REPLACE INTO notification_cursor (agent_id, last_checked) VALUES (?, ?)",
            (agent_id, now.timestamp())
        )
        self.conn.commit()

        # Deduplicate by memory_id (in case multiple subscriptions match same memory)
        seen = set()
        unique_notifs = []
        for notif in notifications:
            if notif.memory_id not in seen:
                seen.add(notif.memory_id)
                unique_notifs.append(notif)

        # Sort by created_at descending
        unique_notifs.sort(key=lambda n: n.created_at, reverse=True)

        return unique_notifs

    def peek_notifications(self, agent_id: str) -> list[Notification]:
        """
        Peek at notifications without updating cursor.

        Args:
            agent_id: The agent ID to peek notifications for

        Returns:
            List of notifications (cursor is NOT updated)
        """
        # Get last checked timestamp
        cursor = self.conn.execute(
            "SELECT last_checked FROM notification_cursor WHERE agent_id = ?",
            (agent_id,)
        )
        row = cursor.fetchone()
        last_checked = datetime.fromtimestamp(row[0]) if row else None

        subscriptions = self.list_subscriptions(agent_id)

        if not subscriptions:
            return []

        notifications = []

        for sub in subscriptions:
            sub_notifs = self._query_notifications_for_sub(sub, last_checked)
            notifications.extend(sub_notifs)

        # Deduplicate
        seen = set()
        unique_notifs = []
        for notif in notifications:
            if notif.memory_id not in seen:
                seen.add(notif.memory_id)
                unique_notifs.append(notif)

        # Sort by created_at descending
        unique_notifs.sort(key=lambda n: n.created_at, reverse=True)

        return unique_notifs

    def reset_cursor(self, agent_id: str) -> None:
        """
        Reset notification cursor (useful for testing or re-checking everything).

        Args:
            agent_id: The agent ID to reset cursor for
        """
        self.conn.execute(
            "DELETE FROM notification_cursor WHERE agent_id = ?",
            (agent_id,)
        )
        self.conn.commit()
