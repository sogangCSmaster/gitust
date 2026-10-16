import { For, Show, type Component, type JSX } from "solid-js";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuBadge,
  useSidebar,
} from "../components/ui/sidebar";
import { Badge } from "../components/ui/badge";
import { Icon } from "../components/workspace/icon";
import type { WorkbenchTab } from "./WorkbenchApp";
export interface WorkbenchNavItem {
  id: WorkbenchTab;
  label: string;
  icon?: JSX.Element;
  count?: number;
}
export const WorkbenchNavigation: Component<{
  items: WorkbenchNavItem[];
  active: WorkbenchTab;
  onSelect: (tab: WorkbenchTab) => void;
}> = (props) => {
  const sidebar = useSidebar();
  return (
    <Sidebar>
      <SidebarHeader class="px-4 py-5">
        <div class="flex items-center gap-2.5">
          <Icon name="layers" size={22} />
          <span class="text-lg font-semibold tracking-tight">gitust</span>
          <Badge variant="outline" class="ml-auto text-[10px] font-normal">
            Preview
          </Badge>
        </div>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupLabel>Workspace</SidebarGroupLabel>
          <SidebarGroupContent>
            <nav aria-label="Workspace views">
              <SidebarMenu>
                <For each={props.items}>
                  {(item) => (
                    <SidebarMenuItem>
                      <SidebarMenuButton
                        as="button"
                        type="button"
                        isActive={props.active === item.id}
                        aria-current={
                          props.active === item.id ? "page" : undefined
                        }
                        class="h-11 gap-2.5 px-3 sm:h-9"
                        onClick={() => {
                          props.onSelect(item.id);
                          sidebar.setOpenMobile(false);
                        }}
                      >
                        {item.icon}
                        <span>{item.label}</span>
                      </SidebarMenuButton>
                      <Show when={item.count !== undefined}>
                        <SidebarMenuBadge class="top-3 sm:top-2.5">
                          {item.count}
                        </SidebarMenuBadge>
                      </Show>
                    </SidebarMenuItem>
                  )}
                </For>
              </SidebarMenu>
            </nav>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter class="p-5">
        <div class="flex items-center gap-2 text-xs text-muted-foreground">
          <Icon name="repo" size={13} />
          <span>Your code. Your workspace.</span>
        </div>
      </SidebarFooter>
    </Sidebar>
  );
};
