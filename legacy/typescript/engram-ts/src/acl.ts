/**
 * Access Control List (ACL) for multi-agent memory sharing
 */

import Database from 'better-sqlite3';
import { Permission, AclEntry } from './types';

/**
 * Initialize ACL tables in the database
 */
export function initAclTables(db: Database.Database): void {
  db.exec(`
    CREATE TABLE IF NOT EXISTS acl_entries (
      agent_id TEXT NOT NULL,
      namespace TEXT NOT NULL,
      permission TEXT NOT NULL,
      granted_by TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      PRIMARY KEY (agent_id, namespace)
    );
    
    CREATE INDEX IF NOT EXISTS idx_acl_agent ON acl_entries(agent_id);
    CREATE INDEX IF NOT EXISTS idx_acl_namespace ON acl_entries(namespace);
  `);
}

/**
 * Grant a permission to an agent for a namespace
 */
export function grantPermission(
  db: Database.Database,
  agentId: string,
  namespace: string,
  permission: Permission,
  grantedBy: string,
): void {
  const stmt = db.prepare(`
    INSERT OR REPLACE INTO acl_entries (agent_id, namespace, permission, granted_by, created_at)
    VALUES (?, ?, ?, ?, ?)
  `);
  
  stmt.run(agentId, namespace, permission, grantedBy, Date.now() / 1000);
}

/**
 * Revoke a permission from an agent for a namespace
 */
export function revokePermission(
  db: Database.Database,
  agentId: string,
  namespace: string,
): boolean {
  const stmt = db.prepare('DELETE FROM acl_entries WHERE agent_id = ? AND namespace = ?');
  const result = stmt.run(agentId, namespace);
  return result.changes > 0;
}

/**
 * Check if an agent has a specific permission for a namespace
 */
export function checkPermission(
  db: Database.Database,
  agentId: string,
  namespace: string,
  permission: Permission,
): boolean {
  // Check for wildcard admin permission first
  const wildcardStmt = db.prepare(
    "SELECT permission FROM acl_entries WHERE agent_id = ? AND namespace = '*'"
  );
  const wildcardRow = wildcardStmt.get(agentId) as { permission: string } | undefined;
  
  if (wildcardRow && wildcardRow.permission === Permission.ADMIN) {
    return true; // Wildcard admin has all permissions
  }
  
  // Check specific namespace permission
  const stmt = db.prepare(
    'SELECT permission FROM acl_entries WHERE agent_id = ? AND namespace = ?'
  );
  const row = stmt.get(agentId, namespace) as { permission: string } | undefined;
  
  if (!row) {
    return false;
  }
  
  const grantedPermission = row.permission as Permission;
  
  // Permission hierarchy check
  switch (permission) {
    case Permission.READ:
      return [Permission.READ, Permission.WRITE, Permission.ADMIN].includes(grantedPermission);
    case Permission.WRITE:
      return [Permission.WRITE, Permission.ADMIN].includes(grantedPermission);
    case Permission.ADMIN:
      return grantedPermission === Permission.ADMIN;
    default:
      return false;
  }
}

/**
 * List all permissions for an agent
 */
export function listPermissions(db: Database.Database, agentId: string): AclEntry[] {
  const stmt = db.prepare(
    'SELECT agent_id, namespace, permission, granted_by, created_at FROM acl_entries WHERE agent_id = ?'
  );
  
  const rows = stmt.all(agentId) as Array<{
    agent_id: string;
    namespace: string;
    permission: string;
    granted_by: string;
    created_at: number;
  }>;
  
  return rows.map(row => ({
    agentId: row.agent_id,
    namespace: row.namespace,
    permission: row.permission as Permission,
    grantedBy: row.granted_by,
    createdAt: row.created_at,
  }));
}
