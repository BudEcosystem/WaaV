// =============================================================================
// DAG (Directed Acyclic Graph) Routing Types
// =============================================================================

/**
 * Node types supported in DAG definitions.
 * These map to the node types in the Rust DAG executor.
 */
export const DAG_NODE_TYPES = [
  'audio_input',
  'audio_output',
  'text_input',
  'text_output',
  'stt_provider',
  'tts_provider',
  'llm',
  'http_endpoint',
  'webhook',
  'transform',
  'router',
  'buffer',
  'switch',
] as const;

export type DAGNodeType = (typeof DAG_NODE_TYPES)[number];

/**
 * A node in the DAG pipeline.
 */
export interface DAGNode {
  /** Unique identifier for this node */
  id: string;
  /** Type of the node */
  type: DAGNodeType;
  /** Node-specific configuration */
  config?: Record<string, unknown>;
}

/**
 * An edge connecting two nodes in the DAG.
 */
export interface DAGEdge {
  /** Source node ID */
  from: string;
  /** Destination node ID */
  to: string;
  /** Optional condition expression (Rhai script) */
  condition?: string;
}

/**
 * Complete DAG definition.
 */
export interface DAGDefinition {
  /** Unique identifier for this DAG */
  id: string;
  /** Human-readable name */
  name: string;
  /** Version string */
  version: string;
  /** Description of the DAG */
  description?: string;
  /** Nodes in the DAG */
  nodes: DAGNode[];
  /** Edges connecting nodes */
  edges: DAGEdge[];
  /** Optional metadata */
  metadata?: Record<string, unknown>;
}

/**
 * DAG configuration for WebSocket sessions.
 */
export interface DAGConfig {
  /** Name of a pre-registered template to use */
  template?: string;
  /** Inline DAG definition (takes precedence over template) */
  definition?: DAGDefinition;
  /** Enable metrics collection for DAG execution */
  enableMetrics?: boolean;
  /** Maximum execution time in milliseconds */
  timeoutMs?: number;
}

/**
 * Default DAG configuration values.
 */
export const DEFAULT_DAG_CONFIG: Partial<DAGConfig> = {
  enableMetrics: false,
  timeoutMs: 30000,
};

/**
 * Validation result for DAG definitions.
 */
export interface DAGValidationResult {
  valid: boolean;
  errors: string[];
  warnings: string[];
}

// =============================================================================
// DAG Validation
// =============================================================================

/**
 * Validate a DAG definition.
 * Checks for:
 * - Required fields
 * - Unique node IDs
 * - Valid edge references
 * - No cycles (DAG must be acyclic)
 */
export function validateDAGDefinition(dag: DAGDefinition): DAGValidationResult {
  const errors: string[] = [];
  const warnings: string[] = [];

  // Check required fields
  if (!dag.id) {
    errors.push('DAG id is required');
  }
  if (!dag.name) {
    errors.push('DAG name is required');
  }
  if (!dag.version) {
    errors.push('DAG version is required');
  }

  // Check for duplicate node IDs
  const nodeIds = new Set<string>();
  for (const node of dag.nodes) {
    if (!node.id) {
      errors.push('Node id is required');
      continue;
    }
    if (nodeIds.has(node.id)) {
      errors.push(`Duplicate node id: ${node.id}`);
    }
    nodeIds.add(node.id);

    // Validate node type
    if (!DAG_NODE_TYPES.includes(node.type)) {
      errors.push(`Invalid node type: ${node.type} for node ${node.id}`);
    }
  }

  // Check edge references
  for (const edge of dag.edges) {
    if (!nodeIds.has(edge.from)) {
      errors.push(`Edge references nonexistent source node: ${edge.from}`);
    }
    if (!nodeIds.has(edge.to)) {
      errors.push(`Edge references nonexistent target node: ${edge.to}`);
    }
  }

  // Check for cycles using DFS
  if (errors.length === 0) {
    const cycleResult = detectCycles(dag);
    if (cycleResult.hasCycle) {
      errors.push(`DAG contains a cycle: ${cycleResult.cyclePath?.join(' → ')}`);
    }
  }

  // Warnings
  if (dag.nodes.length === 0) {
    warnings.push('DAG has no nodes');
  }
  if (dag.edges.length === 0 && dag.nodes.length > 1) {
    warnings.push('DAG has multiple nodes but no edges');
  }

  // Check for disconnected nodes
  const connectedNodes = new Set<string>();
  for (const edge of dag.edges) {
    connectedNodes.add(edge.from);
    connectedNodes.add(edge.to);
  }
  for (const node of dag.nodes) {
    if (dag.nodes.length > 1 && !connectedNodes.has(node.id)) {
      warnings.push(`Node ${node.id} is not connected to any other node`);
    }
  }

  return {
    valid: errors.length === 0,
    errors,
    warnings,
  };
}

