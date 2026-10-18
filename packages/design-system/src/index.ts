import "./styles.css";
export {
  getTheme,
  setTheme,
  toggleTheme,
  type ThemeMode,
} from "./tokens/theme";
export {
  shortcutManager,
  formatShortcutKeys,
  type ShortcutDefinition,
  type KeystrokeTelemetry,
} from "./keyboard/shortcut-manager";
export {
  Icon,
  IconSpriteSheet,
  ICON_DEFS,
  type IconName,
  type IconProps,
} from "./components/workspace/icon";
export { Button, type ButtonProps } from "./components/ui/button";
export { Badge, type BadgeProps } from "./components/ui/badge";
export {
  KeyHint,
  StatusBadge,
  Field,
  PersonAvatar,
  type StatusTone as BadgeVariant,
} from "./components/workspace/presentation";
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
  type LiveAuthSessionInfo,
} from "./patterns/EnterpriseAdminView";
export {
  EnterpriseCredentialsPanel,
  type EnterpriseCredentialsPanelProps,
  type LiveInviteItem,
  type LivePatItem,
  type LiveSshKeyItem,
} from "./patterns/EnterpriseCredentialsPanel";
export {
  AgentSwarmView,
  filterAgentSwarmTasks,
  type AgentSwarmViewProps,
  type AgentSandboxTask,
  type AgentVerificationResult,
  type AgentVerificationStep,
  type AgentProvider,
} from "./patterns/AgentSwarmView";
export { ActiveChordHUD } from "./patterns/ActiveChordHUD";
export {
  SidebarMenuButton,
  SidebarInput,
  type SidebarMenuButtonProps,
} from "./components/ui/sidebar-extras";
