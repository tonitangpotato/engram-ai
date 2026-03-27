"""
Access Control List (ACL) for Engram v2.

Enables multi-agent shared memory with permission management.
Each namespace can have fine-grained access control (read/write/admin).
"""

from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from typing import Optional
import sqlite3


class Permission(Enum):
    """Access control permission levels for multi-agent memory sharing."""
    READ = "read"      # Can recall memories from this namespace
    WRITE = "write"    # Can store memories to this namespace
    ADMIN = "admin"    # Full control (read + write + grant/revoke)

    def can_read(self) -> bool:
        """Check if this permission includes read access."""
        return self in (Permission.READ, Permission.WRITE, Permission.ADMIN)

    def can_write(self) -> bool:
        """Check if this permission includes write access."""
        return self in (Permission.WRITE, Permission.ADMIN)

    def is_admin(self) -> bool:
        """Check if this permission includes admin access."""
        return self == Permission.ADMIN


@dataclass
class AclEntry:
    """Access control list entry for namespace permissions."""
    agent_id: str
    namespace: str
    permission: Permission
    granted_by: str
    created_at: datetime


class AclManager:
    """Manages access control lists for namespaces."""

    def __init__(self, conn: sqlite3.Connection):
        """
        Create a new ACL manager.

        Args:
            conn: SQLite database connection (shared with Memory)
        """
        self.conn = conn
        self._ensure_table()

    def _ensure_table(self):
        """Ensure the ACL table exists."""
        self.conn.execute("""
            CREATE TABLE IF NOT EXISTS acl (
                agent_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                permission TEXT NOT NULL,
                granted_by TEXT NOT NULL,
                created_at REAL NOT NULL,
                PRIMARY KEY (agent_id, namespace)
            )
        """)
        self.conn.execute("""
            CREATE INDEX IF NOT EXISTS idx_acl_namespace ON acl(namespace)
        """)
        self.conn.commit()

    def grant(
        self,
        agent_id: str,
        namespace: str,
        permission: Permission,
        granted_by: str = "system",
    ) -> None:
        """
        Grant a permission to an agent for a namespace.

        Args:
            agent_id: The agent ID to grant permission to
            namespace: Namespace this permission applies to ("*" = all namespaces)
            permission: Permission level to grant
            granted_by: Agent ID that is granting this permission

        Raises:
            PermissionError: If granted_by doesn't have admin permission
        """
        # Check if grantor has admin permission (unless it's "system")
        if granted_by != "system":
            if not self.check_permission(granted_by, namespace, Permission.ADMIN):
                raise PermissionError(
                    f"Agent {granted_by} does not have admin permission for namespace {namespace}"
                )

        self.conn.execute(
            """
            INSERT OR REPLACE INTO acl (agent_id, namespace, permission, granted_by, created_at)
            VALUES (?, ?, ?, ?, ?)
            """,
            (agent_id, namespace, permission.value, granted_by, datetime.now().timestamp())
        )
        self.conn.commit()

    def revoke(self, agent_id: str, namespace: str) -> bool:
        """
        Revoke a permission from an agent for a namespace.

        Args:
            agent_id: The agent ID to revoke permission from
            namespace: Namespace to revoke permission for

        Returns:
            True if permission was revoked, False if no permission existed
        """
        cursor = self.conn.execute(
            "DELETE FROM acl WHERE agent_id = ? AND namespace = ?",
            (agent_id, namespace)
        )
        self.conn.commit()
        return cursor.rowcount > 0

    def check_permission(
        self,
        agent_id: str,
        namespace: str,
        permission: Permission,
    ) -> bool:
        """
        Check if an agent has a specific permission for a namespace.

        Also checks for wildcard permissions ("*" namespace).

        Args:
            agent_id: The agent ID to check
            namespace: Namespace to check permission for
            permission: Required permission level

        Returns:
            True if agent has the permission, False otherwise
        """
        # Check for exact namespace match or wildcard
        cursor = self.conn.execute(
            """
            SELECT permission FROM acl
            WHERE agent_id = ? AND (namespace = ? OR namespace = '*')
            ORDER BY CASE WHEN namespace = ? THEN 0 ELSE 1 END
            LIMIT 1
            """,
            (agent_id, namespace, namespace)
        )
        row = cursor.fetchone()

        if not row:
            return False

        granted = Permission(row[0])

        # Check if granted permission satisfies required permission
        if permission == Permission.READ:
            return granted.can_read()
        elif permission == Permission.WRITE:
            return granted.can_write()
        elif permission == Permission.ADMIN:
            return granted.is_admin()

        return False

    def list_permissions(self, agent_id: str) -> list[AclEntry]:
        """
        List all permissions for an agent.

        Args:
            agent_id: The agent ID to list permissions for

        Returns:
            List of ACL entries for this agent
        """
        cursor = self.conn.execute(
            """
            SELECT agent_id, namespace, permission, granted_by, created_at
            FROM acl WHERE agent_id = ?
            ORDER BY namespace
            """,
            (agent_id,)
        )

        return [
            AclEntry(
                agent_id=row[0],
                namespace=row[1],
                permission=Permission(row[2]),
                granted_by=row[3],
                created_at=datetime.fromtimestamp(row[4])
            )
            for row in cursor.fetchall()
        ]

    def list_agents(self, namespace: str) -> list[AclEntry]:
        """
        List all agents with access to a namespace.

        Args:
            namespace: The namespace to list agents for

        Returns:
            List of ACL entries for this namespace
        """
        cursor = self.conn.execute(
            """
            SELECT agent_id, namespace, permission, granted_by, created_at
            FROM acl WHERE namespace = ? OR namespace = '*'
            ORDER BY agent_id
            """,
            (namespace,)
        )

        return [
            AclEntry(
                agent_id=row[0],
                namespace=row[1],
                permission=Permission(row[2]),
                granted_by=row[3],
                created_at=datetime.fromtimestamp(row[4])
            )
            for row in cursor.fetchall()
        ]