/**
 * Detect cycles in the DAG.
 */
function detectCycles(dag: DAGDefinition): { hasCycle: boolean; cyclePath?: string[] } {
  const adjacency = new Map<string, string[]>();

  // Build adjacency list
  for (const node of dag.nodes) {
    adjacency.set(node.id, []);
  }
  for (const edge of dag.edges) {
    adjacency.get(edge.from)?.push(edge.to);
  }

  const visited = new Set<string>();
  const recursionStack = new Set<string>();
  const path: string[] = [];

  function dfs(nodeId: string): boolean {
    visited.add(nodeId);
    recursionStack.add(nodeId);
    path.push(nodeId);

    const neighbors = adjacency.get(nodeId) || [];
    for (const neighbor of neighbors) {
      if (!visited.has(neighbor)) {
        if (dfs(neighbor)) {
          return true;
        }
      } else if (recursionStack.has(neighbor)) {
        // Found cycle
        path.push(neighbor);
        return true;
      }
    }

    path.pop();
    recursionStack.delete(nodeId);
    return false;
  }

  for (const node of dag.nodes) {
    if (!visited.has(node.id)) {
      if (dfs(node.id)) {
        // Extract the cycle from the path
        const cycleStart = path.indexOf(path[path.length - 1]);
        return {
          hasCycle: true,
          cyclePath: path.slice(cycleStart),
        };
      }
    }
  }

  return { hasCycle: false };
}

// =============================================================================
// DAG Config Helpers
// =============================================================================

/**
 * Create a DAG config with defaults.
 */
export function createDAGConfig(config: Partial<DAGConfig>): DAGConfig {
  return {
    ...DEFAULT_DAG_CONFIG,
    ...config,
  };
}

/**
 * Serialize DAG config to wire format (snake_case).
 */
export function serializeDAGConfig(config: DAGConfig): Record<string, unknown> {
  const wire: Record<string, unknown> = {};

  if (config.template) {
    wire.template = config.template;
  }

  if (config.definition) {
    wire.definition = serializeDAGDefinition(config.definition);
  }

  if (config.enableMetrics !== undefined) {
    wire.enable_metrics = config.enableMetrics;
  }

  if (config.timeoutMs !== undefined) {
    wire.timeout_ms = config.timeoutMs;
  }

  return wire;
}

/**
 * Serialize DAG definition to wire format.
 */
function serializeDAGDefinition(def: DAGDefinition): Record<string, unknown> {
  return {
    id: def.id,
    name: def.name,
    version: def.version,
    description: def.description,
    nodes: def.nodes.map((node) => ({
      id: node.id,
      type: node.type,
      config: node.config,
    })),
    edges: def.edges.map((edge) => ({
      from: edge.from,
      to: edge.to,
      condition: edge.condition,
    })),
    metadata: def.metadata,
  };
}

/**
 * Deserialize DAG config from wire format.
 */
export function deserializeDAGConfig(wire: Record<string, unknown>): DAGConfig {
  const config: DAGConfig = {};

  if (typeof wire.template === 'string') {
    config.template = wire.template;
  }

  if (wire.definition && typeof wire.definition === 'object') {
    config.definition = deserializeDAGDefinition(wire.definition as Record<string, unknown>);
  }

  if (typeof wire.enable_metrics === 'boolean') {
    config.enableMetrics = wire.enable_metrics;
  }

  if (typeof wire.timeout_ms === 'number') {
    config.timeoutMs = wire.timeout_ms;
  }

  return config;
}

/**
 * Deserialize DAG definition from wire format.
 */
