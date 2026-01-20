/**
 * WaaV CLI DAG Visualize Command
 *
 * ASCII DAG pipeline visualization with:
 * - Box-and-arrow rendering
 * - Color-coded node types
 * - Edge labels for conditions
 * - Horizontal and vertical layouts
 * - Export to text file
 */

import React from 'react';
import { render, Text, Box } from 'ink';
import * as fs from 'fs';
import * as yaml from 'js-yaml';
import * as path from 'path';

import { logger } from '../../utils/logger.js';
import type { GlobalOptions } from '../../cli.js';
import {
  Card,
} from '../../components/common/index.js';
import {
  DAGPipelineSchema,
  type DAGPipeline,
  type DAGNodeConfig,
  type DAGEdge,
  type DAGNodeType,
  NODE_TYPE_INFO,
} from '../../types/dag.js';

/**
 * Layout orientation
 */
type LayoutOrientation = 'horizontal' | 'vertical';

/**
 * Node position in grid
 */
interface NodePosition {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Rendered node box
 */
interface RenderedNode {
  id: string;
  lines: string[];
  color: string;
  position: NodePosition;
}

/**
 * ASCII box characters
 */
const BOX_CHARS = {
  topLeft: '┌',
  topRight: '┐',
  bottomLeft: '└',
  bottomRight: '┘',
  horizontal: '─',
  vertical: '│',
  arrow: '▶',
  arrowDown: '▼',
  arrowUp: '▲',
  arrowLeft: '◀',
  cross: '┼',
  teeRight: '├',
  teeLeft: '┤',
  teeDown: '┬',
  teeUp: '┴',
};

/**
 * Get node color based on type
 */
function getNodeColor(nodeType: DAGNodeType): string {
  const info = NODE_TYPE_INFO[nodeType];
  if (!info) return 'white';

  // Map hex colors to terminal colors
  const colorMap: Record<string, string> = {
    '#4CAF50': 'green',     // input
    '#F44336': 'red',       // output
    '#2196F3': 'blue',      // stt
    '#9C27B0': 'magenta',   // tts
    '#FF9800': 'yellow',    // llm
    '#607D8B': 'gray',      // transform
    '#795548': 'gray',      // filter
    '#00BCD4': 'cyan',      // switch
    '#3F51B5': 'blue',      // merge
    '#E91E63': 'magenta',   // split
    '#8BC34A': 'green',     // cache
    '#FFEB3B': 'yellow',    // webhook
    '#673AB7': 'magenta',   // function
    '#9E9E9E': 'gray',      // delay
    '#CDDC39': 'green',     // buffer
    '#00E676': 'green',     // vad
    '#FF5722': 'red',       // emotion
    '#009688': 'cyan',      // sentiment
  };

  return colorMap[info.color] || 'white';
}

/**
 * Render a single node as ASCII box
 */
function renderNodeBox(node: DAGNodeConfig, maxWidth: number = 18): RenderedNode {
  const info = NODE_TYPE_INFO[node.type as DAGNodeType];
  const icon = info?.icon || '?';
  const label = node.name || node.type;
  const truncatedLabel = label.length > maxWidth - 4
    ? label.slice(0, maxWidth - 7) + '...'
    : label;

  const contentWidth = Math.max(truncatedLabel.length + 3, 12);
  const boxWidth = contentWidth + 2; // +2 for borders

  // Build box lines
  const topBorder = BOX_CHARS.topLeft + BOX_CHARS.horizontal.repeat(contentWidth) + BOX_CHARS.topRight;
  const content = BOX_CHARS.vertical + ' ' + icon + ' ' + truncatedLabel.padEnd(contentWidth - 3) + BOX_CHARS.vertical;
  const typeLine = BOX_CHARS.vertical + ' ' + `(${node.type})`.padEnd(contentWidth - 1) + BOX_CHARS.vertical;
  const bottomBorder = BOX_CHARS.bottomLeft + BOX_CHARS.horizontal.repeat(contentWidth) + BOX_CHARS.bottomRight;

  return {
    id: node.id,
    lines: [topBorder, content, typeLine, bottomBorder],
    color: getNodeColor(node.type as DAGNodeType),
    position: { x: 0, y: 0, width: boxWidth, height: 4 },
  };
}

/**
 * Calculate node positions using topological sort
 */
function calculatePositions(
  nodes: DAGNodeConfig[],
  edges: DAGEdge[],
  orientation: LayoutOrientation
): Map<string, NodePosition> {
  const positions = new Map<string, NodePosition>();

  // Build adjacency list
  const adjacency = new Map<string, string[]>();
  const inDegree = new Map<string, number>();

  for (const node of nodes) {
    adjacency.set(node.id, []);
    inDegree.set(node.id, 0);
  }

  for (const edge of edges) {
    const targets = adjacency.get(edge.source) || [];
    targets.push(edge.target);
    adjacency.set(edge.source, targets);
    inDegree.set(edge.target, (inDegree.get(edge.target) || 0) + 1);
  }

  // Topological sort with levels
  const levels: string[][] = [];
  const queue: string[] = [];

  // Find entry points (nodes with no incoming edges)
  for (const node of nodes) {
    if ((inDegree.get(node.id) || 0) === 0) {
      queue.push(node.id);
    }
  }

  while (queue.length > 0) {
    const currentLevel: string[] = [];
    const nextQueue: string[] = [];

    for (const nodeId of queue) {
      currentLevel.push(nodeId);

      const neighbors = adjacency.get(nodeId) || [];
      for (const neighbor of neighbors) {
        const newDegree = (inDegree.get(neighbor) || 0) - 1;
        inDegree.set(neighbor, newDegree);
        if (newDegree === 0) {
          nextQueue.push(neighbor);
        }
      }
    }

    if (currentLevel.length > 0) {
      levels.push(currentLevel);
    }
    queue.length = 0;
    queue.push(...nextQueue);
  }

  // Handle any remaining nodes (in case of cycles)
  const placed = new Set(levels.flat());
  const remaining = nodes.filter((n) => !placed.has(n.id));
  if (remaining.length > 0) {
    levels.push(remaining.map((n) => n.id));
  }

  // Calculate positions based on levels
  const nodeWidth = 20;
  const nodeHeight = 6;
  const horizontalSpacing = 8;
  const verticalSpacing = 3;

  for (let levelIndex = 0; levelIndex < levels.length; levelIndex++) {
    const level = levels[levelIndex];
    if (!level) continue;

    for (let nodeIndex = 0; nodeIndex < level.length; nodeIndex++) {
      const nodeId = level[nodeIndex];
      if (!nodeId) continue;

      if (orientation === 'horizontal') {
        positions.set(nodeId, {
          x: levelIndex * (nodeWidth + horizontalSpacing),
          y: nodeIndex * (nodeHeight + verticalSpacing),
          width: nodeWidth,
          height: nodeHeight,
        });
      } else {
        positions.set(nodeId, {
          x: nodeIndex * (nodeWidth + horizontalSpacing),
          y: levelIndex * (nodeHeight + verticalSpacing),
          width: nodeWidth,
          height: nodeHeight,
        });
      }
    }
  }

  return positions;
}

/**
 * Render edges as ASCII connections
 */
function _renderEdgeConnections(
  edges: DAGEdge[],
  positions: Map<string, NodePosition>,
  _orientation: LayoutOrientation
): string[] {
  const lines: string[] = [];

  for (const edge of edges) {
    const sourcePos = positions.get(edge.source);
    const targetPos = positions.get(edge.target);

    if (!sourcePos || !targetPos) continue;

    // Determine connection type
    if (_orientation === 'horizontal') {
      // Source on left, target on right
      const arrowLine = `${edge.source.slice(0, 8)} ─────${BOX_CHARS.arrow} ${edge.target.slice(0, 8)}`;
      lines.push(arrowLine);
    } else {
      // Source on top, target below
      const arrowLine = `${edge.source.slice(0, 8)} ──┬──${BOX_CHARS.arrowDown} ${edge.target.slice(0, 8)}`;
      lines.push(arrowLine);
    }

    // Add condition label if present
    if (edge.condition) {
      lines.push(`   [condition: ${edge.condition.slice(0, 20)}...]`);
    }
  }

  return lines;
}

/**
 * Render complete DAG as ASCII art
 */
function renderDAG(
  pipeline: DAGPipeline,
  orientation: LayoutOrientation
): string[] {
  const nodes = pipeline.nodes;
  const edges = pipeline.edges;

  if (nodes.length === 0) {
    return ['(empty pipeline)'];
  }

  // Calculate positions (used for future edge rendering improvements)
  const _positions = calculatePositions(nodes, edges, orientation);

  // Build adjacency for graph traversal
  const adjacency = new Map<string, string[]>();
  for (const edge of edges) {
    const targets = adjacency.get(edge.source) || [];
    targets.push(edge.target);
    adjacency.set(edge.source, targets);
  }

  // Find entry nodes
  const hasIncoming = new Set(edges.map((e) => e.target));
  const entryNodes = nodes.filter((n) => !hasIncoming.has(n.id));

  // Render using BFS from entry nodes
  const output: string[] = [];
  const rendered = new Set<string>();
  const queue = [...entryNodes];
  const levels: DAGNodeConfig[][] = [];

  // Group nodes by level
  while (queue.length > 0) {
    const currentLevel: DAGNodeConfig[] = [];
    const nextQueue: DAGNodeConfig[] = [];

    for (const node of queue) {
      if (rendered.has(node.id)) continue;
      rendered.add(node.id);
      currentLevel.push(node);

      const targets = adjacency.get(node.id) || [];
      for (const targetId of targets) {
        const targetNode = nodes.find((n) => n.id === targetId);
        if (targetNode && !rendered.has(targetId)) {
          nextQueue.push(targetNode);
        }
      }
    }

    if (currentLevel.length > 0) {
      levels.push(currentLevel);
    }
    queue.length = 0;
    queue.push(...nextQueue);
  }

  // Add any disconnected nodes
  for (const node of nodes) {
    if (!rendered.has(node.id)) {
      levels.push([node]);
      rendered.add(node.id);
    }
  }

  // Render levels
  if (orientation === 'vertical') {
    for (let i = 0; i < levels.length; i++) {
      const level = levels[i];
      if (!level) continue;

      // Render nodes side by side
      const nodeBoxes = level.map((node) => renderNodeBox(node));
      const maxHeight = Math.max(...nodeBoxes.map((b) => b.lines.length));

      for (let lineIndex = 0; lineIndex < maxHeight; lineIndex++) {
        let line = '';
        for (let nodeIndex = 0; nodeIndex < nodeBoxes.length; nodeIndex++) {
          const box = nodeBoxes[nodeIndex];
          if (!box) continue;
          const boxLine = box.lines[lineIndex] || ' '.repeat(box.lines[0]?.length || 0);
          line += boxLine;
          if (nodeIndex < nodeBoxes.length - 1) {
            line += '   '; // Spacing between nodes
          }
        }
        output.push(line);
      }

      // Add connectors between levels
      if (i < levels.length - 1) {
        const nextLevel = levels[i + 1];
        if (!nextLevel) continue;

        // Draw arrows
        for (const node of level) {
          const targets = adjacency.get(node.id) || [];
          for (const targetId of targets) {
            if (nextLevel.some((n) => n.id === targetId)) {
              const nodeBox = nodeBoxes.find((b) => b.id === node.id);
              const centerOffset = nodeBox?.lines[0] ? Math.floor(nodeBox.lines[0].length / 2) : 10;
              output.push(' '.repeat(centerOffset) + '│');
              output.push(' '.repeat(centerOffset) + '▼');
            }
          }
        }
        output.push(''); // Empty line between levels
      }
    }
  } else {
    // Horizontal layout
    for (let levelIndex = 0; levelIndex < levels.length; levelIndex++) {
      const level = levels[levelIndex];
      if (!level) continue;

      for (const node of level) {
        const box = renderNodeBox(node);

        for (const line of box.lines) {
          output.push(line);
        }

        // Add arrow to next level
        if (levelIndex < levels.length - 1) {
          const targets = adjacency.get(node.id) || [];
          if (targets.length > 0) {
            output.push('        │');
            output.push('        └──────▶');
          }
        }
        output.push('');
      }
    }
  }

  return output;
}

/**
 * DAG Visualization Display Component
 */
interface DAGVisualizationDisplayProps {
  pipeline: DAGPipeline;
  filePath: string;
  orientation: LayoutOrientation;
  asciiArt: string[];
}

const DAGVisualizationDisplay: React.FC<DAGVisualizationDisplayProps> = ({
  pipeline,
  filePath,
  orientation,
  asciiArt,
}) => {
  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box borderStyle="round" paddingX={2} paddingY={1} marginBottom={1}>
        <Text bold color="magenta">
          🔄 DAG Pipeline: {pipeline.name}
        </Text>
      </Box>

      {/* Metadata */}
      <Box marginBottom={1}>
        <Text dimColor>File: {filePath}</Text>
        <Text dimColor> | </Text>
        <Text dimColor>Layout: {orientation}</Text>
        <Text dimColor> | </Text>
        <Text dimColor>Nodes: {pipeline.nodes.length}</Text>
        <Text dimColor> | </Text>
        <Text dimColor>Edges: {pipeline.edges.length}</Text>
      </Box>

      {/* Legend */}
      <Card title="Node Types" variant="info">
        <Box flexWrap="wrap">
          {Object.entries(NODE_TYPE_INFO).slice(0, 8).map(([type, info]) => (
            <Box key={type} marginRight={2}>
              <Text color={getNodeColor(type as DAGNodeType) as Parameters<typeof Text>[0]['color']}>
                {info.icon} {info.label}
              </Text>
            </Box>
          ))}
        </Box>
      </Card>

      {/* DAG Visualization */}
      <Box marginTop={1}>
        <Card title="Pipeline Visualization">
          <Box flexDirection="column">
            {asciiArt.map((line, index) => (
              <Text key={index}>{line}</Text>
            ))}
          </Box>
        </Card>
      </Box>

      {/* Node Details */}
      <Box marginTop={1}>
        <Card title="Node Details" variant="info">
          <Box flexDirection="column">
            {pipeline.nodes.map((node, index) => {
              const info = NODE_TYPE_INFO[node.type as DAGNodeType];
              return (
                <Box key={node.id}>
                  <Text color={getNodeColor(node.type as DAGNodeType) as Parameters<typeof Text>[0]['color']}>
                    {index + 1}. {info?.icon || '?'} {node.name || node.id}
                  </Text>
                  <Text dimColor> ({node.type})</Text>
                  {node.config && Object.keys(node.config).length > 0 && (
                    <Text dimColor>
                      {' '}[{Object.keys(node.config).join(', ')}]
                    </Text>
                  )}
                </Box>
              );
            })}
          </Box>
        </Card>
      </Box>

      {/* Edge Details */}
      {pipeline.edges.length > 0 && (
        <Box marginTop={1}>
          <Card title="Connections" variant="info">
            <Box flexDirection="column">
              {pipeline.edges.map((edge, index) => {
                const sourceNode = pipeline.nodes.find((n) => n.id === edge.source);
                const targetNode = pipeline.nodes.find((n) => n.id === edge.target);
                return (
                  <Box key={edge.id}>
                    <Text>
                      {index + 1}. {sourceNode?.name || edge.source}
                    </Text>
                    <Text color="cyan"> ─────▶ </Text>
                    <Text>{targetNode?.name || edge.target}</Text>
                    {edge.condition && (
                      <Text dimColor> [if: {edge.condition.slice(0, 20)}]</Text>
                    )}
                  </Box>
                );
              })}
            </Box>
          </Card>
        </Box>
      )}

      {/* Footer */}
      <Box marginTop={1} borderStyle="single" borderTop paddingTop={1}>
        <Text dimColor>
          Use `waav dag validate {path.basename(filePath)}` to validate this pipeline
        </Text>
      </Box>
    </Box>
  );
};

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
export async function dagVisualizeCommand(
  name: string,
  options: GlobalOptions & {
    layout?: 'horizontal' | 'vertical';
    output?: string;
    ascii?: boolean;
  }
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
        success: false,
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

  const orientation: LayoutOrientation = options.layout || 'vertical';

  // Render ASCII art
  const asciiArt = renderDAG(pipeline, orientation);

  // Export to file if requested
  if (options.output) {
    const outputPath = path.resolve(options.output);
    const outputContent = [
      `# DAG Pipeline: ${pipeline.name}`,
      `# File: ${filePath}`,
      `# Generated: ${new Date().toISOString()}`,
      '',
      ...asciiArt,
      '',
      '# Node Details:',
      ...pipeline.nodes.map((n, i) => `#   ${i + 1}. ${n.name || n.id} (${n.type})`),
      '',
      '# Connections:',
      ...pipeline.edges.map((e, i) => {
        const sourceNode = pipeline.nodes.find((n) => n.id === e.source);
        const targetNode = pipeline.nodes.find((n) => n.id === e.target);
        return `#   ${i + 1}. ${sourceNode?.name || e.source} -> ${targetNode?.name || e.target}`;
      }),
    ].join('\n');

    fs.writeFileSync(outputPath, outputContent, 'utf-8');
    logger.success(`Visualization exported to: ${outputPath}`);
  }

  // Output results
  if (options.json) {
    console.log(JSON.stringify({
      success: true,
      file: filePath,
      pipeline_name: pipeline.name,
      layout: orientation,
      nodes: pipeline.nodes.map((n) => ({
        id: n.id,
        name: n.name,
        type: n.type,
      })),
      edges: pipeline.edges.map((e) => ({
        source: e.source,
        target: e.target,
        condition: e.condition,
      })),
      ascii: asciiArt,
    }, null, 2));
  } else if (options.ascii) {
    // Plain ASCII output
    console.log(asciiArt.join('\n'));
  } else {
    render(
      <DAGVisualizationDisplay
        pipeline={pipeline}
        filePath={filePath}
        orientation={orientation}
        asciiArt={asciiArt}
      />
    );
  }
}

export default dagVisualizeCommand;
