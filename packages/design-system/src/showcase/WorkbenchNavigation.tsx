import { SidebarMenuButtonBase as SidebarMenuButton } from "../components/ui/sidebar-menu-button";
import { createSignal, For, Show, type Component, type JSX } from "solid-js";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuBadge,
  useSidebar,
} from "../components/ui/sidebar";
import { Icon } from "../components/workspace/icon";
import { Button } from "../components/ui/button";
import type { WorkbenchTab } from "./WorkbenchApp";
export interface WorkbenchNavItem {
  id: WorkbenchTab;
  label: string;
  icon?: JSX.Element;
  count?: number;
  group: "workspace" | "repository" | "explore";
}
export const workbenchNavItems = (
  liveMode: boolean,
  count: number,
): WorkbenchNavItem[] => [
  {
    id: "repositories",
    label: "Repositories",
    icon: <Icon name="repo" />,
    group: "workspace",
  },
  {
    id: "admin",
    label: "Access",
    icon: <Icon name="shield-check" />,
    group: "workspace",
  },
  {
    id: "overview",
    label: "Overview",
    icon: <Icon name="layers" />,
    group: "repository",
  },
  {
    id: "swarm",
    label: "Worktrees",
    count,
    icon: <Icon name="git-branch" />,
    group: "repository",
  },
  {
    id: "tree",
    label: "Files",
    icon: <Icon name="folder" />,
    group: "repository",
  },
  {
    id: "diff",
    label: "Changes",
    icon: <Icon name="diff" />,
    group: "repository",
  },
  {
    id: "issues",
    label: "Issues · Demo",
    icon: <Icon name="git-pull-request" />,
    group: "explore",
  },
  {
    id: "actions",
    label: liveMode ? "Actions" : "Actions · Demo",
    icon: <Icon name="play" />,
    group: liveMode ? "repository" : "explore",
  },
  {
    id: "primitives",
    label: "Components",
    icon: <Icon name="sliders" />,
    group: "explore",
  },
];
export const WorkbenchNavigation: Component<{
  items: WorkbenchNavItem[];
  active: WorkbenchTab;
  repositoryName?: string;
  browseOnly?: boolean;
  liveMode: boolean;
  username?: string;
  onSelect: (tab: WorkbenchTab) => void;
}> = (props) => {
  const sidebar = useSidebar();
  const [exploreOpen, setExploreOpen] = createSignal(false);
  const exploring = () =>
    props.items.some(
      (item) => item.group === "explore" && item.id === props.active,
    );
  const select = (tab: WorkbenchTab) => {
    props.onSelect(tab);
    sidebar.setOpenMobile(false);
  };
  const menu = (group: WorkbenchNavItem["group"]) => (
    <SidebarMenu>
      <For
        each={props.items.filter(
          (item) =>
            item.group === group &&
            (!props.browseOnly || !["swarm", "actions"].includes(item.id)),
        )}
      >
        {(item) => (
          <SidebarMenuItem>
            <SidebarMenuButton
              as="button"
              type="button"
              isActive={props.active === item.id}
              aria-current={props.active === item.id ? "page" : undefined}
              class="h-10 gap-2.5 px-3 sm:h-8"
              onClick={() => select(item.id)}
            >
              {item.icon}
              <span class="truncate">{item.label}</span>
            </SidebarMenuButton>
            <Show when={item.count !== undefined}>
              <SidebarMenuBadge class="top-2.5 sm:top-1.5">
                {item.count}
              </SidebarMenuBadge>
            </Show>
          </SidebarMenuItem>
        )}
      </For>
    </SidebarMenu>
  );
  return (
    <Sidebar>
      <SidebarHeader class="h-14 justify-center px-5">
        <Button
          variant="ghost"
          size="icon"
          class="inline-flex size-8 items-center justify-center rounded-lg text-primary transition-colors hover:bg-sidebar-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="Gitust repositories"
          title="Gitust"
          onClick={() => select("repositories")}
        >
          <Icon name="repo" size={23} />
        </Button>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup class="pt-3">
          <SidebarGroupContent>
            <nav aria-label="Workspace views">{menu("workspace")}</nav>
          </SidebarGroupContent>
        </SidebarGroup>
        <Show when={props.repositoryName}>
          <SidebarGroup class="pt-5">
            <SidebarGroupLabel class="min-w-0 gap-1.5 text-xs">
              <Icon name="repo" size={12} class="shrink-0" />
              <span class="truncate" title={props.repositoryName}>
                {props.repositoryName}
              </span>
              <Show when={!props.liveMode}>
                <span class="ml-auto shrink-0 text-[10px]">Demo</span>
              </Show>
            </SidebarGroupLabel>
            <SidebarGroupContent>
              <nav aria-label="Repository views">{menu("repository")}</nav>
            </SidebarGroupContent>
          </SidebarGroup>
        </Show>
        <SidebarGroup class="mt-auto pt-5">
          <Button
            variant="ghost"
            size="sm"
            class="mb-1 h-8 w-full justify-start gap-2 px-3 text-xs font-normal text-muted-foreground"
            aria-expanded={exploreOpen() || exploring()}
            onClick={() => setExploreOpen((open) => !open)}
          >
            <Icon name="sliders" size={13} />
            Explore previews
          </Button>
          <Show when={exploreOpen() || exploring()}>
            <SidebarGroupContent>
              <nav aria-label="Demo and component previews">
                {menu("explore")}
              </nav>
            </SidebarGroupContent>
          </Show>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter class="px-3 pb-3 pt-2">
        <SidebarMenuButton
          as="button"
          type="button"
          class="h-10 gap-2.5 px-3 sm:h-9"
          onClick={() => select("admin")}
        >
          <Icon name={props.liveMode ? "users" : "key"} size={14} />
          <span class="truncate text-xs">
            {props.liveMode ? props.username || "Your account" : "Sign in"}
          </span>
        </SidebarMenuButton>
      </SidebarFooter>
    </Sidebar>
  );
};
