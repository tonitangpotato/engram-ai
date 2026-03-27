/**
 * Memory Configuration — Tunable Parameters
 */

export interface MemoryConfigOptions {
  // Forgetting
  spacingFactor?: number;
  importanceFloor?: number;
  consolidationBonus?: number;
  forgetThreshold?: number;
  suppressionFactor?: number;
  overlapThreshold?: number;

  // Consolidation (Memory Chain Model)
  mu1?: number;
  mu2?: number;
  alpha?: number;
  consolidationImportanceFloor?: number;
  interleaveRatio?: number;
  replayBoost?: number;
  promoteThreshold?: number;
  demoteThreshold?: number;
  archiveThreshold?: number;

  // Activation (ACT-R)
  actrDecay?: number;
  contextWeight?: number;
  importanceWeight?: number;
  minActivation?: number;

  // Confidence
  defaultReliability?: Record<string, number>;
  confidenceReliabilityWeight?: number;
  confidenceSalienceWeight?: number;
  salienceSigmoidK?: number;

  // Reward
  rewardMagnitude?: number;
  rewardRecentN?: number;
  rewardStrengthBoost?: number;
  rewardSuppression?: number;
  rewardTemporalDiscount?: number;

  // Downscaling
  downscaleFactor?: number;

  // Anomaly
  anomalyWindowSize?: number;
  anomalySigmaThreshold?: number;
  anomalyMinSamples?: number;

  // Hebbian Learning
  hebbianEnabled?: boolean;
  hebbianThreshold?: number;
  hebbianDecay?: number;

  // STDP (Spike-Timing-Dependent Plasticity)
  stdpEnabled?: boolean;
  stdpCausalThreshold?: number;
  stdpMinObservations?: number;

  // Search Weights (hybrid search scoring)
  ftsWeight?: number;
  embeddingWeight?: number;
  actrWeight?: number;
}

export class MemoryConfig {
  spacingFactor: number;
  importanceFloor: number;
  consolidationBonus: number;
  forgetThreshold: number;
  suppressionFactor: number;
  overlapThreshold: number;

  mu1: number;
  mu2: number;
  alpha: number;
  consolidationImportanceFloor: number;
  interleaveRatio: number;
  replayBoost: number;
  promoteThreshold: number;
  demoteThreshold: number;
  archiveThreshold: number;

  actrDecay: number;
  contextWeight: number;
  importanceWeight: number;
  minActivation: number;

  defaultReliability: Record<string, number>;
  confidenceReliabilityWeight: number;
  confidenceSalienceWeight: number;
  salienceSigmoidK: number;

  rewardMagnitude: number;
  rewardRecentN: number;
  rewardStrengthBoost: number;
  rewardSuppression: number;
  rewardTemporalDiscount: number;

  downscaleFactor: number;

  anomalyWindowSize: number;
  anomalySigmaThreshold: number;
  anomalyMinSamples: number;

  hebbianEnabled: boolean;
  hebbianThreshold: number;
  hebbianDecay: number;

  stdpEnabled: boolean;
  stdpCausalThreshold: number;
  stdpMinObservations: number;

  // Search Weights (hybrid search scoring)
  // Weight for FTS exact matching in hybrid recall (0.0-1.0)
  // Recommended: 0.15 for 15% FTS contribution
  ftsWeight: number;
  // Weight for embedding similarity in recall scoring (0.0-1.0)
  // Recommended: 0.60 for 60% semantic similarity contribution
  embeddingWeight: number;
  // Weight for ACT-R activation in recall scoring (0.0-1.0)
  // Recommended: 0.25 for 25% recency/frequency contribution
  // Note: ftsWeight + embeddingWeight + actrWeight should sum to ~1.0
  actrWeight: number;

