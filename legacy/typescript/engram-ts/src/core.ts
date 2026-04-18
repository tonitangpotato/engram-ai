/**
 * Core memory data structures.
 */

function shortId(): string {
  return Math.random().toString(36).substring(2, 10);
}

export enum MemoryType {
  FACTUAL = 'factual',
  EPISODIC = 'episodic',
  RELATIONAL = 'relational',
  EMOTIONAL = 'emotional',
  PROCEDURAL = 'procedural',
  OPINION = 'opinion',
  CAUSAL = 'causal',
}

export enum MemoryLayer {
  L2_CORE = 'core',
  L3_WORKING = 'working',
  L4_ARCHIVE = 'archive',
}

export const DEFAULT_DECAY_RATES: Record<MemoryType, number> = {
  [MemoryType.FACTUAL]: 0.03,
  [MemoryType.EPISODIC]: 0.10,
  [MemoryType.RELATIONAL]: 0.02,
  [MemoryType.EMOTIONAL]: 0.01,
  [MemoryType.PROCEDURAL]: 0.01,
  [MemoryType.OPINION]: 0.05,
  [MemoryType.CAUSAL]: 0.02,
};

export const DEFAULT_IMPORTANCE: Record<MemoryType, number> = {
  [MemoryType.FACTUAL]: 0.3,
  [MemoryType.EPISODIC]: 0.4,
  [MemoryType.RELATIONAL]: 0.6,
  [MemoryType.EMOTIONAL]: 0.9,
  [MemoryType.PROCEDURAL]: 0.5,
  [MemoryType.OPINION]: 0.3,
  [MemoryType.CAUSAL]: 0.7,
};

export interface MemoryEntryData {
  id: string;
  content: string;
  summary: string;
  type: string;
  layer: string;
  created_at: number;
  access_times: number[];
  working_strength: number;
  core_strength: number;
  importance: number;
  pinned: boolean;
  consolidation_count: number;
  last_consolidated: number | null;
  source_file: string;
  graph_node_ids: string[];
  contradicts: string;
  contradicted_by: string;
  metadata?: Record<string, unknown> | null;
}

export class MemoryEntry {
  id: string;
  content: string;
  summary: string;
  memoryType: MemoryType;
  layer: MemoryLayer;
  createdAt: number;
  accessTimes: number[];
  workingStrength: number;
  coreStrength: number;
  importance: number;
  pinned: boolean;
  consolidationCount: number;
  lastConsolidated: number | null;
  sourceFile: string;
  sourceLine: number;
  contradicts: string;
  contradictedBy: string;
  graphNodeIds: string[];
  metadata: Record<string, unknown> | null;

  constructor(opts: Partial<{
    id: string;
    content: string;
    summary: string;
    memoryType: MemoryType;
    layer: MemoryLayer;
    createdAt: number;
    accessTimes: number[];
    workingStrength: number;
    coreStrength: number;
    importance: number;
    pinned: boolean;
    consolidationCount: number;
    lastConsolidated: number | null;
    sourceFile: string;
    sourceLine: number;
    contradicts: string;
    contradictedBy: string;
    graphNodeIds: string[];
    metadata: Record<string, unknown> | null;
  }> = {}) {
    this.id = opts.id ?? shortId();
    this.content = opts.content ?? '';
    this.summary = opts.summary ?? '';
    this.memoryType = opts.memoryType ?? MemoryType.FACTUAL;
    this.layer = opts.layer ?? MemoryLayer.L3_WORKING;
    this.createdAt = opts.createdAt ?? Date.now() / 1000;
    this.accessTimes = opts.accessTimes ?? [];
    this.workingStrength = opts.workingStrength ?? 1.0;
    this.coreStrength = opts.coreStrength ?? 0.0;
    this.importance = opts.importance ?? 0.3;
    this.pinned = opts.pinned ?? false;
    this.consolidationCount = opts.consolidationCount ?? 0;
    this.lastConsolidated = opts.lastConsolidated ?? null;
    this.sourceFile = opts.sourceFile ?? '';
    this.sourceLine = opts.sourceLine ?? 0;
    this.contradicts = opts.contradicts ?? '';
    this.contradictedBy = opts.contradictedBy ?? '';
    this.graphNodeIds = opts.graphNodeIds ?? [];
    this.metadata = opts.metadata ?? null;
  }

  recordAccess(): void {
    this.accessTimes.push(Date.now() / 1000);
  }

  ageHours(): number {
    return (Date.now() / 1000 - this.createdAt) / 3600;
  }

  ageDays(): number {
    return this.ageHours() / 24;
  }

  toDict(): MemoryEntryData {
    const d: MemoryEntryData = {
      id: this.id,
      content: this.content,
      summary: this.summary,
      type: this.memoryType,
      layer: this.layer,
      created_at: this.createdAt,
      access_times: this.accessTimes,
      working_strength: this.workingStrength,
      core_strength: this.coreStrength,
      importance: this.importance,
      pinned: this.pinned,
      consolidation_count: this.consolidationCount,
      last_consolidated: this.lastConsolidated,
      source_file: this.sourceFile,
      graph_node_ids: this.graphNodeIds,
      contradicts: this.contradicts,
      contradicted_by: this.contradictedBy,
    };
    if (this.metadata !== null) {
      d.metadata = this.metadata;
    }
    return d;
  }

  static fromDict(d: MemoryEntryData): MemoryEntry {
    return new MemoryEntry({
      id: d.id,
      content: d.content,
      summary: d.summary ?? '',
      memoryType: d.type as MemoryType,
      layer: d.layer as MemoryLayer,
      createdAt: d.created_at,
      accessTimes: d.access_times ?? [],
      workingStrength: d.working_strength ?? 1.0,
      coreStrength: d.core_strength ?? 0.0,
      importance: d.importance ?? 0.3,
      pinned: d.pinned ?? false,
      consolidationCount: d.consolidation_count ?? 0,
      lastConsolidated: d.last_consolidated ?? null,
      sourceFile: d.source_file ?? '',
      graphNodeIds: d.graph_node_ids ?? [],
      contradicts: d.contradicts ?? '',
      contradictedBy: d.contradicted_by ?? '',
      metadata: d.metadata ?? null,
    });
  }
}
