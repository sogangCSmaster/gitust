import "./tokens/tokens.css";
import "./primitives/primitives.css";
import "./patterns/patterns.css";

// Theme & Tokens
export { getTheme, setTheme, toggleTheme, type ThemeMode } from "./tokens/theme";

// Keyboard & Shortcut Engine
export {
  shortcutManager,
  formatShortcutKeys,
  type ShortcutDefinition,
  type KeystrokeTelemetry,
} from "./keyboard/shortcut-manager";

// Core Primitives
export {
  Icon,
  IconSpriteSheet,
  ICON_DEFS,
  type IconName,
  type IconProps,
} from "./primitives/Icon";
export { Kbd, type KbdProps } from "./primitives/Kbd";
export {
  Button,
  type ButtonProps,
  type ButtonVariant,
  type ButtonSize,
} from "./primitives/Button";
export {
  Badge,
  type BadgeProps,
  type BadgeVariant,
} from "./primitives/Badge";
export { Input, type InputProps } from "./primitives/Input";
export {
  Avatar,
  Tabs,
  type AvatarProps,
  type TabItem,
  type TabsProps,
} from "./primitives/TabsAndAvatar";
export { Dialog, type DialogProps } from "./primitives/Dialog";

// Domain-Specific High-Performance Patterns
export {
  CommandPalette,
  type CommandPaletteProps,
} from "./patterns/CommandPalette";
export {
  VirtualDiffViewer,
  type VirtualDiffViewerProps,
  type DiffLine,
  type DiffLineType,
} from "./patterns/VirtualDiffViewer";
export {
  VirtualFileTree,
  type VirtualFileTreeProps,
  type FileTreeNode,
} from "./patterns/VirtualFileTree";
export {
  IssueListView,
  filterIssueItems,
  type IssueListViewProps,
  type IssueItem,
} from "./patterns/IssueListView";
export {
  ActionsPipelineView,
  type ActionsPipelineViewProps,
  type ActionJobStep,
  type ActionLogLine,
} from "./patterns/ActionsPipelineView";
export {
  EnterpriseAdminView,
  type EnterpriseAdminViewProps,
  type EnterpriseUser,
} from "./patterns/EnterpriseAdminView";
export {
  AgentSwarmView,
  filterAgentSwarmTasks,
  type AgentSwarmViewProps,
  type AgentSandboxTask,
  type AgentProvider,
} from "./patterns/AgentSwarmView";
export { ActiveChordHUD } from "./patterns/ActiveChordHUD";

