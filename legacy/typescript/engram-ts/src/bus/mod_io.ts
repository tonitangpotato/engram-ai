/**
 * Module Reader/Writer — Parse and update agent workspace files
 * Handles SOUL.md, HEARTBEAT.md, and IDENTITY.md with structure-preserving updates
 */

import * as fs from 'fs';
import * as path from 'path';

/**
 * A drive/priority extracted from SOUL.md
 */
export interface Drive {
  /** The drive name/key (e.g., "curiosity", "helpfulness") */
  name: string;
  /** The drive description/value */
  description: string;
  /** Keywords for alignment matching */
  keywords: string[];
}

/**
 * A task from HEARTBEAT.md with completion status
 */
export interface HeartbeatTask {
  /** Task description */
  description: string;
  /** Whether the task is completed (checkbox checked) */
  completed: boolean;
  /** Original line for preservation */
  originalLine: string;
}

/**
 * Identity fields from IDENTITY.md
 */
export interface Identity {
  name?: string;
  creature?: string;
  vibe?: string;
  emoji?: string;
}

/**
 * Extract keywords from a drive name and description
 */
export function extractKeywords(drive: Drive): string[] {
  const keywords: string[] = [];

  // Add name as keyword
  keywords.push(drive.name.toLowerCase());

  // Extract significant words from description (3+ chars, not stopwords)
  const stopwords = ['the', 'and', 'for', 'with', 'that', 'this', 'from', 'are', 'was', 'but'];
  for (const word of drive.description.split(/\s+/)) {
    const clean = word.replace(/[^a-zA-Z0-9]/g, '').toLowerCase();
    if (clean.length >= 3 && !stopwords.includes(clean)) {
      keywords.push(clean);
    }
  }

  // Deduplicate
  return Array.from(new Set(keywords)).sort();
}

/**
 * Parse SOUL.md to extract drives/priorities
 */
