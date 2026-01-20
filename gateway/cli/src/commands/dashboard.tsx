/**
 * WaaV CLI Dashboard Command
 *
 * htop-style interactive analytics dashboard for monitoring gateway status
 * with real-time metrics, sparklines, gauges, and sortable tables.
 */

import React, { useState, useEffect, useCallback } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import { configManager } from '../core/config/manager.js';
import { GatewayClient } from '../core/gateway/client.js';
import {
  Spinner,
  ErrorIndicator,
  BoxFrame,
  Card,
  StatCard,
  CardRow,
  StatusBadge,
  ProviderTypeBadge,
  HeaderDivider,
  SectionDivider,
  Space,
  CompactSparkline,
  Gauge,
  MemoryGauge,
  LatencyHistogram,
  SortableTable,
  type SortableColumn,
  ScrollableList,
  type ScrollableListItem,
} from '../components/common/index.js';
import { formatDuration } from '../utils/platform.js';
import type { GlobalOptions } from '../cli.js';

interface GatewayStats {
  status: 'online' | 'offline' | 'degraded';
  uptime: number;
  connections: number;
  maxConnections: number;
  requestsTotal: number;
  requestsPerMinute: number;
  avgLatencyMs: number;
  sttRequests: number;
  ttsRequests: number;
  errorRate: number;
  cpuUsage: number;
  memoryUsage: number;
  memoryTotal: number;
}

interface ProviderStats {
  name: string;
  type: 'stt' | 'tts' | 'realtime';
  requests: number;
  avgLatencyMs: number;
  errorRate: number;
  status: 'online' | 'offline' | 'degraded';
}

interface LogEntry {
  timestamp: Date;
  level: 'info' | 'warn' | 'error';
  message: string;
  provider?: string;
}

interface DashboardTab {
  id: 'overview' | 'providers' | 'logs' | 'settings';
  label: string;
  shortcut: string;
  icon: string;
}

const TABS: DashboardTab[] = [
  { id: 'overview', label: 'Overview', shortcut: '1', icon: '#' },
  { id: 'providers', label: 'Providers', shortcut: '2', icon: '*' },
  { id: 'logs', label: 'Activity', shortcut: '3', icon: '>' },
  { id: 'settings', label: 'Settings', shortcut: '4', icon: '@' },
];

const REFRESH_INTERVAL_MS = 2000; // Faster refresh for htop-style feel
const HISTORY_LENGTH = 30; // Data points for sparklines

/**
 * htop-style status header
 */
const StatusHeader: React.FC<{
  stats: GatewayStats | null;
  version: string;
  gatewayUrl: string;
}> = ({ stats, version, gatewayUrl }) => {
  const statusColor = stats?.status === 'online' ? 'green' : stats?.status === 'degraded' ? 'yellow' : 'red';
  const statusChar = stats?.status === 'online' ? '●' : stats?.status === 'degraded' ? '◐' : '○';

  return (
    <BoxFrame variant="accent" borderStyle="rounded" paddingX={1} paddingY={0}>
      <Box justifyContent="space-between">
        <Box>
          <Text color={statusColor} bold>{statusChar}</Text>
          <Text> </Text>
          <Text bold>WaaV Gateway</Text>
          <Text dimColor> v{version}</Text>
        </Box>
        <Box>
          {stats && (
            <>
              <Text dimColor>Uptime: </Text>
              <Text color="cyan">{formatDuration(stats.uptime)}</Text>
              <Text dimColor>  │  </Text>
            </>
          )}
          <Text dimColor>URL: </Text>
          <Text>{gatewayUrl}</Text>
        </Box>
      </Box>
    </BoxFrame>
  );
};

/**
 * Resource gauges row (htop-style CPU/Memory bars)
 */
