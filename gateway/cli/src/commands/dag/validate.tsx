/**
 * WaaV CLI DAG Validate Command
 *
 * Comprehensive DAG pipeline validation with:
 * - Zod schema validation
 * - Cycle detection
 * - Unreachable node detection
 * - Provider availability checks
 * - Detailed error messages with suggestions
 */

import React from 'react';
import { render, Text, Box } from 'ink';
import * as fs from 'fs';
import * as yaml from 'js-yaml';
import * as path from 'path';

import { logger } from '../../utils/logger.js';
import type { GlobalOptions } from '../../cli.js';
import {
  Alert,
  Card,
  StatusBadge,
} from '../../components/common/index.js';
import {
  DAGPipelineSchema,
  type DAGPipeline,
  type DAGValidationResult,
  type DAGNodeConfig,
  type DAGEdge,
  type DAGNodeType,
  NODE_TYPE_INFO,
} from '../../types/dag.js';

/**
 * Validation error/warning item
 */
interface ValidationItem {
  type: 'error' | 'warning';
  code: string;
  nodeId?: string;
  edgeId?: string;
  message: string;
  suggestion?: string;
  line?: number;
}

/**
 * Validation result display component
 */
interface ValidationResultDisplayProps {
  result: DAGValidationResult;
  filePath: string;
  items: ValidationItem[];
}

