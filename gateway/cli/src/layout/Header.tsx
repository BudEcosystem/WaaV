/**
 * WaaV CLI Header Component
 *
 * Displays breadcrumbs, gateway status indicator, and version info
 * at the top of the application shell.
 */

import React from 'react';
import { Text, Box } from 'ink';
import { useNavigation, useBreadcrumbs } from '../hooks/useNavigation.js';
import { useGatewayStatus, useVersion } from '../hooks/useAppState.js';
import { Badge } from '../components/common/index.js';
import type { BreadcrumbItem, ScreenName } from '../types/navigation.js';

/**
 * Header component props
 */
interface HeaderProps {
  /** Whether to show breadcrumbs */
  showBreadcrumbs?: boolean;
  /** Whether to show gateway status */
  showGatewayStatus?: boolean;
  /** Whether to show version */
  showVersion?: boolean;
  /** Optional title override */
  title?: string;
}

/**
 * Render breadcrumb navigation trail
 */
const Breadcrumbs: React.FC<{ items: BreadcrumbItem[]; onNavigate?: (screen: ScreenName) => void }> = ({
  items,
  onNavigate: _onNavigate,
}) => {
  if (items.length === 0) {
    return null;
  }

  return (
    <Box>
      {items.map((item, index) => {
        const isLast = index === items.length - 1;

        return (
          <Box key={`${item.screen}-${index}`}>
            <Text
              color={isLast ? 'cyan' : 'gray'}
              bold={isLast}
              dimColor={!isLast}
            >
              {item.title}
            </Text>
            {!isLast && (
              <Text dimColor> {'>'} </Text>
            )}
          </Box>
        );
      })}
    </Box>
  );
};

/**
 * Gateway status indicator
 */
const GatewayStatusIndicator: React.FC = () => {
  const { status, isConnected, isConnecting, hasError } = useGatewayStatus();

  if (isConnecting) {
    return (
      <Box>
        <Text color="yellow">●</Text>
        <Text dimColor> Connecting...</Text>
      </Box>
    );
  }

  if (hasError) {
    return (
      <Box>
        <Badge label="Error" variant="error" />
      </Box>
    );
  }

  if (isConnected && status) {
    return (
      <Box>
        <Badge label="Connected" variant="success" />
        {status.connections > 0 && (
          <Text dimColor> ({status.connections} conn)</Text>
        )}
      </Box>
    );
  }

  return (
    <Box>
      <Badge label="Offline" variant="error" />
    </Box>
  );
};

/**
 * Header component for the application shell
 */
export const Header: React.FC<HeaderProps> = ({
  showBreadcrumbs = true,
  showGatewayStatus = true,
  showVersion = true,
  title,
}) => {
  const breadcrumbs = useBreadcrumbs();
  const { navigate } = useNavigation();
  const version = useVersion();

  return (
    <Box
      flexDirection="row"
      justifyContent="space-between"
      paddingX={1}
      paddingY={0}
      borderStyle="single"
      borderColor="gray"
      borderTop
      borderLeft
      borderRight
      borderBottom={false}
    >
      {/* Left side: Breadcrumbs or Title */}
      <Box flexGrow={1}>
        {title ? (
          <Text bold color="cyan">{title}</Text>
        ) : showBreadcrumbs ? (
          <Breadcrumbs
            items={breadcrumbs}
            onNavigate={navigate}
          />
        ) : (
          <Text bold color="cyan">WaaV</Text>
        )}
      </Box>

      {/* Right side: Status indicators */}
      <Box>
        {showGatewayStatus && (
          <Box marginRight={2}>
            <Text dimColor>Gateway: </Text>
            <GatewayStatusIndicator />
          </Box>
        )}
        {showVersion && (
          <Box>
            <Text dimColor>v{version}</Text>
          </Box>
        )}
      </Box>
    </Box>
  );
};

/**
 * Compact header variant for space-constrained views
 */
export const CompactHeader: React.FC<{ title?: string }> = ({ title }) => {
  const { isConnected } = useGatewayStatus();
  const { currentScreen } = useNavigation();

  const displayTitle = title || currentScreen.replace(/_/g, ' ');

  return (
    <Box
      flexDirection="row"
      justifyContent="space-between"
      paddingX={1}
    >
      <Text bold color="cyan">{displayTitle}</Text>
      <Box>
        <Text color={isConnected ? 'green' : 'red'}>●</Text>
        <Text dimColor> {isConnected ? 'Online' : 'Offline'}</Text>
      </Box>
    </Box>
  );
};

export default Header;
