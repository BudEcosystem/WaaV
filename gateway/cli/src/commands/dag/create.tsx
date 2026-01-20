/**
 * WaaV CLI DAG Create Command
 *
 * Interactive DAG pipeline builder with step-by-step wizard
 */

import React, { useState, useCallback } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import { nanoid } from 'nanoid';
import * as yaml from 'js-yaml';
import * as fs from 'fs';
import * as path from 'path';

import { logger } from '../../utils/logger.js';
import type { GlobalOptions } from '../../cli.js';
import {
  Select,
  TextInput,
  Confirm,
  Card,
  Alert,
  Spinner,
  type SelectItem,
} from '../../components/common/index.js';
import {
  type DAGNodeConfig,
  type DAGEdge,
  type DAGPipeline,
  type DAGNodeType,
  NODE_TYPE_INFO,
  DAGPipelineSchema,
} from '../../types/dag.js';

/**
 * Wizard step definitions
 */
type WizardStep =
  | 'metadata'
  | 'entry_node'
  | 'add_nodes'
  | 'configure_node'
  | 'add_edges'
  | 'configure_edge'
  | 'exit_node'
  | 'review'
  | 'save';

/**
 * Wizard state
 */
interface WizardState {
  step: WizardStep;
  // Metadata
  pipelineName: string;
  pipelineDescription: string;
  // Nodes
  nodes: DAGNodeConfig[];
  currentNodeIndex: number;
  // Edges
  edges: DAGEdge[];
  currentEdgeIndex: number;
  // Temp state for current node/edge being configured
  tempNode: Partial<DAGNodeConfig>;
  tempEdge: Partial<DAGEdge>;
  // Navigation
  subStep: number;
  error: string | null;
  saving: boolean;
}

/**
 * Initial wizard state
 */
const initialState: WizardState = {
  step: 'metadata',
  pipelineName: '',
  pipelineDescription: '',
  nodes: [],
  currentNodeIndex: -1,
  edges: [],
  currentEdgeIndex: -1,
  tempNode: {},
  tempEdge: {},
  subStep: 0,
  error: null,
  saving: false,
};

/**
 * Node type selection items
 */
const nodeTypeItems: SelectItem[] = Object.entries(NODE_TYPE_INFO).map(([type, info]) => ({
  label: `${info.icon} ${info.label}`,
  value: type,
  description: info.description,
}));

/**
 * DAG Create Wizard Component
 */
interface DAGCreateWizardProps {
  options: GlobalOptions;
  outputPath?: string;
}