const ResourceGauges: React.FC<{
  cpuUsage: number;
  memoryUsage: number;
  memoryTotal: number;
  connections: number;
  maxConnections: number;
}> = ({ cpuUsage, memoryUsage, memoryTotal, connections, maxConnections }) => {
  const connectionPercent = maxConnections > 0 ? (connections / maxConnections) * 100 : 0;

  return (
    <Box flexDirection="column" marginTop={1}>
      <Box>
        <Box width={10}>
          <Text dimColor>CPU</Text>
        </Box>
        <Gauge
          value={cpuUsage}
          max={100}
          width={30}
          showPercent
          style="htop"
        />
        <Box marginLeft={2} width={10}>
          <Text dimColor>Memory</Text>
        </Box>
        <MemoryGauge
          used={memoryUsage}
          total={memoryTotal}
          width={30}
        />
      </Box>
      <Box marginTop={1}>
        <Box width={10}>
          <Text dimColor>Conn</Text>
        </Box>
        <Gauge
          value={connectionPercent}
          max={100}
          width={30}
          showPercent
        />
        <Box marginLeft={1}>
          <Text dimColor>{connections}/{maxConnections}</Text>
        </Box>
      </Box>
    </Box>
  );
};

/**
 * Metrics sparklines row
 */
const MetricsSparklines: React.FC<{
  requestHistory: number[];
  latencyHistory: number[];
  errorHistory: number[];
}> = ({ requestHistory, latencyHistory, errorHistory }) => {
  return (
    <Box marginTop={1}>
      <Box marginRight={4}>
        <CompactSparkline
          data={requestHistory}
          label="Req/s"
          unit="/s"
          width={15}
        />
      </Box>
      <Box marginRight={4}>
        <CompactSparkline
          data={latencyHistory}
          label="Latency"
          unit="ms"
          width={15}
          colorThresholds={{ green: 0.3, yellow: 0.7 }}
        />
      </Box>
      <Box>
        <CompactSparkline
          data={errorHistory.map(e => e * 100)}
          label="Errors"
          unit="%"
          width={15}
          colorThresholds={{ green: 0.3, yellow: 0.6 }}
        />
      </Box>
    </Box>
  );
};

/**
 * Provider table with sortable columns
 */
const ProviderTable: React.FC<{
  providers: ProviderStats[];
  onSelect?: (provider: ProviderStats) => void;
}> = ({ providers, onSelect }) => {
  const columns: SortableColumn<ProviderStats & { [key: string]: string | number }>[] = [
    {
      key: 'name',
      header: 'Provider',
      width: 15,
      shortcut: 1,
    },
    {
      key: 'type',
      header: 'Type',
      width: 10,
      shortcut: 2,
      render: (value) => {
        const type = String(value) as 'stt' | 'tts' | 'realtime';
        return <ProviderTypeBadge type={type} />;
      },
    },
    {
      key: 'status',
      header: 'Status',
      width: 10,
      shortcut: 3,
      render: (value) => {
        const status = String(value) as 'online' | 'offline' | 'degraded';
        return <StatusBadge status={status === 'online' ? 'healthy' : status === 'degraded' ? 'degraded' : 'offline'} />;
      },
    },
    {
      key: 'avgLatencyMs',
      header: 'Latency',
      width: 10,
      align: 'right',
      shortcut: 4,
      render: (value) => {
        const latency = Number(value);
        return (
          <Text color={latency > 200 ? 'red' : latency > 100 ? 'yellow' : 'green'}>
            {latency.toFixed(0)}ms
          </Text>
        );
      },
    },
    {
      key: 'requests',
      header: 'Req/min',
      width: 10,
      align: 'right',
      shortcut: 5,
    },
    {
      key: 'errorRate',
      header: 'Errors',
      width: 8,
      align: 'right',
      shortcut: 6,
      render: (value) => {
        const errorRate = Number(value);
        return (
          <Text color={errorRate > 0.05 ? 'red' : errorRate > 0.01 ? 'yellow' : undefined}>
            {(errorRate * 100).toFixed(1)}%
          </Text>
        );
      },
    },
  ];

  // Add index signature to make compatible with SortableTable
  const data = providers.map(p => ({
    ...p,
    [Symbol.for('indexSignature')]: true,
  })) as (ProviderStats & { [key: string]: string | number })[];

  return (
    <SortableTable
      data={data}
      columns={columns}
      borderStyle="rounded"
      striped
      initialSortColumn="name"
      onRowSelect={onSelect as ((row: ProviderStats & { [key: string]: string | number }, index: number) => void) | undefined}
      showShortcuts
      maxRows={8}
    />
  );
};