function deserializeDAGDefinition(wire: Record<string, unknown>): DAGDefinition {
  return {
    id: wire.id as string,
    name: wire.name as string,
    version: wire.version as string,
    description: wire.description as string | undefined,
    nodes: (wire.nodes as Array<Record<string, unknown>>).map((node) => ({
      id: node.id as string,
      type: node.type as DAGNodeType,
      config: node.config as Record<string, unknown> | undefined,
    })),
    edges: (wire.edges as Array<Record<string, unknown>>).map((edge) => ({
      from: edge.from as string,
      to: edge.to as string,
      condition: edge.condition as string | undefined,
    })),
    metadata: wire.metadata as Record<string, unknown> | undefined,
  };
}

// =============================================================================
// Pre-built DAG Templates
// =============================================================================

/**
 * Simple STT pipeline: audio_input → stt → text_output
 */
export const TEMPLATE_SIMPLE_STT: DAGDefinition = {
  id: 'simple-stt',
  name: 'Simple STT Pipeline',
  version: '1.0',
  description: 'Convert audio to text using speech-to-text',
  nodes: [
    { id: 'input', type: 'audio_input' },
    { id: 'stt', type: 'stt_provider', config: { provider: 'deepgram' } },
    { id: 'output', type: 'text_output' },
  ],
  edges: [
    { from: 'input', to: 'stt' },
    { from: 'stt', to: 'output' },
  ],
};

/**
 * Simple TTS pipeline: text_input → tts → audio_output
 */
export const TEMPLATE_SIMPLE_TTS: DAGDefinition = {
  id: 'simple-tts',
  name: 'Simple TTS Pipeline',
  version: '1.0',
  description: 'Convert text to speech using text-to-speech',
  nodes: [
    { id: 'input', type: 'text_input' },
    { id: 'tts', type: 'tts_provider', config: { provider: 'elevenlabs' } },
    { id: 'output', type: 'audio_output' },
  ],
  edges: [
    { from: 'input', to: 'tts' },
    { from: 'tts', to: 'output' },
  ],
};

/**
 * Voice assistant pipeline: audio_input → stt → llm → tts → audio_output
 */
export const TEMPLATE_VOICE_ASSISTANT: DAGDefinition = {
  id: 'voice-assistant',
  name: 'Voice Assistant Pipeline',
  version: '1.0',
  description: 'Full voice assistant with STT, LLM, and TTS',
  nodes: [
    { id: 'audio_in', type: 'audio_input' },
    { id: 'stt', type: 'stt_provider', config: { provider: 'deepgram' } },
    { id: 'llm', type: 'llm', config: { provider: 'openai', model: 'gpt-4' } },
    { id: 'tts', type: 'tts_provider', config: { provider: 'elevenlabs' } },
    { id: 'audio_out', type: 'audio_output' },
  ],
  edges: [
    { from: 'audio_in', to: 'stt' },
    { from: 'stt', to: 'llm' },
    { from: 'llm', to: 'tts' },
    { from: 'tts', to: 'audio_out' },
  ],
};

/**
 * Transcription pipeline: audio_input → stt → text_output (optimized for batch)
 */
export const TEMPLATE_TRANSCRIPTION: DAGDefinition = {
  id: 'transcription',
  name: 'Transcription Pipeline',
  version: '1.0',
  description: 'Optimized for batch audio transcription',
  nodes: [
    { id: 'input', type: 'audio_input' },
    { id: 'stt', type: 'stt_provider', config: { provider: 'deepgram', model: 'nova-3' } },
    { id: 'output', type: 'text_output' },
  ],
  edges: [
    { from: 'input', to: 'stt' },
    { from: 'stt', to: 'output' },
  ],
};

/**
 * All available built-in templates.
 */
export const BUILTIN_TEMPLATES: Record<string, DAGDefinition> = {
  'simple-stt': TEMPLATE_SIMPLE_STT,
  'simple-tts': TEMPLATE_SIMPLE_TTS,
  'voice-assistant': TEMPLATE_VOICE_ASSISTANT,
  transcription: TEMPLATE_TRANSCRIPTION,
};

/**
 * Get a built-in template by name.
 */
export function getBuiltinTemplate(name: string): DAGDefinition | undefined {
  return BUILTIN_TEMPLATES[name];
}
