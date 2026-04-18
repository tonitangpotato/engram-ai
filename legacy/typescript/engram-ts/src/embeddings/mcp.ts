/**
 * MCP embedding provider (calls Python MCP server)
 */

import { spawn, ChildProcess } from 'child_process';
import { EmbeddingProvider, EmbeddingResult, ProviderInfo } from './base';

interface MCPRequest {
  jsonrpc: '2.0';
  id: number;
  method: string;
  params: any;
}

interface MCPResponse {
  jsonrpc: '2.0';
  id: number;
  result?: any;
  error?: {
    code: number;
    message: string;
  };
}

export class MCPEmbeddingProvider implements EmbeddingProvider {
  name = 'mcp';
  model: string;
  private command: string;
  private args: string[];
  private env: Record<string, string>;
  private process: ChildProcess | null = null;
  private requestId = 0;
  private pendingRequests = new Map<number, { resolve: Function; reject: Function }>();
  private buffer = '';

  constructor(opts: {
    command?: string;
    args?: string[];
    env?: Record<string, string>;
    model?: string;
  }) {
    this.command = opts.command || 'python3';
    this.args = opts.args || ['-m', 'engram.mcp_server'];
    this.env = opts.env || {};
    this.model = opts.model || 'auto';
  }

  private async ensureProcess(): Promise<void> {
    if (this.process) return;

    this.process = spawn(this.command, this.args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, ...this.env },
    });

    this.process.stdout?.on('data', (chunk) => {
      this.buffer += chunk.toString();
      const lines = this.buffer.split('\n');
      this.buffer = lines.pop() || '';

      for (const line of lines) {
        if (!line.trim()) continue;
        try {
          const response: MCPResponse = JSON.parse(line);
          const pending = this.pendingRequests.get(response.id);
          if (pending) {
            if (response.error) {
              pending.reject(new Error(response.error.message));
            } else {
              pending.resolve(response.result);
            }
            this.pendingRequests.delete(response.id);
          }
        } catch (err) {
          console.error('Failed to parse MCP response:', line, err);
        }
      }
    });

    this.process.stderr?.on('data', (chunk) => {
      console.error('MCP stderr:', chunk.toString());
    });

    this.process.on('exit', (code) => {
      console.error('MCP process exited with code:', code);
      this.process = null;
      // Reject all pending requests
      for (const [id, pending] of this.pendingRequests) {
        pending.reject(new Error('MCP process exited'));
      }
      this.pendingRequests.clear();
    });

    // Initialize
    await this.call('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'engram-ts', version: '1.0.0' },
    });
  }

  private async call(method: string, params: any): Promise<any> {
    await this.ensureProcess();

    const id = ++this.requestId;
    const request: MCPRequest = {
      jsonrpc: '2.0',
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      this.process?.stdin?.write(JSON.stringify(request) + '\n');

      // Timeout after 30s
      setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id);
          reject(new Error('MCP request timeout'));
        }
      }, 30000);
    });
  }

  async embed(text: string): Promise<EmbeddingResult> {
    // Store to get embedding
    const result = await this.call('tools/call', {
      name: 'store',
      arguments: {
        content: text,
        type: 'factual',
        importance: 0.5,
      },
    });

    // Recall to get the embedding back
    const recalls = await this.call('tools/call', {
      name: 'recall',
      arguments: {
        query: text,
        limit: 1,
      },
    });

    if (!recalls.content || recalls.content.length === 0) {
      throw new Error('Failed to get embedding from MCP');
    }

    // Extract embedding from result
    // Note: This is simplified - actual implementation depends on MCP server response format
    const embedding = recalls.content[0].embedding || [];

    return {
      embedding,
      dimensions: embedding.length,
    };
  }

  async isAvailable(): Promise<boolean> {
    try {
      await this.ensureProcess();
      const result = await this.call('tools/call', {
        name: 'embedding_status',
        arguments: {},
      });
      return result.content[0].provider !== 'none';
    } catch {
      return false;
    }
  }

  async getInfo(): Promise<ProviderInfo> {
    try {
      const result = await this.call('tools/call', {
        name: 'embedding_status',
        arguments: {},
      });

      const status = result.content[0];

      return {
        name: 'mcp',
        model: status.model || 'unknown',
        dimensions: status.vector_count || 0,
        available: status.provider !== 'none',
      };
    } catch (error) {
      return {
        name: 'mcp',
        model: 'unknown',
        dimensions: 0,
        available: false,
        error: error instanceof Error ? error.message : 'Unknown error',
      };
    }
  }

  async close(): Promise<void> {
    if (this.process) {
      this.process.kill();
      this.process = null;
    }
  }
}