const DAGCreateWizard: React.FC<DAGCreateWizardProps> = ({ options, outputPath }) => {
  const { exit } = useApp();
  const [state, setState] = useState<WizardState>(initialState);

  // Update state helper
  const updateState = useCallback((updates: Partial<WizardState>) => {
    setState((prev) => ({ ...prev, ...updates }));
  }, []);

  // Handle keyboard shortcuts
  useInput((_input, key) => {
    if (key.escape) {
      exit();
      return;
    }

    // Quick save shortcut
    if (key.ctrl && _input === 's' && state.step === 'review') {
      updateState({ step: 'save', saving: true });
    }
  });

  // Step handlers
  const handleMetadataSubmit = useCallback((name: string, description: string) => {
    if (!name.trim()) {
      updateState({ error: 'Pipeline name is required' });
      return;
    }

    updateState({
      pipelineName: name.trim(),
      pipelineDescription: description.trim(),
      step: 'entry_node',
      subStep: 0,
      error: null,
    });
  }, [updateState]);

  const handleNodeTypeSelect = useCallback((type: string, isEntry: boolean = false, isExit: boolean = false) => {
    const nodeType = type as DAGNodeType;
    const info = NODE_TYPE_INFO[nodeType];

    const newNode: DAGNodeConfig = {
      id: nanoid(),
      type: nodeType,
      name: `${info.label} ${state.nodes.length + 1}`,
      description: info.description,
      enabled: true,
      inputs: info.inputs.map((inp) => ({
        name: inp.name,
        type: inp.type,
        required: true,
      })),
      outputs: info.outputs.map((out) => ({
        name: out.name,
        type: out.type,
      })),
      on_error: 'stop',
      parallel: false,
    };

    // Set position for visualization
    const yPos = state.nodes.length * 100;
    newNode.position = { x: 200, y: yPos };

    // If entry node, start at x=0
    if (isEntry) {
      newNode.position = { x: 0, y: 50 };
    }

    // If exit node, place at end
    if (isExit) {
      newNode.position = { x: 400, y: 50 };
    }

    updateState({
      tempNode: newNode,
      step: 'configure_node',
      subStep: 0,
    });
  }, [state.nodes.length, updateState]);

  const handleNodeConfigure = useCallback((config: Record<string, unknown>) => {
    const newNode: DAGNodeConfig = {
      ...state.tempNode,
      config,
    } as DAGNodeConfig;

    const isEntryNode = state.nodes.length === 0;
    const updatedNodes = [...state.nodes, newNode];

    // If this was the entry node, go to add more nodes
    if (isEntryNode) {
      updateState({
        nodes: updatedNodes,
        tempNode: {},
        step: 'add_nodes',
        subStep: 0,
      });
    } else {
      // Otherwise, ask about edges
      updateState({
        nodes: updatedNodes,
        tempNode: {},
        step: 'add_edges',
        subStep: 0,
      });
    }
  }, [state.tempNode, state.nodes, updateState]);

  const handleAddMoreNodes = useCallback((addMore: boolean) => {
    if (addMore) {
      updateState({
        step: 'add_nodes',
        subStep: 1, // Show node type selection
      });
    } else {
      // Check if we have at least 2 nodes for edges
      if (state.nodes.length < 2) {
        updateState({ error: 'Add at least 2 nodes to create a pipeline' });
        return;
      }
      updateState({
        step: 'add_edges',
        subStep: 0,
      });
    }
  }, [state.nodes.length, updateState]);

  const handleEdgeCreate = useCallback((sourceId: string, targetId: string) => {
    const newEdge: DAGEdge = {
      id: nanoid(),
      source: sourceId,
      target: targetId,
      source_output: 'default',
      target_input: 'default',
      enabled: true,
    };

    const updatedEdges = [...state.edges, newEdge];

    updateState({
      edges: updatedEdges,
      tempEdge: {},
      step: 'add_edges',
      subStep: 0,
    });
  }, [state.edges, updateState]);

  const handleAddMoreEdges = useCallback((addMore: boolean) => {
    if (addMore) {
      updateState({
        step: 'add_edges',
        subStep: 1, // Show edge creation
      });
    } else {
      updateState({
        step: 'review',
        subStep: 0,
      });
    }
  }, [updateState]);

  const handleSave = useCallback(async () => {
    updateState({ saving: true, error: null });

    try {
      // Build the pipeline
      const pipeline: DAGPipeline = {
        id: nanoid(),
        name: state.pipelineName,
        description: state.pipelineDescription,
        version: '1.0.0',
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        tags: [],
        nodes: state.nodes,
        edges: state.edges,
        config: {
          sample_rate: 16000,
          channels: 1,
          timeout_ms: 30000,
          max_concurrent_nodes: 10,
          log_level: 'info',
          log_inputs: false,
          log_outputs: false,
        },
        variables: {},
      };

      // Validate with Zod
      const result = DAGPipelineSchema.safeParse(pipeline);
      if (!result.success) {
        const errorMessages = result.error.errors.map((e) => e.message).join(', ');
        updateState({ error: `Validation failed: ${errorMessages}`, saving: false });
        return;
      }

      // Determine output path
      const finalPath = outputPath || path.join(process.cwd(), `${state.pipelineName.toLowerCase().replace(/\s+/g, '-')}.dag.yaml`);

      // Write YAML file
      const yamlContent = yaml.dump(result.data, {
        indent: 2,
        lineWidth: 120,
        noRefs: true,
      });

      fs.writeFileSync(finalPath, yamlContent, 'utf-8');

      if (options.json) {
        console.log(JSON.stringify({
          success: true,
          path: finalPath,
          pipeline: result.data,
        }));
      } else {
        logger.success(`Pipeline saved to: ${finalPath}`);
      }

      exit();
    } catch (err) {
      updateState({
        error: `Failed to save: ${err instanceof Error ? err.message : 'Unknown error'}`,
        saving: false,
      });
    }
  }, [state, outputPath, options.json, updateState, exit]);

  // Render based on current step
  const renderStep = () => {
    switch (state.step) {
      case 'metadata':
        return <MetadataStep onSubmit={handleMetadataSubmit} error={state.error} />;

      case 'entry_node':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Step 2: Select Entry Node</Text>
            <Text dimColor>The entry node is where data enters your pipeline</Text>
            <Box marginTop={1}>
              <Select
                items={nodeTypeItems.filter((item) =>
                  ['input', 'vad'].includes(item.value)
                )}
                onSelect={(item) => handleNodeTypeSelect(item.value, true)}
              />
            </Box>
          </Box>
        );

      case 'add_nodes':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Step 3: Add Processing Nodes</Text>
            <Text dimColor>Current nodes: {state.nodes.length}</Text>

            {state.subStep === 0 ? (
              <Box marginTop={1}>
                <Confirm
                  message="Add another node?"
                  onConfirm={(confirmed) => handleAddMoreNodes(confirmed)}
                />
              </Box>
            ) : (
              <Box marginTop={1}>
                <Select
                  items={nodeTypeItems}
                  onSelect={(item) => handleNodeTypeSelect(item.value)}
                />
              </Box>
            )}
          </Box>
        );

      case 'configure_node':
        return (
          <NodeConfigureStep
            node={state.tempNode}
            onSubmit={handleNodeConfigure}
          />
        );

      case 'add_edges':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Step 4: Connect Nodes</Text>
            <Text dimColor>Current edges: {state.edges.length}</Text>

            {state.subStep === 0 ? (
              <Box marginTop={1}>
                <Confirm
                  message="Add a connection between nodes?"
                  onConfirm={(confirmed) => handleAddMoreEdges(confirmed)}
                />
              </Box>
            ) : (
              <EdgeCreateStep
                nodes={state.nodes}
                existingEdges={state.edges}
                onSubmit={handleEdgeCreate}
              />
            )}
          </Box>
        );

      case 'review':
        return (
          <ReviewStep
            pipelineName={state.pipelineName}
            description={state.pipelineDescription}
            nodes={state.nodes}
            edges={state.edges}
            onSave={handleSave}
            onEdit={() => updateState({ step: 'metadata', subStep: 0 })}
          />
        );

      case 'save':
        return (
          <Box>
            <Spinner />
            <Text> Saving pipeline...</Text>
          </Box>
        );

      default:
        return <Text>Unknown step</Text>;
    }
  };

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box borderStyle="round" paddingX={2} paddingY={1} marginBottom={1}>
        <Text bold color="magenta">🔄 WaaV DAG Pipeline Builder</Text>
      </Box>

      {/* Error display */}
      {state.error && (
        <Box marginBottom={1}>
          <Alert type="error" message={state.error} />
        </Box>
      )}

      {/* Current step */}
      {renderStep()}

      {/* Footer */}
      <Box marginTop={2} borderStyle="single" borderTop paddingTop={1}>
        <Text dimColor>[ESC] Cancel  [Ctrl+S] Quick Save (in review)</Text>
      </Box>
    </Box>
  );
};

