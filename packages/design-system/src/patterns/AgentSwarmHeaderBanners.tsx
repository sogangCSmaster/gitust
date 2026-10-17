import { Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
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
  <Collapsible>
    <CollapsibleTrigger
      as={Button}
      variant="ghost"
      size="sm"
      class="-ml-2 w-full justify-start text-muted-foreground"
    >
      <Icon name="sliders" /> Repository tools
    </CollapsibleTrigger>
    <CollapsibleContent class="space-y-3 py-3">
      <p
        aria-label="Worktree summary"
        class="text-xs leading-relaxed text-muted-foreground"
      >
        {props.stats.verified} ready for review · {props.stats.compiling}{" "}
        running
        <Show when={props.stats.failed}> · {props.stats.failed} failed</Show>
      </p>
      <p class="text-xs leading-relaxed text-muted-foreground">
        Fetch upstream changes. Approved merges are published separately.
      </p>
      <Button variant="outline" size="sm" onClick={props.onTriggerGithubSync}>
        <Icon name="rotate-ccw" /> Fetch upstream
      </Button>
    </CollapsibleContent>
  </Collapsible>
);
