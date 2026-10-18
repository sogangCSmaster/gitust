import {
  createMemo,
  createSignal,
  For,
  lazy,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import { DeferredView } from "../components/workspace/deferred-view";
import { Icon, type IconName } from "../components/workspace/icon";
import type { ConnectedRepository } from "./connected-repository-client";
import type { RepositoryRenameActions } from "./repository-rename-types";
import { RepositoryInitialCommit } from "./RepositoryInitialCommit";
import { RepositoryClone, type RepositoryCloneTarget } from "./RepositoryClone";
import { RepositoryRename } from "./RepositoryRename";
import { RepositoryDefaultBranch } from "./RepositoryDefaultBranch";
import type { DefaultBranchActions } from "./repository-default-branch";
import { RepositoryVisibility } from "./RepositoryVisibility";
import type { VisibilityActions } from "./repository-visibility";
import {
  attachRepositoryOverviewActions,
  type RepositoryWorkspaceCore,
} from "./repository-workspace";

const RepositoryAdministrationPanel = lazy(() =>
  import("../patterns/RepositoryAdministrationPanel").then((module) => ({
    default: module.RepositoryAdministrationPanel,
  })),
);

export const RepositoryOverview: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  worktrees: number;
  onNavigate: (tab: "swarm" | "tree" | "diff" | "actions") => void;
  actions?:
    | {
        rename: RepositoryRenameActions;
        refreshViews: () => unknown;
        clone: () => RepositoryCloneTarget | null;
        defaultBranch?: DefaultBranchActions;
        visibility?: VisibilityActions;
      }
    | RepositoryWorkspaceCore;
}> = (props) => {
  const [showAdmin, setShowAdmin] = createSignal(false);
  const overviewActions = createMemo(() =>
    props.actions
      ? "rename" in props.actions
        ? props.actions
        : attachRepositoryOverviewActions(props.actions)
      : undefined,
  );
  const canAdminister = () =>
    props.liveMode &&
    Boolean(
      props.repository.canAdminister ||
        props.repository.canRename ||
        props.repository.canChangeVisibility,
    );
  const destinations: Array<{
    id: "swarm" | "tree" | "diff" | "actions";
    label: string;
    detail: string;
    icon: IconName;
  }> = [
    {
      id: "swarm",
      label: "Worktrees",
      detail: "Review, verify, and merge agent changes.",
      icon: "git-branch",
    },
    {
      id: "tree",
      label: "Files",
      detail: "Browse the repository source.",
      icon: "folder",
    },
    {
      id: "diff",
      label: "Changes",
      detail: "Inspect commits and file diffs.",
      icon: "diff",
    },
    ...(props.liveMode
      ? [
          {
            id: "actions" as const,
            label: "Actions",
            detail: "Inspect retained automatic Checks attempts.",
            icon: "play" as const,
          },
        ]
      : []),
  ];
  return (
    <section
      class="mx-auto w-full max-w-4xl space-y-6"
      aria-labelledby="repository-title"
    >
      <div class="min-w-0 space-y-2">
        <div class="flex min-w-0 flex-wrap items-center gap-2">
          <Icon name="repo" size={20} class="shrink-0 text-primary" />
          <h1
            id="repository-title"
            title={props.repository.name}
            class="truncate text-xl font-semibold tracking-tight"
          >
            {props.repository.name}
          </h1>
          <RepositoryRename
            repository={props.repository}
            liveMode={props.liveMode}
            actions={overviewActions()?.rename}
          />
          <RepositoryClone
            repository={props.repository}
            liveMode={props.liveMode}
            begin={overviewActions()?.clone}
          />
          <Show when={!props.liveMode}>
            <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
              Demo
            </span>
          </Show>
          <Show
            when={
              props.liveMode &&
              props.repository.capability === "browse" &&
              !props.repository.canSpawnWorktree
            }
          >
            <span class="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
              {props.repository.canPush ? "Git hosting" : "Read only"}
            </span>
          </Show>
          <Show when={props.liveMode && props.repository.canSpawnWorktree}>
            <span class="shrink-0 rounded bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-primary">
              Workbench Enabled
            </span>
          </Show>
          <Show when={props.liveMode && props.repository.archived}>
            <span class="shrink-0 rounded bg-warning/20 px-1.5 py-0.5 text-[10px] font-medium text-warning-foreground">
              Archived
            </span>
          </Show>
          <Show when={canAdminister()}>
            <Button
              size="sm"
              variant={showAdmin() ? "secondary" : "outline"}
              class="ml-auto h-7 px-2 text-xs"
              aria-pressed={showAdmin()}
              onClick={() => setShowAdmin((v) => !v)}
            >
              <Icon name="sliders" size={12} />
              Admin & lifecycle
            </Button>
          </Show>
        </div>
        <Show when={props.repository.description}>
          {(desc) => <p class="text-xs text-muted-foreground">{desc()}</p>}
        </Show>
        <div class="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
          <RepositoryDefaultBranch
            repository={props.repository}
            liveMode={props.liveMode}
            actions={overviewActions()?.defaultBranch}
          />
          <Show when={props.repository.headCommit}>
            {(sha) => (
              <span class="inline-flex items-center gap-1.5">
                <Icon name="git-commit" size={12} />
                <code title={sha()}>{sha().slice(0, 7)}</code>
              </span>
            )}
          </Show>
          <RepositoryVisibility
            repository={props.repository}
            liveMode={props.liveMode}
            actions={overviewActions()?.visibility}
          />
        </div>
      </div>
      <Show when={canAdminister() && showAdmin()}>
        <DeferredView label="Repository administration">
          <RepositoryAdministrationPanel
            repositoryId={props.repository.id}
            repositoryName={props.repository.name}
            onCatalogChanged={() => void overviewActions()?.refreshViews()}
          />
        </DeferredView>
      </Show>
      <Show when={props.repository.headCommit === null}>
        <RepositoryInitialCommit
          branch={props.repository.branch}
          onRefresh={() => overviewActions()?.refreshViews()}
        />
      </Show>
      <div class="overflow-hidden rounded-xl border bg-card">
        <For
          each={destinations.filter(
            (destination) =>
              props.repository.capability === "workbench" ||
              props.repository.canSpawnWorktree === true ||
              ["tree", "diff"].includes(destination.id) ||
              (destination.id === "actions" &&
                Boolean(
                  props.repository.canViewLocalChecksAutomatic ||
                    props.repository.canRunLocalChecks,
                )),
          )}
        >
          {(destination) => (
            <Button
              variant="ghost"
              class="h-auto w-full justify-start gap-3 rounded-none border-b px-4 py-4 text-left last:border-b-0 sm:px-5"
              onClick={() => props.onNavigate(destination.id)}
            >
              <Icon
                name={destination.icon}
                size={18}
                class="shrink-0 text-primary"
              />
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-2 text-sm font-medium">
                  {destination.label}
                  <Show when={destination.id === "swarm"}>
                    <span class="text-xs font-normal text-muted-foreground">
                      {props.worktrees}
                    </span>
                  </Show>
                </span>
                <span class="mt-1 block whitespace-normal text-xs font-normal text-muted-foreground">
                  {destination.detail}
                </span>
              </span>
              <Icon
                name="chevron-right"
                class="shrink-0 text-muted-foreground"
              />
            </Button>
          )}
        </For>
      </div>
    </section>
  );
};