/**
 * Metadata input step
 */
interface MetadataStepProps {
  onSubmit: (name: string, description: string) => void;
  error: string | null;
}

const MetadataStep: React.FC<MetadataStepProps> = ({ onSubmit }) => {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [focusedField, setFocusedField] = useState<'name' | 'description' | 'submit'>('name');

  useInput((_input, key) => {
    if (key.tab || key.downArrow) {
      if (focusedField === 'name') setFocusedField('description');
      else if (focusedField === 'description') setFocusedField('submit');
      else setFocusedField('name');
    }
    if (key.upArrow) {
      if (focusedField === 'submit') setFocusedField('description');
      else if (focusedField === 'description') setFocusedField('name');
      else setFocusedField('submit');
    }
    if (key.return && focusedField === 'submit') {
      onSubmit(name, description);
    }
  });

  return (
    <Box flexDirection="column">
      <Text bold color="cyan">Step 1: Pipeline Metadata</Text>

      <Box marginTop={1} flexDirection="column">
        <Box>
          <Text color={focusedField === 'name' ? 'cyan' : undefined}>
            {focusedField === 'name' ? '▶ ' : '  '}Name:
          </Text>
          <TextInput
            value={name}
            onChange={setName}
            placeholder="My Voice Pipeline"
            focus={focusedField === 'name'}
          />
        </Box>

        <Box marginTop={1}>
          <Text color={focusedField === 'description' ? 'cyan' : undefined}>
            {focusedField === 'description' ? '▶ ' : '  '}Description:
          </Text>
          <TextInput
            value={description}
            onChange={setDescription}
            placeholder="A custom voice processing pipeline"
            focus={focusedField === 'description'}
          />
        </Box>

        <Box marginTop={1}>
          <Text
            color={focusedField === 'submit' ? 'green' : 'gray'}
            bold={focusedField === 'submit'}
          >
            {focusedField === 'submit' ? '▶ ' : '  '}[Press Enter to Continue]
          </Text>
        </Box>
      </Box>

      <Box marginTop={1}>
        <Text dimColor>[Tab/↓] Next field  [↑] Previous field  [Enter] Continue</Text>
      </Box>
    </Box>
  );
};

