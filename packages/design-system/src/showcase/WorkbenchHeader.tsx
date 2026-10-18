import { Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { SidebarTrigger } from "../components/ui/sidebar";
import { Separator } from "../components/ui/separator";
import { Icon } from "../components/workspace/icon";
import { KeyHint } from "../components/workspace/key-hint";
import { getTheme, toggleTheme } from "../tokens/theme";
export interface WorkbenchHeaderProps {
  title: string;
  repositoryName?: string;
  onBackToRepositories: () => void;
  onOpenCommandPalette: () => void;
  onOpenShortcutsModal: () => void;
}
export const WorkbenchHeader: Component<WorkbenchHeaderProps> = (props) => (
  <header class="sticky top-0 z-20 flex h-12 min-w-0 shrink-0 items-center justify-between gap-2 border-b bg-background/90 px-3 backdrop-blur-xl sm:h-14 sm:px-5">
    <div class="flex min-w-0 items-center gap-2">
      <SidebarTrigger class="size-8 shrink-0" />
      <Separator orientation="vertical" class="mr-1 h-4 shrink-0" />
      <Show when={props.repositoryName}>
        <Button
          variant="ghost"
          size="icon"
          class="size-8 shrink-0"
          aria-label="Back to repositories"
          title="Back to repositories"
          onClick={props.onBackToRepositories}
        >
          <Icon name="chevron-right" class="rotate-180" />
        </Button>
        <span
          class="hidden min-w-0 max-w-48 truncate text-xs text-muted-foreground lg:inline"
          title={props.repositoryName}
        >
          {props.repositoryName}
        </span>
        <Icon
          name="chevron-right"
          class="hidden shrink-0 text-muted-foreground lg:inline"
          size={12}
        />
      </Show>
      <span class="min-w-0 truncate whitespace-nowrap text-sm font-medium">
        {props.title}
      </span>
    </div>
    <div class="flex items-center gap-1">
      <Button
        variant="ghost"
        class="hidden h-8 gap-3 text-xs text-muted-foreground sm:inline-flex"
        onClick={props.onOpenCommandPalette}
      >
        <Icon name="search" />
        Commands
        <KeyHint keys="mod+k" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        class="sm:hidden"
        aria-label="Search commands"
        onClick={props.onOpenCommandPalette}
      >
        <Icon name="search" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        class="hidden sm:inline-flex"
        aria-label="Keyboard shortcuts"
        onClick={props.onOpenShortcutsModal}
      >
        <Icon name="keyboard" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        aria-label="Toggle theme"
        onClick={toggleTheme}
      >
        <Icon name={getTheme() === "dark" ? "sun" : "moon"} />
      </Button>
    </div>
  </header>
);
