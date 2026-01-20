/**
 * WaaV CLI Common UI Components
 *
 * Re-exports all common components
 */

export {
  Spinner,
  SuccessIndicator,
  ErrorIndicator,
  WarningIndicator,
  InfoIndicator,
  LoadingStep,
  MultiStepLoader,
  type SpinnerStyle,
} from './Spinner.js';

export {
  ProgressBar,
  IndeterminateProgress,
  StepProgress,
  TransferProgress,
  type ProgressBarStyle,
} from './ProgressBar.js';

export {
  Select,
  Confirm,
  MultiSelect,
  type SelectItem,
} from './Select.js';

export {
  TextInput,
  PasswordInput,
  TextArea,
} from './TextInput.js';

export {
  Table,
  KeyValueTable,
  StatusTable,
  type TableColumn,
  type TableBorderStyle,
} from './Table.js';

// Phase 2 UI Components

export {
  BoxFrame,
  MiniCard,
  Panel,
  type BoxFrameProps,
  type BoxFrameVariant,
  type BoxFrameBorderStyle,
} from './BoxFrame.js';

export {
  Alert,
  InlineAlert,
  Toast,
  Callout,
  SuccessAlert,
  WarningAlert,
  ErrorAlert,
  InfoAlert,
  TipAlert,
  type AlertProps,
  type AlertType,
} from './Alert.js';

export {
  Badge,
  StatusBadge,
  ProviderTypeBadge,
  ConfiguredBadge,
  CountBadge,
  TagGroup,
  LatencyBadge,
  FeatureBadge,
  RegionBadge,
  type BadgeProps,
  type BadgeVariant,
} from './Badge.js';

export {
  Card,
  StatCard,
  StatusCard,
  InfoCard,
  ProviderCard,
  CardGrid,
  CardRow,
  type CardProps,
} from './Card.js';

export {
  Divider,
  SectionDivider,
  Space,
  VerticalDivider,
  HeaderDivider,
  DoubleDivider,
  type DividerProps,
  type DividerStyle,
} from './Divider.js';

// Phase 3 htop/nvtop-style Components

export {
  Sparkline,
  CompactSparkline,
  SparklineGroup,
  BufferedSparkline,
  useSparklineBuffer,
  type SparklineProps,
  type SparklineStyle,
  type SparklineColorThresholds,
  type CompactSparklineProps,
  type SparklineGroupProps,
  type BufferedSparklineProps,
} from './Sparkline.js';

export {
  Gauge,
  CPUGauge,
  MemoryGauge,
  NetworkGauge,
  VerticalGauge,
  GaugeRow,
  type GaugeProps,
  type GaugeStyle,
  type GaugeOrientation,
  type GaugeColorThresholds,
  type CPUGaugeProps,
  type MemoryGaugeProps,
  type NetworkGaugeProps,
  type VerticalGaugeProps,
  type GaugeRowProps,
} from './Gauge.js';

export {
  Histogram,
  LatencyHistogram,
  StatsSummary,
  type HistogramProps,
  type HistogramBucket,
  type HistogramStyle,
  type HistogramOrientation,
  type PercentileMarker,
  type LatencyHistogramProps,
  type StatsSummaryProps,
} from './Histogram.js';

export {
  ScrollableList,
  LogViewer,
  useScrollableList,
  type ScrollableListProps,
  type ScrollableListItem,
  type LogViewerProps,
} from './ScrollableList.js';

export {
  SortableTable,
  ProviderTable,
  useSortableTable,
  type SortableTableProps,
  type SortableColumn,
  type SortableTableBorderStyle,
  type SortDirection,
  type ProviderTableProps,
  type ProviderTableRow,
} from './SortableTable.js';