/**
 * Node configuration step
 */
interface NodeConfigureStepProps {
  node: Partial<DAGNodeConfig>;
  onSubmit: (config: Record<string, unknown>) => void;
}

const NodeConfigureStep: React.FC<NodeConfigureStepProps> = ({ node, onSubmit }) => {
  const nodeType = node.type as DAGNodeType;
  const info = NODE_TYPE_INFO[nodeType];
  const [config, setConfig] = useState<Record<string, string>>({});
  const [focusedIndex, setFocusedIndex] = useState(0);

  const configFields = info?.config_fields || [];

  useInput((_input, key) => {
    if (key.tab || key.downArrow) {
      setFocusedIndex((prev) => (prev + 1) % (configFields.length + 1));
    }
    if (key.upArrow) {
      setFocusedIndex((prev) => (prev - 1 + configFields.length + 1) % (configFields.length + 1));
    }
    if (key.return && focusedIndex === configFields.length) {
      // Parse config values
      const parsedConfig: Record<string, unknown> = {};
      for (const field of configFields) {
        const value = config[field.name] || field.default;
        if (field.type === 'number') {
          parsedConfig[field.name] = parseFloat(String(value)) || 0;
        } else {
          parsedConfig[field.name] = value;
        }
      }
      onSubmit(parsedConfig);
    }
  });

  return (
    <Box flexDirection="column">
      <Text bold color="cyan">
        Configure: {info?.icon} {info?.label}
      </Text>
      <Text dimColor>{info?.description}</Text>

      <Box marginTop={1} flexDirection="column">
        {configFields.map((field, index) => (
          <Box key={field.name} marginBottom={1}>
            <Text color={focusedIndex === index ? 'cyan' : undefined}>
              {focusedIndex === index ? '▶ ' : '  '}
              {field.name}
              {field.required ? ' *' : ''}:{' '}
            </Text>
            <TextInput
              value={config[field.name] || String(field.default ?? '')}
              onChange={(value) => setConfig({ ...config, [field.name]: value })}
              placeholder={String(field.default ?? `Enter ${field.name}`)}
              focus={focusedIndex === index}
            />
          </Box>
        ))}

        <Box marginTop={1}>
          <Text
            color={focusedIndex === configFields.length ? 'green' : 'gray'}
            bold={focusedIndex === configFields.length}
          >
            {focusedIndex === configFields.length ? '▶ ' : '  '}[Press Enter to Add Node]
          </Text>
        </Box>
      </Box>
    </Box>
  );
};

/**
 * Edge creation step
 */
interface EdgeCreateStepProps {
  nodes: DAGNodeConfig[];
  existingEdges: DAGEdge[];
  onSubmit: (sourceId: string, targetId: string) => void;
}

