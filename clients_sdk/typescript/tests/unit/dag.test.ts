import { describe, it, expect } from 'vitest';
import {
  DAGConfig,
  DAGDefinition,
  DAGNode,
  DAGEdge,
  DAGNodeType,
  DAG_NODE_TYPES,
  validateDAGDefinition,
  createDAGConfig,
  serializeDAGConfig,
} from '../../src/types/dag';

describe('DAG Node Types', () => {
  const expectedNodeTypes: DAGNodeType[] = [
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
  ];

  it('should have all expected node types defined', () => {
    for (const nodeType of expectedNodeTypes) {
      expect(DAG_NODE_TYPES).toContain(nodeType);
    }
  });

  it('should support at least 13 node types', () => {
    expect(DAG_NODE_TYPES.length).toBeGreaterThanOrEqual(13);
  });
});

describe('DAG Definition Validation', () => {
  it('should validate a simple STT pipeline DAG', () => {
    const dag: DAGDefinition = {
      id: 'simple-stt',
      name: 'Simple STT Pipeline',
      version: '1.0',
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

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  it('should validate a voice assistant DAG (STT → LLM → TTS)', () => {
    const dag: DAGDefinition = {
      id: 'voice-assistant',
      name: 'Voice Assistant Pipeline',
      version: '1.0',
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

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(true);
  });

  it('should reject DAG with missing required fields', () => {
    const dag = {
      id: 'incomplete',
      nodes: [],
      edges: [],
    } as unknown as DAGDefinition;

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(false);
    expect(result.errors.length).toBeGreaterThan(0);
    expect(result.errors.some(e => e.includes('name'))).toBe(true);
    expect(result.errors.some(e => e.includes('version'))).toBe(true);
  });

  it('should reject DAG with duplicate node IDs', () => {
    const dag: DAGDefinition = {
      id: 'duplicate-nodes',
      name: 'Duplicate Nodes',
      version: '1.0',
      nodes: [
        { id: 'input', type: 'audio_input' },
        { id: 'input', type: 'stt_provider' }, // Duplicate!
      ],
      edges: [],
    };

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.toLowerCase().includes('duplicate'))).toBe(true);
  });

  it('should reject DAG with edges referencing non-existent nodes', () => {
    const dag: DAGDefinition = {
      id: 'invalid-edges',
      name: 'Invalid Edges',
      version: '1.0',
      nodes: [
        { id: 'input', type: 'audio_input' },
      ],
      edges: [
        { from: 'input', to: 'nonexistent' },
      ],
    };

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('nonexistent'))).toBe(true);
  });

  it('should reject DAG with cycles', () => {
    const dag: DAGDefinition = {
      id: 'cyclic',
      name: 'Cyclic DAG',
      version: '1.0',
      nodes: [
        { id: 'a', type: 'transform' },
        { id: 'b', type: 'transform' },
        { id: 'c', type: 'transform' },
      ],
      edges: [
        { from: 'a', to: 'b' },
        { from: 'b', to: 'c' },
        { from: 'c', to: 'a' }, // Cycle!
      ],
    };

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('cycle'))).toBe(true);
  });

  it('should validate conditional edges', () => {
    const dag: DAGDefinition = {
      id: 'conditional',
      name: 'Conditional Routing',
      version: '1.0',
      nodes: [
        { id: 'input', type: 'text_input' },
        { id: 'router', type: 'router' },
        { id: 'path_a', type: 'transform' },
        { id: 'path_b', type: 'transform' },
        { id: 'output', type: 'text_output' },
      ],
      edges: [
        { from: 'input', to: 'router' },
        { from: 'router', to: 'path_a', condition: 'input.length > 100' },
        { from: 'router', to: 'path_b', condition: 'input.length <= 100' },
        { from: 'path_a', to: 'output' },
        { from: 'path_b', to: 'output' },
      ],
    };

    const result = validateDAGDefinition(dag);
    expect(result.valid).toBe(true);
  });
});

describe('DAG Config', () => {
  it('should create DAG config from template name', () => {
    const config = createDAGConfig({ template: 'voice-assistant' });
    expect(config.template).toBe('voice-assistant');
    expect(config.definition).toBeUndefined();
  });

  it('should create DAG config from inline definition', () => {
    const definition: DAGDefinition = {
      id: 'inline',
      name: 'Inline DAG',
      version: '1.0',
      nodes: [{ id: 'input', type: 'audio_input' }],
      edges: [],
    };

    const config = createDAGConfig({ definition });
    expect(config.definition).toEqual(definition);
    expect(config.template).toBeUndefined();
  });

  it('should apply default values', () => {
    const config = createDAGConfig({ template: 'test' });
    expect(config.enableMetrics).toBe(false);
    expect(config.timeoutMs).toBe(30000);
  });

  it('should serialize DAG config to wire format', () => {
    const config: DAGConfig = {
      template: 'voice-assistant',
      enableMetrics: true,
      timeoutMs: 60000,
    };

    const wire = serializeDAGConfig(config);
    expect(wire.template).toBe('voice-assistant');
    expect(wire.enable_metrics).toBe(true);
    expect(wire.timeout_ms).toBe(60000);
  });

  it('should serialize inline definition to wire format', () => {
    const config: DAGConfig = {
      definition: {
        id: 'test',
        name: 'Test',
        version: '1.0',
        nodes: [{ id: 'n1', type: 'audio_input' }],
        edges: [],
      },
    };

    const wire = serializeDAGConfig(config);
    expect(wire.definition).toBeDefined();
    expect(wire.definition.id).toBe('test');
  });
});