  constructor(opts: MemoryConfigOptions = {}) {
    this.spacingFactor = opts.spacingFactor ?? 0.5;
    this.importanceFloor = opts.importanceFloor ?? 0.5;
    this.consolidationBonus = opts.consolidationBonus ?? 0.2;
    this.forgetThreshold = opts.forgetThreshold ?? 0.01;
    this.suppressionFactor = opts.suppressionFactor ?? 0.05;
    this.overlapThreshold = opts.overlapThreshold ?? 0.3;

    this.mu1 = opts.mu1 ?? 0.15;
    this.mu2 = opts.mu2 ?? 0.005;
    this.alpha = opts.alpha ?? 0.08;
    this.consolidationImportanceFloor = opts.consolidationImportanceFloor ?? 0.2;
    this.interleaveRatio = opts.interleaveRatio ?? 0.3;
    this.replayBoost = opts.replayBoost ?? 0.01;
    this.promoteThreshold = opts.promoteThreshold ?? 0.25;
    this.demoteThreshold = opts.demoteThreshold ?? 0.05;
    this.archiveThreshold = opts.archiveThreshold ?? 0.15;

    this.actrDecay = opts.actrDecay ?? 0.5;
    this.contextWeight = opts.contextWeight ?? 1.5;
    this.importanceWeight = opts.importanceWeight ?? 0.5;
    this.minActivation = opts.minActivation ?? -10.0;

    this.defaultReliability = opts.defaultReliability ?? {
      factual: 0.85,
      episodic: 0.90,
      relational: 0.75,
      emotional: 0.95,
      procedural: 0.90,
      opinion: 0.60,
      causal: 0.70,
    };
    this.confidenceReliabilityWeight = opts.confidenceReliabilityWeight ?? 0.7;
    this.confidenceSalienceWeight = opts.confidenceSalienceWeight ?? 0.3;
    this.salienceSigmoidK = opts.salienceSigmoidK ?? 2.0;

    this.rewardMagnitude = opts.rewardMagnitude ?? 0.15;
    this.rewardRecentN = opts.rewardRecentN ?? 3;
    this.rewardStrengthBoost = opts.rewardStrengthBoost ?? 0.05;
    this.rewardSuppression = opts.rewardSuppression ?? 0.1;
    this.rewardTemporalDiscount = opts.rewardTemporalDiscount ?? 0.5;

    this.downscaleFactor = opts.downscaleFactor ?? 0.95;

    this.anomalyWindowSize = opts.anomalyWindowSize ?? 100;
    this.anomalySigmaThreshold = opts.anomalySigmaThreshold ?? 2.0;
    this.anomalyMinSamples = opts.anomalyMinSamples ?? 5;

    this.hebbianEnabled = opts.hebbianEnabled ?? true;
    this.hebbianThreshold = opts.hebbianThreshold ?? 3;
    this.hebbianDecay = opts.hebbianDecay ?? 0.95;

    this.stdpEnabled = opts.stdpEnabled ?? true;
    this.stdpCausalThreshold = opts.stdpCausalThreshold ?? 2.0;
    this.stdpMinObservations = opts.stdpMinObservations ?? 3;

    // Search weights for hybrid scoring
    this.ftsWeight = opts.ftsWeight ?? 0.15;        // 15% exact matching
    this.embeddingWeight = opts.embeddingWeight ?? 0.60;   // 60% semantic similarity
    this.actrWeight = opts.actrWeight ?? 0.25;        // 25% recency/frequency/importance
  }

  static default(): MemoryConfig {
    return new MemoryConfig();
  }

  static chatbot(): MemoryConfig {
    return new MemoryConfig({
      mu1: 0.08,
      mu2: 0.003,
      alpha: 0.12,
      interleaveRatio: 0.4,
      replayBoost: 0.015,
      actrDecay: 0.4,
      contextWeight: 2.0,
      downscaleFactor: 0.96,
      rewardMagnitude: 0.2,
      forgetThreshold: 0.005,
    });
  }

  static taskAgent(): MemoryConfig {
    return new MemoryConfig({
      mu1: 0.25,
      mu2: 0.01,
      alpha: 0.05,
      interleaveRatio: 0.1,
      replayBoost: 0.005,
      actrDecay: 0.6,
      promoteThreshold: 0.35,
      archiveThreshold: 0.2,
      downscaleFactor: 0.90,
      forgetThreshold: 0.02,
    });
  }

  static personalAssistant(): MemoryConfig {
    return new MemoryConfig({
      mu1: 0.12,
      mu2: 0.001,
      alpha: 0.10,
      interleaveRatio: 0.3,
      replayBoost: 0.02,
      actrDecay: 0.45,
      importanceWeight: 0.7,
      promoteThreshold: 0.20,
      demoteThreshold: 0.03,
      downscaleFactor: 0.97,
      forgetThreshold: 0.005,
      confidenceReliabilityWeight: 0.8,
      confidenceSalienceWeight: 0.2,
    });
  }

