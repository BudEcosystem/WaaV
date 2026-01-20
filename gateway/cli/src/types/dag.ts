/**
 * WaaV CLI DAG Pipeline Types
 *
 * Defines the schema for DAG-based voice pipelines
 */

import { z } from 'zod';

// Node types in a DAG
export type DAGNodeType =
  | 'input'      // Audio/text input source
  | 'output'     // Audio/text output sink
  | 'stt'        // Speech-to-text
  | 'tts'        // Text-to-speech
  | 'llm'        // Language model processing
  | 'transform'  // Data transformation
  | 'filter'     // Conditional filtering
  | 'switch'     // Conditional routing
  | 'merge'      // Merge multiple inputs
  | 'split'      // Split to multiple outputs
  | 'cache'      // Caching layer
  | 'webhook'    // External webhook call
  | 'function'   // Custom function execution
  | 'delay'      // Delay/debounce
  | 'buffer'     // Buffer audio/text
  | 'vad'        // Voice activity detection
  | 'emotion'    // Emotion detection
  | 'sentiment'; // Sentiment analysis

// Data types flowing between nodes
export type DAGDataType = 'audio' | 'text' | 'json' | 'binary' | 'any';

// Node configuration based on type
export const DAGNodeConfigSchema = z.object({
  // Common properties
  id: z.string().min(1),
  type: z.string() as z.ZodType<DAGNodeType>,
  name: z.string().optional(),
  description: z.string().optional(),
  enabled: z.boolean().default(true),

  // Position for visualization
  position: z.object({
    x: z.number(),
    y: z.number(),
  }).optional(),

  // Type-specific configuration
  config: z.record(z.unknown()).optional(),

  // Input/output specifications
  inputs: z.array(z.object({
    name: z.string(),
    type: z.string() as z.ZodType<DAGDataType>,
    required: z.boolean().default(true),
  })).optional(),

  outputs: z.array(z.object({
    name: z.string(),
    type: z.string() as z.ZodType<DAGDataType>,
  })).optional(),

  // Error handling
  on_error: z.enum(['stop', 'skip', 'retry', 'fallback']).default('stop'),
  retry_config: z.object({
    max_attempts: z.number().int().positive().default(3),
    delay_ms: z.number().positive().default(1000),
    backoff_multiplier: z.number().positive().default(2),
  }).optional(),
  fallback_node: z.string().optional(),

  // Performance hints
  timeout_ms: z.number().positive().optional(),
  parallel: z.boolean().default(false),
});

export type DAGNodeConfig = z.infer<typeof DAGNodeConfigSchema>;

// Edge connecting two nodes
export const DAGEdgeSchema = z.object({
  id: z.string().min(1),
  source: z.string().min(1),
  target: z.string().min(1),
  source_output: z.string().default('default'),
  target_input: z.string().default('default'),
  condition: z.string().optional(), // JavaScript expression for conditional routing
  transform: z.string().optional(), // JavaScript expression for data transformation
  enabled: z.boolean().default(true),
});

export type DAGEdge = z.infer<typeof DAGEdgeSchema>;

// Complete DAG pipeline definition
export const DAGPipelineSchema = z.object({
  // Metadata
  id: z.string().min(1),
  name: z.string().min(1),
  description: z.string().optional(),
  version: z.string().default('1.0.0'),
  author: z.string().optional(),
  tags: z.array(z.string()).default([]),

  // Creation/modification timestamps
  created_at: z.string().datetime().optional(),
  updated_at: z.string().datetime().optional(),

  // Nodes and edges
  nodes: z.array(DAGNodeConfigSchema),
  edges: z.array(DAGEdgeSchema),

  // Global configuration
  config: z.object({
    // Default provider settings
    default_stt_provider: z.string().optional(),
    default_tts_provider: z.string().optional(),
    default_llm_provider: z.string().optional(),

    // Audio settings
    sample_rate: z.number().int().positive().default(16000),
    channels: z.number().int().positive().default(1),

    // Execution settings
    timeout_ms: z.number().positive().default(30000),
    max_concurrent_nodes: z.number().int().positive().default(10),

    // Logging
    log_level: z.enum(['debug', 'info', 'warn', 'error']).default('info'),
    log_inputs: z.boolean().default(false),
    log_outputs: z.boolean().default(false),
  }).default({}),

  // Variables that can be passed at runtime
  variables: z.record(z.object({
    type: z.enum(['string', 'number', 'boolean', 'json']),
    default: z.unknown().optional(),
    required: z.boolean().default(false),
    description: z.string().optional(),
  })).default({}),
});

