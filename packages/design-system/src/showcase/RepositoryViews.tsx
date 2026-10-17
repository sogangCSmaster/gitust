import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { TextField, TextFieldInput } from "../components/ui/text-field";
import { Icon, type IconName } from "../components/workspace/icon";
import type {
  ConnectedRepository,
  ConnectedRepositoryState,
} from "./connected-repository-client";

export const DEMO_REPOSITORY: ConnectedRepository = {
  id: "demo",
  name: "gitust",
  branch: "main",
  headCommit: "554fb6c",
  capability: "workbench",
  isDefault: true,
};

export const RepositoryLanding: Component<{
  state: ConnectedRepositoryState;
  liveMode: boolean;
  onOpen: (repository: ConnectedRepository) => void;
  onRefresh: () => void;
  onSignIn: () => void;
}> = (props) => {
  const [query, setQuery] = createSignal("");
  const repositories = createMemo(() =>
    props.liveMode ? props.state.repositories : [DEMO_REPOSITORY],
  );
  const match = createMemo(() => {
    return repositories().filter((repo) =>
      repo.name.toLowerCase().includes(query().trim().toLowerCase()),
    );
  });
  return (
    <section
      class="mx-auto w-full max-w-4xl space-y-5"
      aria-labelledby="repositories-title"
    >
      <div class="flex min-w-0 items-center justify-between gap-3">
        <div class="min-w-0">
          <h1
            id="repositories-title"
            class="text-xl font-semibold tracking-tight"
          >
            Repositories
          </h1>
          <p class="mt-1 text-xs text-muted-foreground">
            {props.liveMode
              ? "Choose a repository."
              : "Explore a sample repository, or sign in to your server."}
          </p>
        </div>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Refresh repositories"
          onClick={props.onRefresh}
          disabled={props.state.loading || !props.liveMode}
        >
          <Icon name="rotate-ccw" />
        </Button>
      </div>
      <TextField value={query()} onChange={setQuery}>
        <div class="relative">
          <Icon
            name="search"
            class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground"
          />
          <TextFieldInput
            class="pl-9"
            aria-label="Find a repository"
            placeholder="Find a repository"
          />
        </div>
      </TextField>
      <Show when={props.state.error && props.liveMode}>
        <Card>
          <CardContent class="space-y-3 pt-4">
            <p role="alert" class="text-sm text-error-foreground">
              {props.state.error}
            </p>
            <Button variant="outline" size="sm" onClick={props.onRefresh}>
              Try again
            </Button>
          </CardContent>
        </Card>
      </Show>
      <Show
        when={match().length > 0}
        fallback={
          <Show when={!props.state.error}>
            <p
              role="status"
              class="py-8 text-center text-sm text-muted-foreground"
            >
              {props.state.loading
                ? "Loading repositories…"
                : query()
                  ? "No matching repositories."
                  : "No repository is available."}
            </p>
          </Show>
        }
      >
        <div class="overflow-hidden rounded-xl border bg-card">
          <For each={match()}>
            {(repo) => (
              <Button
                variant="ghost"
                class="h-auto w-full justify-start gap-3 rounded-none border-b px-3 py-3 text-left last:border-b-0 hover:bg-accent/60 sm:px-4"
                aria-label={`Open repository ${repo.name}`}
                onClick={() => props.onOpen(repo)}
              >
                <span class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
                  <Icon name="repo" size={20} />
                </span>
                <span class="min-w-0 flex-1">
                  <span class="block truncate text-sm font-semibold">
                    {repo.name}
                  </span>
                  <span class="mt-1 flex min-w-0 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <Icon name="git-branch" size={12} />
                    <span class="truncate" title={repo.branch}>
                      {repo.branch}
                    </span>
                    <span class="shrink-0">
                      {props.liveMode
                        ? repo.capability === "browse"
                          ? "Browse only"
                          : "Workbench"
                        : "Demo"}
                    </span>
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
      </Show>
      <Show when={!props.liveMode}>
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
          <span>Sample data. Changes stay in this browser.</span>
          <Button
            size="sm"
            variant="link"
            class="h-auto p-0"
            onClick={props.onSignIn}
          >
            Sign in to your server
          </Button>
        </div>
      </Show>
      <Show when={props.liveMode}>
        <p class="text-xs text-muted-foreground">
          Browse source in every listed repository. Agent work stays in the
          workbench.
        </p>
      </Show>
    </section>
  );
};

export const RepositoryOverview: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  worktrees: number;
  onNavigate: (tab: "swarm" | "tree" | "diff" | "actions") => void;
}> = (props) => {
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
        <div class="flex min-w-0 items-center gap-2">
          <Icon name="repo" size={20} class="shrink-0 text-primary" />
          <h1
            id="repository-title"
            title={props.repository.name}
            class="truncate text-xl font-semibold tracking-tight"
          >
            {props.repository.name}
          </h1>
          <Show when={!props.liveMode}>
            <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
              Demo
            </span>
          </Show>
          <Show
            when={props.liveMode && props.repository.capability === "browse"}
          >
            <span class="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
              Browse only
            </span>
          </Show>
        </div>
        <div class="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
          <span class="inline-flex min-w-0 max-w-full items-center gap-1.5">
            <Icon name="git-branch" size={12} class="shrink-0" />
            <span class="truncate" title={props.repository.branch}>
              {props.repository.branch}
            </span>
          </span>
          <span class="inline-flex items-center gap-1.5">
            <Icon name="git-commit" size={12} />
            <code title={props.repository.headCommit}>
              {props.repository.headCommit.slice(0, 7)}
            </code>
          </span>
        </div>
      </div>
      <div class="overflow-hidden rounded-xl border bg-card">
        <For
          each={destinations.filter(
            (destination) =>
              props.repository.capability === "workbench" ||
              ["tree", "diff"].includes(destination.id),
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
