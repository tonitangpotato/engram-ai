/**
 * Core types for Engram v2 features
 */

import { MemoryEntry } from './core';

/**
 * Access control permission levels for multi-agent memory sharing
 */
export enum Permission {
  /** Read access: can recall memories from this namespace */
  READ = 'read',
  /** Write access: can store memories to this namespace */
  WRITE = 'write',
  /** Admin access: full control (read + write + grant/revoke) */
  ADMIN = 'admin',
}

/**
 * Access control list entry for namespace permissions
 */
export interface AclEntry {
  /** Agent ID that has this permission */
  agentId: string;
  /** Namespace this permission applies to ("*" = all namespaces) */
  namespace: string;
  /** Permission level */
  permission: Permission;
  /** Agent ID that granted this permission */
  grantedBy: string;
  /** When this permission was granted */
  createdAt: number;
}

/**
 * A Hebbian link between memories from different namespaces
 */
export interface CrossLink {
  /** Source memory ID */
  sourceId: string;
  /** Source namespace */
  sourceNs: string;
  /** Target memory ID */
  targetId: string;
  /** Target namespace */
  targetNs: string;
  /** Link strength (0.0-1.0) */
  strength: number;
  /** Optional description or context */
  description?: string;
}

/**
 * Search result with activation score and confidence
 */
export interface RecallResult {
  entry: MemoryEntry;
  activation: number;
  confidence: number;
  confidenceLabel: string;
}

/**
 * Result of recall with cross-namespace associations
 */
export interface RecallWithAssociationsResult {
  /** Main recall results */
  memories: RecallResult[];
  /** Cross-namespace associations found */
  crossLinks: CrossLink[];
}
