import { Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import { Card, CardContent } from "../components/ui/card";
import { Icon } from "../components/workspace/icon";
export interface AgentSwarmStats {
  verified: number;
  compiling: number;
  merged: number;
  failed: number;
  total: number;
}
export interface AgentSwarmHeaderBannersProps {
  stats: AgentSwarmStats;
  onTriggerGithubSync: () => void;
}
export const AgentSwarmHeaderBanners: Component<
  AgentSwarmHeaderBannersProps
> = (props) => (
  <div class="space-y-3">
    <div
      aria-label="Worktree summary"
      class="flex flex-wrap gap-x-5 gap-y-2 text-xs text-muted-foreground"
    >
      <span>
        <strong class="font-medium text-foreground">{props.stats.total}</strong>{" "}
        worktrees
      </span>
      <span class="text-success-foreground">
        {props.stats.verified} ready for review
      </span>
      <span>{props.stats.compiling} running</span>
      <span>{props.stats.merged} merged</span>
      <Show when={props.stats.failed}>
        <span class="text-error-foreground">{props.stats.failed} failed</span>
      </Show>
    </div>
    <Collapsible>
      <CollapsibleTrigger
        as={Button}
        variant="ghost"
        size="sm"
        class="-ml-3 text-muted-foreground"
      >
        <Icon name="sliders" />
        Repository tools
      </CollapsibleTrigger>
      <CollapsibleContent class="pt-3">
        <Card>
          <CardContent class="flex flex-wrap items-center justify-between gap-4 pt-6">
            <p class="max-w-lg text-sm text-muted-foreground">
              Fetch changes from the configured upstream. Publishing an approved
              merge is a separate action.
            </p>
            <Button variant="outline" onClick={props.onTriggerGithubSync}>
              <Icon name="rotate-ccw" />
              Fetch upstream
            </Button>
          </CardContent>
        </Card>
      </CollapsibleContent>
    </Collapsible>
  </div>
);
