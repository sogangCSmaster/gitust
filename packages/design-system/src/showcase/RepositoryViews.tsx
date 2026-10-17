import {
  createMemo,
  createSignal,
  For,
  Show,
  lazy,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { TextField, TextFieldInput } from "../components/ui/text-field";
import { Icon } from "../components/workspace/icon";
import { DeferredView } from "../components/workspace/deferred-view";
import type {
  ConnectedRepository,
  ConnectedRepositoryState,
} from "./connected-repository-client";
import { DEMO_REPOSITORY } from "./repository-demo";
export { DEMO_REPOSITORY } from "./repository-demo";
const Overview = lazy(() =>
  import("./RepositoryOverview").then((module) => ({
    default: module.RepositoryOverview,
  })),
);
export const RepositoryOverview: typeof import("./RepositoryOverview").RepositoryOverview =
  (props) => (
    <DeferredView label="Repository overview">
      <Overview {...props} />
    </DeferredView>
  );

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
                  <span class="flex min-w-0 items-center gap-2">
                    <span class="truncate text-sm font-semibold">
                      {repo.name}
                    </span>
                    <Show when={props.liveMode && repo.headCommit === null}>
                      <span class="shrink-0 text-[10px] font-normal text-muted-foreground">
                        No commit
                      </span>
                    </Show>
                  </span>
                  <span class="mt-1 flex min-w-0 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <Icon name="git-branch" size={12} />
                    <span class="truncate" title={repo.branch}>
                      {repo.branch}
                    </span>
                    <span class="shrink-0">
                      {props.liveMode
                        ? repo.capability === "browse"
                          ? repo.canPush
                            ? "Git hosting"
                            : "Read only"
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