const EdgeCreateStep: React.FC<EdgeCreateStepProps> = ({ nodes, existingEdges, onSubmit }) => {
  const [sourceIndex, setSourceIndex] = useState(0);
  const [targetIndex, setTargetIndex] = useState(1);
  const [focusedField, setFocusedField] = useState<'source' | 'target' | 'submit'>('source');

  const nodeItems: SelectItem[] = nodes.map((node) => {
    const info = NODE_TYPE_INFO[node.type as DAGNodeType];
    return {
      label: `${info?.icon || '?'} ${node.name || node.id}`,
      value: node.id,
    };
  });

  // Check if edge already exists
  const edgeExists = (sourceId: string, targetId: string) =>
    existingEdges.some((e) => e.source === sourceId && e.target === targetId);

  useInput((_input, key) => {
    if (key.tab) {
      if (focusedField === 'source') setFocusedField('target');
      else if (focusedField === 'target') setFocusedField('submit');
      else setFocusedField('source');
    }
    if (focusedField === 'source') {
      if (key.upArrow) setSourceIndex((prev) => Math.max(0, prev - 1));
      if (key.downArrow) setSourceIndex((prev) => Math.min(nodes.length - 1, prev + 1));
    }
    if (focusedField === 'target') {
      if (key.upArrow) setTargetIndex((prev) => Math.max(0, prev - 1));
      if (key.downArrow) setTargetIndex((prev) => Math.min(nodes.length - 1, prev + 1));
    }
    if (key.return && focusedField === 'submit') {
      const sourceNode = nodes[sourceIndex];
      const targetNode = nodes[targetIndex];

      if (!sourceNode || !targetNode) {
        return; // Invalid node selection
      }

      const sourceId = sourceNode.id;
      const targetId = targetNode.id;

      if (sourceId === targetId) {
        return; // Can't connect to self
      }

      if (edgeExists(sourceId, targetId)) {
        return; // Edge already exists
      }

      onSubmit(sourceId, targetId);
    }
  });

  const sourceNode = nodes[sourceIndex];
  const targetNode = nodes[targetIndex];
  const canSubmit = sourceNode && targetNode &&
    sourceNode.id !== targetNode.id &&
    !edgeExists(sourceNode.id, targetNode.id);

  return (
    <Box flexDirection="column">
      <Text bold color="cyan">Create Connection</Text>

      <Box marginTop={1} flexDirection="column">
        <Box>
          <Text color={focusedField === 'source' ? 'cyan' : undefined}>
            {focusedField === 'source' ? '▶ ' : '  '}Source:
          </Text>
          <Text bold>{nodeItems[sourceIndex]?.label || 'None'}</Text>
          {focusedField === 'source' && <Text dimColor> [↑↓ to change]</Text>}
        </Box>

        <Box marginTop={1}>
          <Text color={focusedField === 'target' ? 'cyan' : undefined}>
            {focusedField === 'target' ? '▶ ' : '  '}Target:
          </Text>
          <Text bold>{nodeItems[targetIndex]?.label || 'None'}</Text>
          {focusedField === 'target' && <Text dimColor> [↑↓ to change]</Text>}
        </Box>

        <Box marginTop={1}>
          <Text
            color={focusedField === 'submit' && canSubmit ? 'green' : 'gray'}
            bold={focusedField === 'submit'}
          >
            {focusedField === 'submit' ? '▶ ' : '  '}
            {canSubmit ? '[Press Enter to Create Edge]' : '[Invalid connection]'}
          </Text>
        </Box>
      </Box>

      {/* Visual preview */}
      <Box marginTop={2} borderStyle="round" paddingX={2} paddingY={1}>
        <Text>
          {nodeItems[sourceIndex]?.label}
          <Text color="cyan"> ────▶ </Text>
          {nodeItems[targetIndex]?.label}
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Review step
 */
interface ReviewStepProps {
  pipelineName: string;
  description: string;
  nodes: DAGNodeConfig[];
  edges: DAGEdge[];
  onSave: () => void;
  onEdit: () => void;
}

const ReviewStep: React.FC<ReviewStepProps> = ({
  pipelineName,
  description,
  nodes,
  edges,
  onSave,
  onEdit,
}) => {
  const [focusedAction, setFocusedAction] = useState<'save' | 'edit'>('save');

  useInput((_input, key) => {
    if (key.tab || key.leftArrow || key.rightArrow) {
      setFocusedAction((prev) => (prev === 'save' ? 'edit' : 'save'));
    }
    if (key.return) {
      if (focusedAction === 'save') onSave();
      else onEdit();
    }
  });

  return (
    <Box flexDirection="column">
      <Text bold color="cyan">Review Pipeline</Text>

      <Card title="Pipeline Info" variant="primary">
        <Box flexDirection="column">
          <Text>
            <Text bold>Name:</Text> {pipelineName}
          </Text>
          <Text>
            <Text bold>Description:</Text> {description || '(none)'}
          </Text>
        </Box>
      </Card>

      <Box marginTop={1}>
        <Card title={`Nodes (${nodes.length})`} variant="info">
          <Box flexDirection="column">
            {nodes.map((node, index) => {
              const info = NODE_TYPE_INFO[node.type as DAGNodeType];
              return (
                <Text key={node.id}>
                  {index + 1}. {info?.icon} {node.name || node.type}
                </Text>
              );
            })}
          </Box>
        </Card>
      </Box>

      <Box marginTop={1}>
        <Card title={`Edges (${edges.length})`} variant="info">
          <Box flexDirection="column">
            {edges.length === 0 ? (
              <Text dimColor>No connections defined</Text>
            ) : (
              edges.map((edge, index) => {
                const sourceNode = nodes.find((n) => n.id === edge.source);
                const targetNode = nodes.find((n) => n.id === edge.target);
                return (
                  <Text key={edge.id}>
                    {index + 1}. {sourceNode?.name || 'Unknown'} → {targetNode?.name || 'Unknown'}
                  </Text>
                );
              })
            )}
          </Box>
        </Card>
      </Box>

      {/* ASCII Preview */}
      <Box marginTop={1}>
        <Card title="Pipeline Preview">
          <Box flexDirection="column">
            {renderASCIIPreview(nodes, edges)}
          </Box>
        </Card>
      </Box>

      {/* Actions */}
      <Box marginTop={2}>
        <Text
          color={focusedAction === 'save' ? 'green' : 'gray'}
          bold={focusedAction === 'save'}
          inverse={focusedAction === 'save'}
        >
          {' '}[Save Pipeline]
        </Text>
        <Text>  </Text>
        <Text
          color={focusedAction === 'edit' ? 'yellow' : 'gray'}
          bold={focusedAction === 'edit'}
          inverse={focusedAction === 'edit'}
        >
          {' '}[Edit]
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Render simple ASCII preview of pipeline
 */
function renderASCIIPreview(nodes: DAGNodeConfig[], edges: DAGEdge[]): React.ReactNode {
  if (nodes.length === 0) {
    return <Text dimColor>No nodes</Text>;
  }

  // Build adjacency list
  const adjacency: Map<string, string[]> = new Map();
  for (const edge of edges) {
    const targets = adjacency.get(edge.source) || [];
    targets.push(edge.target);
    adjacency.set(edge.source, targets);
  }

  // Find entry nodes (no incoming edges)
  const hasIncoming = new Set(edges.map((e) => e.target));
  const entryNodes = nodes.filter((n) => !hasIncoming.has(n.id));

  // Simple linear rendering
  const lines: React.ReactNode[] = [];

  const renderNode = (node: DAGNodeConfig, depth: number = 0) => {
    const info = NODE_TYPE_INFO[node.type as DAGNodeType];
    const indent = '  '.repeat(depth);
    const icon = info?.icon || '?';

    lines.push(
      <Text key={node.id}>
        {indent}┌─────────────────┐
      </Text>
    );
    lines.push(
      <Text key={`${node.id}-name`}>
        {indent}│ {icon} {(node.name || node.type).slice(0, 13).padEnd(13)} │
      </Text>
    );
    lines.push(
      <Text key={`${node.id}-bottom`}>
        {indent}└────────┬────────┘
      </Text>
    );

    const targets = adjacency.get(node.id) || [];
    if (targets.length > 0) {
      lines.push(
        <Text key={`${node.id}-arrow`}>
          {indent}         │
        </Text>
      );
      lines.push(
        <Text key={`${node.id}-arrow2`}>
          {indent}         ▼
        </Text>
      );
    }
  };

  // Render from entry nodes
  const rendered = new Set<string>();
  const queue = [...entryNodes];

  while (queue.length > 0) {
    const node = queue.shift()!;
    if (rendered.has(node.id)) continue;
    rendered.add(node.id);
    renderNode(node);

    const targets = adjacency.get(node.id) || [];
    for (const targetId of targets) {
      const targetNode = nodes.find((n) => n.id === targetId);
      if (targetNode && !rendered.has(targetId)) {
        queue.push(targetNode);
      }
    }
  }

  // Render any disconnected nodes
  for (const node of nodes) {
    if (!rendered.has(node.id)) {
      lines.push(<Text key={`${node.id}-disc`} dimColor>(disconnected)</Text>);
      renderNode(node);
    }
  }

  return <>{lines}</>;
}

/**
 * Main command function
 */
export async function dagCreateCommand(options: GlobalOptions & { output?: string }): Promise<void> {
  if (options.json) {
    // Non-interactive mode - output empty template
    const template: DAGPipeline = {
      id: nanoid(),
      name: 'New Pipeline',
      description: 'A voice processing pipeline',
      version: '1.0.0',
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      tags: [],
      nodes: [],
      edges: [],
      config: {
        sample_rate: 16000,
        channels: 1,
        timeout_ms: 30000,
        max_concurrent_nodes: 10,
        log_level: 'info',
        log_inputs: false,
        log_outputs: false,
      },
      variables: {},
    };

    console.log(JSON.stringify(template, null, 2));
    return;
  }

  logger.info('Starting interactive DAG pipeline builder...');
  logger.info('Press ESC at any time to cancel');
  logger.plain('');

  // Render interactive wizard
  render(<DAGCreateWizard options={options} outputPath={options.output} />);
}

export default dagCreateCommand;