const ValidationResultDisplay: React.FC<ValidationResultDisplayProps> = ({
  result,
  filePath,
  items,
}) => {
  const errors = items.filter((i) => i.type === 'error');
  const warnings = items.filter((i) => i.type === 'warning');

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box borderStyle="round" paddingX={2} paddingY={1} marginBottom={1}>
        <Text bold color={result.valid ? 'green' : 'red'}>
          {result.valid ? '✓' : '✗'} DAG Validation: {path.basename(filePath)}
        </Text>
      </Box>

      {/* Overall status */}
      <Box marginBottom={1}>
        <StatusBadge status={result.valid ? 'online' : 'offline'} />
        <Text>
          {' '}
          {result.valid ? 'Pipeline is valid' : 'Pipeline has validation errors'}
        </Text>
      </Box>

      {/* Statistics */}
      <Card title="Pipeline Statistics" variant="info">
        <Box flexDirection="column">
          <Box>
            <Box width={20}><Text>Nodes:</Text></Box>
            <Text bold>{result.stats.node_count}</Text>
          </Box>
          <Box>
            <Box width={20}><Text>Edges:</Text></Box>
            <Text bold>{result.stats.edge_count}</Text>
          </Box>
          <Box>
            <Box width={20}><Text>Entry Points:</Text></Box>
            <Text bold>{result.stats.entry_points.length}</Text>
            {result.stats.entry_points.length > 0 && (
              <Text dimColor> ({result.stats.entry_points.join(', ')})</Text>
            )}
          </Box>
          <Box>
            <Box width={20}><Text>Exit Points:</Text></Box>
            <Text bold>{result.stats.exit_points.length}</Text>
            {result.stats.exit_points.length > 0 && (
              <Text dimColor> ({result.stats.exit_points.join(', ')})</Text>
            )}
          </Box>
          <Box>
            <Box width={20}><Text>Max Depth:</Text></Box>
            <Text bold>{result.stats.max_depth}</Text>
          </Box>
          <Box>
            <Box width={20}><Text>Has Cycles:</Text></Box>
            <Text bold color={result.stats.has_cycles ? 'red' : 'green'}>
              {result.stats.has_cycles ? 'Yes (invalid)' : 'No'}
            </Text>
          </Box>
        </Box>
      </Card>

      {/* Errors */}
      {errors.length > 0 && (
        <Box marginTop={1}>
          <Card title={`Errors (${errors.length})`} variant="error">
            <Box flexDirection="column">
              {errors.map((error, index) => (
                <Box key={index} marginBottom={1} flexDirection="column">
                  <Box>
                    <Text color="red" bold>✗ [{error.code}]</Text>
                    <Text> </Text>
                    {error.nodeId && (
                      <Text dimColor>Node: {error.nodeId} | </Text>
                    )}
                    {error.edgeId && (
                      <Text dimColor>Edge: {error.edgeId} | </Text>
                    )}
                    <Text>{error.message}</Text>
                  </Box>
                  {error.suggestion && (
                    <Box marginLeft={2}>
                      <Text color="cyan">💡 {error.suggestion}</Text>
                    </Box>
                  )}
                </Box>
              ))}
            </Box>
          </Card>
        </Box>
      )}

      {/* Warnings */}
      {warnings.length > 0 && (
        <Box marginTop={1}>
          <Card title={`Warnings (${warnings.length})`} variant="warning">
            <Box flexDirection="column">
              {warnings.map((warning, index) => (
                <Box key={index} marginBottom={1} flexDirection="column">
                  <Box>
                    <Text color="yellow" bold>⚠ [{warning.code}]</Text>
                    <Text> </Text>
                    {warning.nodeId && (
                      <Text dimColor>Node: {warning.nodeId} | </Text>
                    )}
                    <Text>{warning.message}</Text>
                  </Box>
                  {warning.suggestion && (
                    <Box marginLeft={2}>
                      <Text color="cyan">💡 {warning.suggestion}</Text>
                    </Box>
                  )}
                </Box>
              ))}
            </Box>
          </Card>
        </Box>
      )}

      {/* Success message */}
      {result.valid && (
        <Box marginTop={1}>
          <Alert type="success" message="Pipeline validation passed! Ready for deployment." />
        </Box>
      )}

      {/* Summary */}
      <Box marginTop={1} borderStyle="single" borderTop paddingTop={1}>
        <Text dimColor>
          Validation complete: {errors.length} error(s), {warnings.length} warning(s)
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Detect cycles in the DAG using DFS
 */
function detectCycles(nodes: DAGNodeConfig[], edges: DAGEdge[]): {
  hasCycles: boolean;
  cycleNodes: string[];
} {
  const adjacency = new Map<string, string[]>();
  for (const edge of edges) {
    const targets = adjacency.get(edge.source) || [];
    targets.push(edge.target);
    adjacency.set(edge.source, targets);
  }

  const visited = new Set<string>();
  const recStack = new Set<string>();
  const cycleNodes: string[] = [];

  function dfs(nodeId: string): boolean {
    visited.add(nodeId);
    recStack.add(nodeId);

    const neighbors = adjacency.get(nodeId) || [];
    for (const neighbor of neighbors) {
      if (!visited.has(neighbor)) {
        if (dfs(neighbor)) {
          cycleNodes.push(nodeId);
          return true;
        }
      } else if (recStack.has(neighbor)) {
        cycleNodes.push(neighbor);
        cycleNodes.push(nodeId);
        return true;
      }
    }

    recStack.delete(nodeId);
    return false;
  }

  for (const node of nodes) {
    if (!visited.has(node.id)) {
      if (dfs(node.id)) {
        return { hasCycles: true, cycleNodes };
      }
    }
  }

  return { hasCycles: false, cycleNodes: [] };
}

/**
 * Find unreachable nodes (not reachable from any entry point)
 */
function findUnreachableNodes(
  nodes: DAGNodeConfig[],
  edges: DAGEdge[],
  entryPoints: string[]
): string[] {
  const adjacency = new Map<string, string[]>();
  for (const edge of edges) {
    const targets = adjacency.get(edge.source) || [];
    targets.push(edge.target);
    adjacency.set(edge.source, targets);
  }

  const reachable = new Set<string>();

  function bfs(startId: string) {
    const queue = [startId];
    while (queue.length > 0) {
      const nodeId = queue.shift()!;
      if (reachable.has(nodeId)) continue;
      reachable.add(nodeId);

      const neighbors = adjacency.get(nodeId) || [];
      for (const neighbor of neighbors) {
        if (!reachable.has(neighbor)) {
          queue.push(neighbor);
        }
      }
    }
  }

  // Start BFS from all entry points
  for (const entryId of entryPoints) {
    bfs(entryId);
  }

  // Find nodes not in reachable set
  return nodes
    .filter((node) => !reachable.has(node.id))
    .map((node) => node.id);
}

/**
 * Calculate max depth of the DAG
 */
function calculateMaxDepth(
  _nodes: DAGNodeConfig[],
  edges: DAGEdge[],
  entryPoints: string[]
): number {
  const adjacency = new Map<string, string[]>();
  for (const edge of edges) {
    const targets = adjacency.get(edge.source) || [];
    targets.push(edge.target);
    adjacency.set(edge.source, targets);
  }

  const depths = new Map<string, number>();

  function calculateDepth(nodeId: string, visited: Set<string>): number {
    if (depths.has(nodeId)) {
      return depths.get(nodeId)!;
    }

    if (visited.has(nodeId)) {
      return 0; // Cycle detected, avoid infinite recursion
    }

    visited.add(nodeId);

    const neighbors = adjacency.get(nodeId) || [];
    let maxChildDepth = 0;

    for (const neighbor of neighbors) {
      const childDepth = calculateDepth(neighbor, visited);
      maxChildDepth = Math.max(maxChildDepth, childDepth);
    }

    visited.delete(nodeId);

    const depth = 1 + maxChildDepth;
    depths.set(nodeId, depth);
    return depth;
  }

  let maxDepth = 0;
  for (const entryId of entryPoints) {
    const depth = calculateDepth(entryId, new Set());
    maxDepth = Math.max(maxDepth, depth);
  }

  return maxDepth;
}

/**
 * Validate a DAG pipeline
 */
function validatePipeline(pipeline: DAGPipeline): {
  result: DAGValidationResult;
  items: ValidationItem[];
} {
  const items: ValidationItem[] = [];
  const nodes = pipeline.nodes;
  const edges = pipeline.edges;

  // 1. Check for empty pipeline
  if (nodes.length === 0) {
    items.push({
      type: 'error',
      code: 'EMPTY_PIPELINE',
      message: 'Pipeline has no nodes',
      suggestion: 'Add at least one node using `waav dag create`',
    });
  }

  // 2. Validate node IDs are unique
  const nodeIds = new Set<string>();
  for (const node of nodes) {
    if (nodeIds.has(node.id)) {
      items.push({
        type: 'error',
        code: 'DUPLICATE_NODE_ID',
        nodeId: node.id,
        message: `Duplicate node ID: ${node.id}`,
        suggestion: 'Each node must have a unique ID',
      });
    }
    nodeIds.add(node.id);
  }

  // 3. Validate edge references exist
  for (const edge of edges) {
    if (!nodeIds.has(edge.source)) {
      items.push({
        type: 'error',
        code: 'INVALID_EDGE_SOURCE',
        edgeId: edge.id,
        message: `Edge source "${edge.source}" does not exist`,
        suggestion: 'Verify the source node ID exists in the nodes array',
      });
    }
    if (!nodeIds.has(edge.target)) {
      items.push({
        type: 'error',
        code: 'INVALID_EDGE_TARGET',
        edgeId: edge.id,
        message: `Edge target "${edge.target}" does not exist`,
        suggestion: 'Verify the target node ID exists in the nodes array',
      });
    }
  }

  // 4. Check for duplicate edges
  const edgeKeys = new Set<string>();
  for (const edge of edges) {
    const key = `${edge.source}->${edge.target}`;
    if (edgeKeys.has(key)) {
      items.push({
        type: 'warning',
        code: 'DUPLICATE_EDGE',
        edgeId: edge.id,
        message: `Duplicate edge from ${edge.source} to ${edge.target}`,
        suggestion: 'Remove duplicate edge definitions',
      });
    }
    edgeKeys.add(key);
  }

  // 5. Detect cycles
  const { hasCycles, cycleNodes } = detectCycles(nodes, edges);
  if (hasCycles) {
    items.push({
      type: 'error',
      code: 'CYCLE_DETECTED',
      nodeId: cycleNodes.join(', '),
      message: 'Pipeline contains a cycle which would cause infinite loop',
      suggestion: 'Remove one of the edges creating the cycle',
    });
  }

  // 6. Find entry and exit points
  const hasIncoming = new Set(edges.map((e) => e.target));
  const hasOutgoing = new Set(edges.map((e) => e.source));

  const entryPoints = nodes
    .filter((n) => !hasIncoming.has(n.id))
    .map((n) => n.id);

  const exitPoints = nodes
    .filter((n) => !hasOutgoing.has(n.id))
    .map((n) => n.id);

  // 7. Warn about no entry points
  if (entryPoints.length === 0 && nodes.length > 0) {
    items.push({
      type: 'warning',
      code: 'NO_ENTRY_POINT',
      message: 'No entry point found (all nodes have incoming edges)',
      suggestion: 'Add an input node or remove an incoming edge to create an entry point',
    });
  }

  // 8. Warn about no exit points
  if (exitPoints.length === 0 && nodes.length > 0) {
    items.push({
      type: 'warning',
      code: 'NO_EXIT_POINT',
      message: 'No exit point found (all nodes have outgoing edges)',
      suggestion: 'Add an output node or remove an outgoing edge to create an exit point',
    });
  }

  // 9. Find unreachable nodes
  const unreachable = findUnreachableNodes(nodes, edges, entryPoints);
  for (const nodeId of unreachable) {
    items.push({
      type: 'warning',
      code: 'UNREACHABLE_NODE',
      nodeId,
      message: `Node "${nodeId}" is not reachable from any entry point`,
      suggestion: 'Connect this node to the pipeline or remove it',
    });
  }

  // 10. Validate node types
  for (const node of nodes) {
    if (!NODE_TYPE_INFO[node.type as DAGNodeType]) {
      items.push({
        type: 'error',
        code: 'INVALID_NODE_TYPE',
        nodeId: node.id,
        message: `Unknown node type: ${node.type}`,
        suggestion: `Valid types: ${Object.keys(NODE_TYPE_INFO).join(', ')}`,
      });
    }
  }

  // 11. Validate type compatibility on edges
  for (const edge of edges) {
    const sourceNode = nodes.find((n) => n.id === edge.source);
    const targetNode = nodes.find((n) => n.id === edge.target);

    if (sourceNode && targetNode) {
      const sourceInfo = NODE_TYPE_INFO[sourceNode.type as DAGNodeType];
      const targetInfo = NODE_TYPE_INFO[targetNode.type as DAGNodeType];

      if (sourceInfo && targetInfo) {
        const sourceOutput = sourceInfo.outputs.find(
          (o) => o.name === (edge.source_output || 'default')
        ) || sourceInfo.outputs[0];

        const targetInput = targetInfo.inputs.find(
          (i) => i.name === (edge.target_input || 'default')
        ) || targetInfo.inputs[0];

        if (sourceOutput && targetInput) {
          // Check type compatibility
          if (
            sourceOutput.type !== 'any' &&
            targetInput.type !== 'any' &&
            sourceOutput.type !== targetInput.type
          ) {
            items.push({
              type: 'warning',
              code: 'TYPE_MISMATCH',
              edgeId: edge.id,
              message: `Type mismatch: ${sourceNode.name || sourceNode.id} outputs ${sourceOutput.type}, but ${targetNode.name || targetNode.id} expects ${targetInput.type}`,
              suggestion: 'Add a transform node to convert between types',
            });
          }
        }
      }
    }
  }

  // 12. Check for missing required configuration
  for (const node of nodes) {
    const info = NODE_TYPE_INFO[node.type as DAGNodeType];
    if (info) {
      for (const field of info.config_fields) {
        if (field.required && (!node.config || node.config[field.name] === undefined)) {
          items.push({
            type: 'error',
            code: 'MISSING_CONFIG',
            nodeId: node.id,
            message: `Missing required config: ${field.name}`,
            suggestion: `Add "${field.name}" to node configuration`,
          });
        }
      }
    }
  }

  // 13. Warn about disabled nodes
  const disabledNodes = nodes.filter((n) => !n.enabled);
  if (disabledNodes.length > 0) {
    items.push({
      type: 'warning',
      code: 'DISABLED_NODES',
      message: `${disabledNodes.length} node(s) are disabled`,
      suggestion: 'Disabled nodes will be skipped during execution',
    });
  }

  // 14. Warn about disabled edges
  const disabledEdges = edges.filter((e) => !e.enabled);
  if (disabledEdges.length > 0) {
    items.push({
      type: 'warning',
      code: 'DISABLED_EDGES',
      message: `${disabledEdges.length} edge(s) are disabled`,
      suggestion: 'Disabled edges will be skipped during execution',
    });
  }

  // Calculate max depth
  const maxDepth = calculateMaxDepth(nodes, edges, entryPoints);

  // Build result
  const hasErrors = items.some((i) => i.type === 'error');

  const result: DAGValidationResult = {
    valid: !hasErrors,
    errors: items.map((i) => ({
      type: i.type,
      node_id: i.nodeId,
      edge_id: i.edgeId,
      message: i.message,
      suggestion: i.suggestion,
    })),
    stats: {
      node_count: nodes.length,
      edge_count: edges.length,
      entry_points: entryPoints,
      exit_points: exitPoints,
      max_depth: maxDepth,
      has_cycles: hasCycles,
    },
  };

  return { result, items };
}

/**
 * Load and parse a DAG file
 */
function loadDAGFile(filePath: string): {
  pipeline: DAGPipeline | null;
  parseError: string | null;
} {
  try {
    const absolutePath = path.resolve(filePath);

    if (!fs.existsSync(absolutePath)) {
      return { pipeline: null, parseError: `File not found: ${absolutePath}` };
    }

    const content = fs.readFileSync(absolutePath, 'utf-8');

    // Determine format based on extension
    let rawData: unknown;
    if (absolutePath.endsWith('.json')) {
      rawData = JSON.parse(content);
    } else {
      // Assume YAML
      rawData = yaml.load(content);
    }

    // Validate with Zod
    const parseResult = DAGPipelineSchema.safeParse(rawData);

    if (!parseResult.success) {
      const errorMessages = parseResult.error.errors
        .map((e) => `${e.path.join('.')}: ${e.message}`)
        .join('\n');
      return {
        pipeline: null,
        parseError: `Schema validation failed:\n${errorMessages}`,
      };
    }

    return { pipeline: parseResult.data, parseError: null };
  } catch (error) {
    return {
      pipeline: null,
      parseError: `Failed to parse file: ${error instanceof Error ? error.message : 'Unknown error'}`,
    };
  }
}

/**
 * Main command function
 */
export async function dagValidateCommand(
  name: string,
  options: GlobalOptions & { fix?: boolean }
): Promise<void> {
  // Resolve file path
  let filePath = name;

  // If no extension, try common patterns
  if (!name.includes('.')) {
    const possiblePaths = [
      `${name}.dag.yaml`,
      `${name}.dag.yml`,
      `${name}.dag.json`,
      `${name}.yaml`,
      `${name}.yml`,
      `${name}.json`,
    ];

    for (const p of possiblePaths) {
      if (fs.existsSync(p)) {
        filePath = p;
        break;
      }
    }
  }

  // Load the file
  const { pipeline, parseError } = loadDAGFile(filePath);

  if (parseError) {
    if (options.json) {
      console.log(JSON.stringify({
        valid: false,
        error: parseError,
        file: filePath,
      }));
    } else {
      logger.error(`Failed to load DAG file: ${filePath}`);
      logger.error(parseError);
    }
    process.exit(1);
    return;
  }

  if (!pipeline) {
    logger.error('Failed to load pipeline');
    process.exit(1);
    return;
  }

  // Validate the pipeline
  const { result, items } = validatePipeline(pipeline);

  // Output results
  if (options.json) {
    console.log(JSON.stringify({
      valid: result.valid,
      file: filePath,
      pipeline_name: pipeline.name,
      errors: items.filter((i) => i.type === 'error'),
      warnings: items.filter((i) => i.type === 'warning'),
      stats: result.stats,
    }, null, 2));
  } else {
    render(
      <ValidationResultDisplay
        result={result}
        filePath={filePath}
        items={items}
      />
    );
  }

  // Exit with appropriate code
  if (!result.valid) {
    process.exit(1);
  }
}

export default dagValidateCommand;