  static researcher(): MemoryConfig {
    return new MemoryConfig({
      mu1: 0.05,
      mu2: 0.001,
      alpha: 0.15,
      interleaveRatio: 0.5,
      replayBoost: 0.025,
      actrDecay: 0.35,
      contextWeight: 2.0,
      importanceWeight: 0.3,
      promoteThreshold: 0.15,
      demoteThreshold: 0.02,
      archiveThreshold: 0.10,
      downscaleFactor: 0.98,
      forgetThreshold: 0.001,
    });
  }
}

// === Config File Hierarchy ===
// Priority: code params > ANTHROPIC_AUTH_TOKEN env > ANTHROPIC_API_KEY env > ~/.config/engram/config.json > no extractor

/**
 * Engram config file format (~/.config/engram/config.json)
 *
 * NOTE: Auth tokens are NEVER stored in config file. They come from env vars.
 */
export interface EngramFileConfig {
  extractor?: {
    /** Provider: "anthropic" | "ollama" */
    provider: string;
    /** Model name */
    model?: string;
    /** Host URL (for Ollama) */
    host?: string;
  };
  embedding?: {
    /** Provider: "ollama" | "openai" | "mcp" */
    provider?: string;
    /** Model name */
    model?: string;
    /** Host URL */
    host?: string;
  };
}

/**
 * Get the config file path (~/.config/engram/config.json)
 */
export function getConfigPath(): string {
  const os = require('os');
  const path = require('path');
  return path.join(os.homedir(), '.config', 'engram', 'config.json');
}

/**
 * Load config from ~/.config/engram/config.json
 * Returns null if file doesn't exist or is invalid.
 */
export function loadFileConfig(): EngramFileConfig | null {
  try {
    const fs = require('fs');
    const configPath = getConfigPath();
    if (!fs.existsSync(configPath)) return null;
    const content = fs.readFileSync(configPath, 'utf-8');
    return JSON.parse(content) as EngramFileConfig;
  } catch {
    return null;
  }
}

/**
 * Save config to ~/.config/engram/config.json
 * Creates parent directories if needed.
 */
export function saveFileConfig(config: EngramFileConfig): void {
  const fs = require('fs');
  const path = require('path');
  const configPath = getConfigPath();
  const dir = path.dirname(configPath);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(configPath, JSON.stringify(config, null, 2) + '\n', 'utf-8');
}

/**
 * Interactive config setup (for `engram init` CLI command).
 * Uses readline to prompt the user.
 */
export async function interactiveConfigSetup(): Promise<void> {
  const readline = require('readline');
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });

  const ask = (question: string): Promise<string> =>
    new Promise((resolve) => rl.question(question, resolve));

  console.log('\n🧠 Engram Configuration Setup\n');
  console.log('This sets up ~/.config/engram/config.json');
  console.log('NOTE: Auth tokens are stored in env vars, NOT in the config file.\n');

  const config: EngramFileConfig = loadFileConfig() ?? {};

  // Extractor setup
  const setupExtractor = await ask('Set up LLM extraction? (y/N): ');
  if (setupExtractor.toLowerCase() === 'y') {
    const provider = await ask('Extractor provider (anthropic/ollama) [anthropic]: ');
    const selectedProvider = provider.trim() || 'anthropic';

    config.extractor = { provider: selectedProvider };

    if (selectedProvider === 'anthropic') {
      const model = await ask('Model [claude-haiku-4-5-20251001]: ');
      if (model.trim()) config.extractor.model = model.trim();

      // Remind about env vars
      const hasOAuth = Boolean(process.env.ANTHROPIC_AUTH_TOKEN);
      const hasApiKey = Boolean(process.env.ANTHROPIC_API_KEY);
      if (!hasOAuth && !hasApiKey) {
        console.log('\n⚠️  No Anthropic auth token found in environment.');
        console.log('Set one of:');
        console.log('  export ANTHROPIC_AUTH_TOKEN=sk-ant-oat01-...');
        console.log('  export ANTHROPIC_API_KEY=sk-ant-api03-...\n');
      } else {
        console.log(`✅ Using ${hasOAuth ? 'ANTHROPIC_AUTH_TOKEN (OAuth)' : 'ANTHROPIC_API_KEY'}`);
      }
    } else if (selectedProvider === 'ollama') {
      const model = await ask('Model [llama3.2:3b]: ');
      if (model.trim()) config.extractor.model = model.trim();
      const host = await ask('Ollama host [http://localhost:11434]: ');
      if (host.trim()) config.extractor.host = host.trim();
    }
  }

  // Save
  saveFileConfig(config);
  console.log(`\n✅ Config saved to ${getConfigPath()}`);

  rl.close();
}