export type DAGPipeline = z.infer<typeof DAGPipelineSchema>;

// DAG validation result
export interface DAGValidationResult {
  valid: boolean;
  errors: {
    type: 'error' | 'warning';
    node_id?: string;
    edge_id?: string;
    message: string;
    suggestion?: string;
  }[];
  stats: {
    node_count: number;
    edge_count: number;
    entry_points: string[];
    exit_points: string[];
    max_depth: number;
    has_cycles: boolean;
  };
}

// DAG execution status
export interface DAGExecutionStatus {
  pipeline_id: string;
  execution_id: string;
  status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';
  started_at?: Date;
  completed_at?: Date;
  node_statuses: Record<string, {
    status: 'pending' | 'running' | 'completed' | 'failed' | 'skipped';
    started_at?: Date;
    completed_at?: Date;
    error?: string;
    output_preview?: string;
  }>;
  current_node?: string;
  progress_percent: number;
  error?: string;
}

// Pre-built pipeline templates
export interface DAGTemplate {
  id: string;
  name: string;
  description: string;
  category: 'voice_assistant' | 'transcription' | 'translation' | 'custom';
  difficulty: 'beginner' | 'intermediate' | 'advanced';
  pipeline: DAGPipeline;
  preview_image?: string;
}

// Node type definitions for UI
export const NODE_TYPE_INFO: Record<DAGNodeType, {
  label: string;
  description: string;
  icon: string;
  color: string;
  inputs: { name: string; type: DAGDataType }[];
  outputs: { name: string; type: DAGDataType }[];
  config_fields: { name: string; type: string; required: boolean; default?: unknown }[];
}> = {
  input: {
    label: 'Input',
    description: 'Audio or text input source',
    icon: '📥',
    color: '#4CAF50',
    inputs: [],
    outputs: [{ name: 'default', type: 'any' }],
    config_fields: [
      { name: 'type', type: 'select', required: true, default: 'audio' },
    ],
  },
  output: {
    label: 'Output',
    description: 'Audio or text output sink',
    icon: '📤',
    color: '#F44336',
    inputs: [{ name: 'default', type: 'any' }],
    outputs: [],
    config_fields: [
      { name: 'type', type: 'select', required: true, default: 'audio' },
    ],
  },
  stt: {
    label: 'Speech-to-Text',
    description: 'Convert audio to text',
    icon: '🎤',
    color: '#2196F3',
    inputs: [{ name: 'audio', type: 'audio' }],
    outputs: [{ name: 'text', type: 'text' }],
    config_fields: [
      { name: 'provider', type: 'select', required: true, default: 'deepgram' },
      { name: 'language', type: 'string', required: false, default: 'en-US' },
      { name: 'model', type: 'string', required: false },
    ],
  },
  tts: {
    label: 'Text-to-Speech',
    description: 'Convert text to audio',
    icon: '🔊',
    color: '#9C27B0',
    inputs: [{ name: 'text', type: 'text' }],
    outputs: [{ name: 'audio', type: 'audio' }],
    config_fields: [
      { name: 'provider', type: 'select', required: true, default: 'elevenlabs' },
      { name: 'voice_id', type: 'string', required: true },
      { name: 'model', type: 'string', required: false },
    ],
  },
  llm: {
    label: 'LLM',
    description: 'Process text with language model',
    icon: '🧠',
    color: '#FF9800',
    inputs: [{ name: 'text', type: 'text' }],
    outputs: [{ name: 'text', type: 'text' }],
    config_fields: [
      { name: 'provider', type: 'select', required: true, default: 'openai' },
      { name: 'model', type: 'string', required: true, default: 'gpt-4o' },
      { name: 'system_prompt', type: 'textarea', required: false },
      { name: 'temperature', type: 'number', required: false, default: 0.7 },
    ],
  },
  transform: {
    label: 'Transform',
    description: 'Transform data format',
    icon: '🔄',
    color: '#607D8B',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'output', type: 'any' }],
    config_fields: [
      { name: 'expression', type: 'code', required: true },
    ],
  },
  filter: {
    label: 'Filter',
    description: 'Filter data conditionally',
    icon: '🔍',
    color: '#795548',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'pass', type: 'any' }, { name: 'fail', type: 'any' }],
    config_fields: [
      { name: 'condition', type: 'code', required: true },
    ],
  },
  switch: {
    label: 'Switch',
    description: 'Route data conditionally',
    icon: '🔀',
    color: '#00BCD4',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'case_1', type: 'any' }, { name: 'case_2', type: 'any' }, { name: 'default', type: 'any' }],
    config_fields: [
      { name: 'cases', type: 'array', required: true },
    ],
  },
  merge: {
    label: 'Merge',
    description: 'Merge multiple inputs',
    icon: '🔗',
    color: '#3F51B5',
    inputs: [{ name: 'input_1', type: 'any' }, { name: 'input_2', type: 'any' }],
    outputs: [{ name: 'output', type: 'any' }],
    config_fields: [
      { name: 'strategy', type: 'select', required: true, default: 'concat' },
    ],
  },
  split: {
    label: 'Split',
    description: 'Split to multiple outputs',
    icon: '📊',
    color: '#E91E63',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'output_1', type: 'any' }, { name: 'output_2', type: 'any' }],
    config_fields: [],
  },
  cache: {
    label: 'Cache',
    description: 'Cache results',
    icon: '💾',
    color: '#8BC34A',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'output', type: 'any' }],
    config_fields: [
      { name: 'ttl_seconds', type: 'number', required: false, default: 3600 },
      { name: 'key_expression', type: 'code', required: false },
    ],
  },
  webhook: {
    label: 'Webhook',
    description: 'Call external webhook',
    icon: '🌐',
    color: '#FFEB3B',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'response', type: 'json' }],
    config_fields: [
      { name: 'url', type: 'string', required: true },
      { name: 'method', type: 'select', required: true, default: 'POST' },
      { name: 'headers', type: 'object', required: false },
    ],
  },
  function: {
    label: 'Function',
    description: 'Execute custom function',
    icon: '⚡',
    color: '#673AB7',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'output', type: 'any' }],
    config_fields: [
      { name: 'code', type: 'code', required: true },
    ],
  },
  delay: {
    label: 'Delay',
    description: 'Delay processing',
    icon: '⏱️',
    color: '#9E9E9E',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'output', type: 'any' }],
    config_fields: [
      { name: 'delay_ms', type: 'number', required: true, default: 1000 },
    ],
  },
  buffer: {
    label: 'Buffer',
    description: 'Buffer data',
    icon: '📦',
    color: '#CDDC39',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'output', type: 'any' }],
    config_fields: [
      { name: 'size', type: 'number', required: true, default: 1024 },
      { name: 'flush_interval_ms', type: 'number', required: false },
    ],
  },
  vad: {
    label: 'VAD',
    description: 'Voice activity detection',
    icon: '📡',
    color: '#00E676',
    inputs: [{ name: 'audio', type: 'audio' }],
    outputs: [{ name: 'speech', type: 'audio' }, { name: 'events', type: 'json' }],
    config_fields: [
      { name: 'threshold', type: 'number', required: false, default: 0.5 },
      { name: 'speech_pad_ms', type: 'number', required: false, default: 300 },
    ],
  },
  emotion: {
    label: 'Emotion',
    description: 'Detect emotions',
    icon: '😀',
    color: '#FF5722',
    inputs: [{ name: 'input', type: 'any' }],
    outputs: [{ name: 'emotions', type: 'json' }, { name: 'passthrough', type: 'any' }],
    config_fields: [
      { name: 'provider', type: 'select', required: true, default: 'hume' },
    ],
  },
  sentiment: {
    label: 'Sentiment',
    description: 'Analyze sentiment',
    icon: '📈',
    color: '#009688',
    inputs: [{ name: 'text', type: 'text' }],
    outputs: [{ name: 'sentiment', type: 'json' }, { name: 'text', type: 'text' }],
    config_fields: [],
  },
};