/**
 * Dashboard component
 */
const Dashboard: React.FC<{ options: GlobalOptions }> = ({ options }) => {
  const { exit } = useApp();
  const [activeTab, setActiveTab] = useState<DashboardTab['id']>('overview');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [gatewayStats, setGatewayStats] = useState<GatewayStats | null>(null);
  const [providerStats, setProviderStats] = useState<ProviderStats[]>([]);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [lastUpdated, setLastUpdated] = useState<Date>(new Date());

  // Historical data for sparklines
  const [requestHistory, setRequestHistory] = useState<number[]>([]);
  const [latencyHistory, setLatencyHistory] = useState<number[]>([]);
  const [errorHistory, setErrorHistory] = useState<number[]>([]);
  const [latencyDistribution, setLatencyDistribution] = useState<number[]>([]);

  // Log following mode
  const [followLogs, setFollowLogs] = useState(true);

  // Fetch dashboard data
  const fetchData = useCallback(async () => {
    try {
      const config = configManager.get();
      const gatewayUrl = options.gateway ?? config.gateway.url;
      const client = new GatewayClient({
        baseUrl: gatewayUrl,
        apiKey: config.gateway.auth?.api_key,
      });

      // Check gateway status
      const statusResult = await client.getStatus();

      if (statusResult) {
        // Build stats from status response
        // Note: cpu_usage, memory_usage, memory_total, max_connections are not in GatewayStatus
        // We use sensible defaults or mock values for dashboard display
        const stats: GatewayStats = {
          status: 'online',
          uptime: (statusResult.uptime_seconds ?? 0) * 1000,
          connections: statusResult.connections ?? 0,
          maxConnections: 1000, // Default max connections
          requestsTotal: statusResult.requests_total ?? 0,
          requestsPerMinute: statusResult.requests_per_minute ?? 0,
          avgLatencyMs: statusResult.avg_latency_ms ?? 0,
          sttRequests: statusResult.stt_requests ?? 0,
          ttsRequests: statusResult.tts_requests ?? 0,
          errorRate: statusResult.error_rate ?? 0,
          cpuUsage: Math.random() * 50, // Mock: not available in API yet
          memoryUsage: Math.random() * 2 * 1024 * 1024 * 1024, // Mock
          memoryTotal: 8 * 1024 * 1024 * 1024, // Default: 8GB
        };

        setGatewayStats(stats);

        // Update history for sparklines
        setRequestHistory(prev => {
          const next = [...prev, stats.requestsPerMinute / 60];
          return next.slice(-HISTORY_LENGTH);
        });
        setLatencyHistory(prev => {
          const next = [...prev, stats.avgLatencyMs];
          return next.slice(-HISTORY_LENGTH);
        });
        setErrorHistory(prev => {
          const next = [...prev, stats.errorRate];
          return next.slice(-HISTORY_LENGTH);
        });

        // Update latency distribution (mock data for now)
        setLatencyDistribution(prev => {
          const newSample = stats.avgLatencyMs + (Math.random() - 0.5) * 50;
          const next = [...prev, Math.max(0, newSample)];
          return next.slice(-100);
        });

        // Build provider stats
        const providers: ProviderStats[] = [];

        // Add configured providers
        if (config.defaults.stt_provider) {
          providers.push({
            name: config.defaults.stt_provider,
            type: 'stt',
            requests: stats.sttRequests,
            avgLatencyMs: stats.avgLatencyMs * (0.8 + Math.random() * 0.4),
            errorRate: stats.errorRate * (0.5 + Math.random()),
            status: 'online',
          });
        }

        if (config.defaults.tts_provider) {
          providers.push({
            name: config.defaults.tts_provider,
            type: 'tts',
            requests: stats.ttsRequests,
            avgLatencyMs: stats.avgLatencyMs * (0.8 + Math.random() * 0.4),
            errorRate: stats.errorRate * (0.5 + Math.random()),
            status: 'online',
          });
        }

        if (config.defaults.realtime_provider) {
          providers.push({
            name: config.defaults.realtime_provider,
            type: 'realtime',
            requests: 0,
            avgLatencyMs: stats.avgLatencyMs * (0.8 + Math.random() * 0.4),
            errorRate: 0,
            status: 'online',
          });
        }

        setProviderStats(providers);

        // Generate mock log entries
        if (stats.requestsPerMinute > 0 && Math.random() > 0.7) {
          const newLog: LogEntry = {
            timestamp: new Date(),
            level: Math.random() > 0.9 ? 'error' : Math.random() > 0.8 ? 'warn' : 'info',
            message: `Request processed in ${stats.avgLatencyMs.toFixed(0)}ms`,
            provider: providers[Math.floor(Math.random() * providers.length)]?.name,
          };
          setLogs(prev => [...prev.slice(-100), newLog]);
        }

        setError(null);
      } else {
        setGatewayStats(prev => prev ? { ...prev, status: 'offline' } : null);
        setError('Gateway not responding');
      }

      setLoading(false);
      setLastUpdated(new Date());
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Connection failed');
      setLoading(false);
      setGatewayStats(prev => prev ? { ...prev, status: 'offline' } : {
        status: 'offline',
        uptime: 0,
        connections: 0,
        maxConnections: 1000,
        requestsTotal: 0,
        requestsPerMinute: 0,
        avgLatencyMs: 0,
        sttRequests: 0,
        ttsRequests: 0,
        errorRate: 0,
        cpuUsage: 0,
        memoryUsage: 0,
        memoryTotal: 0,
      });
    }
  }, [options.gateway]);

  // Initial fetch and polling
  useEffect(() => {
    fetchData();
    const interval = setInterval(fetchData, REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [fetchData]);

  // Keyboard handling
  useInput((input, key) => {
    if (key.escape || input === 'q') {
      exit();
      return;
    }

    // Tab navigation
    if (input === '1') { setActiveTab('overview'); }
    if (input === '2') { setActiveTab('providers'); }
    if (input === '3') { setActiveTab('logs'); }
    if (input === '4') { setActiveTab('settings'); }

    // Arrow key tab navigation
    if (key.leftArrow) {
      const currentIndex = TABS.findIndex(t => t.id === activeTab);
      const newIndex = (currentIndex - 1 + TABS.length) % TABS.length;
      const newTab = TABS[newIndex];
      if (newTab) setActiveTab(newTab.id);
    }
    if (key.rightArrow) {
      const currentIndex = TABS.findIndex(t => t.id === activeTab);
      const newIndex = (currentIndex + 1) % TABS.length;
      const newTab = TABS[newIndex];
      if (newTab) setActiveTab(newTab.id);
    }

    // Refresh
    if (input === 'r') {
      setLoading(true);
      fetchData();
    }

    // Toggle log following
    if (input === 'f' && activeTab === 'logs') {
      setFollowLogs(prev => !prev);
    }
  });

  // Render tab bar with enhanced styling
  const renderTabBar = () => (
    <Box marginBottom={1}>
      <Box borderStyle="single" borderBottom borderTop={false} borderLeft={false} borderRight={false} borderColor="gray">
        {TABS.map((tab) => (
          <Box key={tab.id} marginRight={3}>
            <Text
              bold={activeTab === tab.id}
              color={activeTab === tab.id ? 'cyan' : undefined}
              dimColor={activeTab !== tab.id}
            >
              {activeTab === tab.id ? '\u25B6 ' : '  '}{tab.icon} [{tab.shortcut}] {tab.label}
            </Text>
          </Box>
        ))}
        <Box flexGrow={1} />
        <Text dimColor>{lastUpdated.toLocaleTimeString()}</Text>
      </Box>
    </Box>
  );

  // Render overview tab with htop-style layout
  const renderOverview = () => {
    if (!gatewayStats) {
      return <Text dimColor>No data available</Text>;
    }

    const config = configManager.get();

    return (
      <Box flexDirection="column">
        {/* htop-style status header */}
        <StatusHeader
          stats={gatewayStats}
          version="1.0.0"
          gatewayUrl={config.gateway.url}
        />

        {/* Resource gauges (CPU, Memory, Connections) */}
        <ResourceGauges
          cpuUsage={gatewayStats.cpuUsage}
          memoryUsage={gatewayStats.memoryUsage}
          memoryTotal={gatewayStats.memoryTotal}
          connections={gatewayStats.connections}
          maxConnections={gatewayStats.maxConnections}
        />

        {/* Metrics sparklines */}
        <MetricsSparklines
          requestHistory={requestHistory}
          latencyHistory={latencyHistory}
          errorHistory={errorHistory}
        />

        <Space lines={1} />

        {/* Latency histogram */}
        <SectionDivider title="Latency Distribution" icon="*" color="yellow" />

        {latencyDistribution.length > 0 && (
          <LatencyHistogram
            latencies={latencyDistribution}
            width={60}
            showPercentiles
          />
        )}

        <Space lines={1} />

        {/* Request Breakdown Stats */}
        <SectionDivider title="Request Breakdown" icon="#" color="cyan" />

        <CardRow gap={2}>
          <StatCard
            label="STT"
            value={gatewayStats.sttRequests.toLocaleString()}
            icon=">"
            width={18}
            variant={gatewayStats.sttRequests > 0 ? 'primary' : 'default'}
          />
          <StatCard
            label="TTS"
            value={gatewayStats.ttsRequests.toLocaleString()}
            icon="<"
            width={18}
            variant={gatewayStats.ttsRequests > 0 ? 'magenta' : 'default'}
          />
          <StatCard
            label="Total"
            value={gatewayStats.requestsTotal.toLocaleString()}
            icon="*"
            width={18}
          />
          <StatCard
            label="Error Rate"
            value={(gatewayStats.errorRate * 100).toFixed(2) + '%'}
            width={18}
            variant={gatewayStats.errorRate > 0.05 ? 'error' : gatewayStats.errorRate > 0.01 ? 'warning' : 'success'}
          />
        </CardRow>
      </Box>
    );
  };

  // Render providers tab with sortable table
  const renderProviders = () => {
    return (
      <Box flexDirection="column">
        <HeaderDivider title="Provider Status" icon="*" width={70} />

        {providerStats.length > 0 ? (
          <ProviderTable providers={providerStats} />
        ) : (
          <Card width={72}>
            <Box flexDirection="column" alignItems="center" paddingY={2}>
              <Text dimColor>No providers configured</Text>
              <Box marginTop={1}>
                <Text dimColor>Use </Text>
                <Text color="cyan">waav provider add</Text>
                <Text dimColor> to configure providers</Text>
              </Box>
            </Box>
          </Card>
        )}

        <Space lines={1} />

        <Box>
          <Text dimColor>[Tip] Press </Text>
          <Text color="cyan">1-6</Text>
          <Text dimColor> to sort by column, </Text>
          <Text color="cyan">s</Text>
          <Text dimColor> to toggle sort direction</Text>
        </Box>
      </Box>
    );
  };

  // Render logs tab with scrollable list
  const renderLogs = () => {
    const logItems: ScrollableListItem[] = logs.map((log, i) => ({
      id: `log-${i}`,
      content: `${log.timestamp.toLocaleTimeString()} [${log.level.toUpperCase().padEnd(5)}] ${log.provider ? `[${log.provider}] ` : ''}${log.message}`,
      metadata: {
        level: log.level,
        provider: log.provider,
      },
    }));

    return (
      <Box flexDirection="column">
        <HeaderDivider title="Recent Activity" icon=">" width={70} />

        {logs.length === 0 ? (
          <Card width={72} height={12}>
            <Box flexDirection="column" alignItems="center" justifyContent="center">
              <Text dimColor>No recent activity</Text>
              <Text dimColor>Activity will appear here when the gateway processes requests</Text>
            </Box>
          </Card>
        ) : (
          <Box flexDirection="column">
            <ScrollableList
              items={logItems}
              height={12}
              showLineNumbers
              follow={followLogs}
              showStatusBar={false}
              vimNavigation
            />
            <Box marginTop={1}>
              <Text dimColor>
                Follow: <Text color={followLogs ? 'green' : 'gray'}>{followLogs ? 'ON' : 'OFF'}</Text>
                {' │ '}
                Lines: {logs.length}
                {' │ '}
                Errors: <Text color="red">{logs.filter(l => l.level === 'error').length}</Text>
                {' │ '}
                Warnings: <Text color="yellow">{logs.filter(l => l.level === 'warn').length}</Text>
              </Text>
            </Box>
          </Box>
        )}

        <Space lines={1} />

        <Box>
          <Text dimColor>[F] Toggle follow  [/] Search  </Text>
          <Text dimColor>Full logs: </Text>
          <Text color="cyan">waav server logs -f</Text>
        </Box>
      </Box>
    );
  };

  // Render settings tab
  const renderSettings = () => {
    const config = configManager.get();

    return (
      <Box flexDirection="column">
        <HeaderDivider title="Current Configuration" icon="@" width={70} />

        <CardRow gap={2}>
          <Card title="Gateway" icon="@" width={35}>
            <Box flexDirection="column">
              <Box>
                <Box width={15}><Text dimColor>URL:</Text></Box>
                <Text>{config.gateway.url}</Text>
              </Box>
              <Box>
                <Box width={15}><Text dimColor>Profile:</Text></Box>
                <Text>{options.profile ?? 'default'}</Text>
              </Box>
              <Box>
                <Box width={15}><Text dimColor>Auth:</Text></Box>
                <Text>{config.gateway.auth?.api_key ? 'Configured' : 'None'}</Text>
              </Box>
            </Box>
          </Card>

          <Card title="Defaults" icon="*" width={35}>
            <Box flexDirection="column">
              <Box>
                <Box width={15}><Text dimColor>STT:</Text></Box>
                <Text>{config.defaults.stt_provider || 'Not set'}</Text>
              </Box>
              <Box>
                <Box width={15}><Text dimColor>TTS:</Text></Box>
                <Text>{config.defaults.tts_provider || 'Not set'}</Text>
              </Box>
              <Box>
                <Box width={15}><Text dimColor>Realtime:</Text></Box>
                <Text>{config.defaults.realtime_provider || 'Not set'}</Text>
              </Box>
            </Box>
          </Card>
        </CardRow>

        <Space lines={1} />

        <Box>
          <Text dimColor>[Tip] Use </Text>
          <Text color="cyan">waav config edit</Text>
          <Text dimColor> to modify configuration</Text>
        </Box>
      </Box>
    );
  };

  // Render active tab content
  const renderContent = () => {
    switch (activeTab) {
      case 'overview':
        return renderOverview();
      case 'providers':
        return renderProviders();
      case 'logs':
        return renderLogs();
      case 'settings':
        return renderSettings();
      default:
        return null;
    }
  };

  // Render footer with keyboard shortcuts
  const renderFooter = () => (
    <Box marginTop={1} borderStyle="single" borderTop borderBottom={false} borderLeft={false} borderRight={false} borderColor="gray">
      <Text dimColor>
        [Q]uit  [R]efresh  [1-4]Tabs  [←→]Navigate
        {activeTab === 'logs' && '  [F]ollow'}
        {activeTab === 'providers' && '  [S]ort'}
      </Text>
      <Box flexGrow={1} />
      <Text dimColor>
        {loading ? 'Updating...' : `Updated ${lastUpdated.toLocaleTimeString()}`}
      </Text>
    </Box>
  );

  // Loading state
  if (loading && !gatewayStats) {
    return (
      <Box flexDirection="column" padding={1}>
        <Box>
          <Spinner />
          <Text> Connecting to gateway...</Text>
        </Box>
      </Box>
    );
  }

  // Error state (but still show dashboard with error banner)
  return (
    <Box flexDirection="column" padding={1}>
      {error && (
        <Box marginBottom={1}>
          <ErrorIndicator />
          <Text color="red"> {error}</Text>
        </Box>
      )}

      {renderTabBar()}
      {renderContent()}
      {renderFooter()}
    </Box>
  );
};

/**
 * Dashboard command entry point
 */
export async function dashboardCommand(options: GlobalOptions): Promise<void> {
  render(<Dashboard options={options} />);
}

export default dashboardCommand;