export function parseSoul(content: string): Drive[] {
  const drives: Drive[] = [];
  let currentSection = '';

  for (const line of content.split('\n')) {
    const trimmed = line.trim();

    // Track section headers
    if (trimmed.startsWith('#')) {
      currentSection = trimmed.replace(/^#+\s*/, '');
      continue;
    }

    // Skip empty lines
    if (!trimmed) continue;

    // Parse key: value pairs
    const colonIdx = trimmed.indexOf(':');
    if (colonIdx !== -1) {
      const key = trimmed.substring(0, colonIdx).trim();
      const value = trimmed.substring(colonIdx + 1).trim();

      // Skip if key looks like a URL or is empty
      if (!key.includes('/') && key && value) {
        const drive: Drive = {
          name: key,
          description: value,
          keywords: [],
        };
        drive.keywords = extractKeywords(drive);
        drives.push(drive);
        continue;
      }
    }

    // Parse bullet points
    if (trimmed.startsWith('-') || trimmed.startsWith('*')) {
      const item = trimmed.substring(1).trim();
      if (item) {
        const name = currentSection
          ? `${currentSection}/${item.split(/\s+/).slice(0, 3).join(' ')}`
          : item.split(/\s+/).slice(0, 3).join(' ');

        const drive: Drive = {
          name,
          description: item,
          keywords: [],
        };
        drive.keywords = extractKeywords(drive);
        drives.push(drive);
      }
    }
  }

  return drives;
}

/**
 * Parse HEARTBEAT.md to extract tasks with completion status
 */
export function parseHeartbeat(content: string): HeartbeatTask[] {
  const tasks: HeartbeatTask[] = [];

  for (const line of content.split('\n')) {
    const trimmed = line.trim();

    // Parse checkbox items
    if (trimmed.startsWith('- [')) {
      const bracketEnd = trimmed.indexOf(']', 3);
      if (bracketEnd !== -1) {
        const checkboxContent = trimmed.substring(3, bracketEnd);
        const completed = checkboxContent.toLowerCase() === 'x';
        const description = trimmed.substring(bracketEnd + 1).trim();

        if (description) {
          tasks.push({
            description,
            completed,
            originalLine: line,
          });
        }
      }
    }
  }

  return tasks;
}

/**
 * Parse IDENTITY.md to extract identity fields
 */
export function parseIdentity(content: string): Identity {
  const identity: Identity = {};

  for (const line of content.split('\n')) {
    const colonIdx = line.indexOf(':');
    if (colonIdx === -1) continue;

    const key = line.substring(0, colonIdx).trim().toLowerCase();
    const value = line.substring(colonIdx + 1).trim();

    if (!value) continue;

    switch (key) {
      case 'name':
        identity.name = value;
        break;
      case 'creature':
        identity.creature = value;
        break;
      case 'vibe':
        identity.vibe = value;
        break;
      case 'emoji':
        identity.emoji = value;
        break;
    }
  }

  return identity;
}

/**
 * Read and parse SOUL.md from workspace directory
 */
export function readSoul(workspaceDir: string): Drive[] {
  const filePath = path.join(workspaceDir, 'SOUL.md');
  if (!fs.existsSync(filePath)) {
    return [];
  }
  const content = fs.readFileSync(filePath, 'utf-8');
  return parseSoul(content);
}

/**
 * Read and parse HEARTBEAT.md from workspace directory
 */
export function readHeartbeat(workspaceDir: string): HeartbeatTask[] {
  const filePath = path.join(workspaceDir, 'HEARTBEAT.md');
  if (!fs.existsSync(filePath)) {
    return [];
  }
  const content = fs.readFileSync(filePath, 'utf-8');
  return parseHeartbeat(content);
}

/**
 * Read and parse IDENTITY.md from workspace directory
 */
export function readIdentity(workspaceDir: string): Identity {
  const filePath = path.join(workspaceDir, 'IDENTITY.md');
  if (!fs.existsSync(filePath)) {
    return {};
  }
  const content = fs.readFileSync(filePath, 'utf-8');
  return parseIdentity(content);
}

/**
 * Update a specific field in SOUL.md (key: value pair)
 */
export function updateSoulField(
  workspaceDir: string,
  key: string,
  newValue: string,
): boolean {
  const filePath = path.join(workspaceDir, 'SOUL.md');
  if (!fs.existsSync(filePath)) {
    return false;
  }

  const content = fs.readFileSync(filePath, 'utf-8');
  const lines = content.split('\n');
  let updated = false;

  for (let i = 0; i < lines.length; i++) {
    const colonIdx = lines[i].indexOf(':');
    if (colonIdx !== -1) {
      const lineKey = lines[i].substring(0, colonIdx).trim();
      if (lineKey.toLowerCase() === key.toLowerCase()) {
        lines[i] = `${lineKey}: ${newValue}`;
        updated = true;
        break;
      }
    }
  }

  if (updated) {
    fs.writeFileSync(filePath, lines.join('\n'));
  }

  return updated;
}

/**
 * Add a new drive to SOUL.md at the end
 */
export function addSoulDrive(workspaceDir: string, key: string, value: string): void {
  const filePath = path.join(workspaceDir, 'SOUL.md');
  let content = fs.existsSync(filePath) ? fs.readFileSync(filePath, 'utf-8') : '';

  if (content && !content.endsWith('\n')) {
    content += '\n';
  }
  content += `${key}: ${value}\n`;

  fs.writeFileSync(filePath, content);
}

/**
 * Update a task completion status in HEARTBEAT.md
 */
export function updateHeartbeatTask(
  workspaceDir: string,
  taskDescription: string,
  completed: boolean,
): boolean {
  const filePath = path.join(workspaceDir, 'HEARTBEAT.md');
  if (!fs.existsSync(filePath)) {
    return false;
  }

  const content = fs.readFileSync(filePath, 'utf-8');
  const lines = content.split('\n');
  let updated = false;

  const checkboxMark = completed ? 'x' : ' ';

  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (trimmed.startsWith('- [')) {
      const bracketEnd = trimmed.indexOf(']', 3);
      if (bracketEnd !== -1) {
        const desc = trimmed.substring(bracketEnd + 1).trim();
        if (desc.toLowerCase() === taskDescription.toLowerCase()) {
          // Preserve indentation
          const indent = lines[i].substring(0, lines[i].length - trimmed.length);
          lines[i] = `${indent}- [${checkboxMark}] ${desc}`;
          updated = true;
          break;
        }
      }
    }
  }

  if (updated) {
    fs.writeFileSync(filePath, lines.join('\n'));
  }

  return updated;
}

/**
 * Add a new task to HEARTBEAT.md
 */
export function addHeartbeatTask(workspaceDir: string, description: string): void {
  const filePath = path.join(workspaceDir, 'HEARTBEAT.md');
  let content = fs.existsSync(filePath) ? fs.readFileSync(filePath, 'utf-8') : '';

  if (content && !content.endsWith('\n')) {
    content += '\n';
  }
  content += `- [ ] ${description}\n`;

  fs.writeFileSync(filePath, content);
}

/**
 * Update a field in IDENTITY.md
 */
export function updateIdentityField(
  workspaceDir: string,
  field: string,
  newValue: string,
): boolean {
  const filePath = path.join(workspaceDir, 'IDENTITY.md');
  if (!fs.existsSync(filePath)) {
    return false;
  }

  const content = fs.readFileSync(filePath, 'utf-8');
  const lines = content.split('\n');
  let updated = false;

  for (let i = 0; i < lines.length; i++) {
    const colonIdx = lines[i].indexOf(':');
    if (colonIdx !== -1) {
      const lineKey = lines[i].substring(0, colonIdx).trim();
      if (lineKey.toLowerCase() === field.toLowerCase()) {
        lines[i] = `${lineKey}: ${newValue}`;
        updated = true;
        break;
      }
    }
  }

  if (updated) {
    fs.writeFileSync(filePath, lines.join('\n'));
  }

  return updated;
}
