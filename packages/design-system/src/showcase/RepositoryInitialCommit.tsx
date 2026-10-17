import type { Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Icon } from "../components/workspace/icon";

export const RepositoryInitialCommit: Component<{
  branch: string;
  onRefresh: () => unknown;
}> = (props) => (
  <section
    class="min-w-0 rounded-xl border bg-card p-4 sm:p-5"
    aria-label="Default branch without a commit"
  >
    <h2 class="text-sm font-medium">No commits on the default branch</h2>
    <p class="mt-2 truncate text-xs text-muted-foreground" title={props.branch}>
      <Icon name="git-branch" size={12} class="mr-1 inline-block" />{" "}
      {props.branch}
    </p>
    <p class="mt-2 text-xs text-muted-foreground">
      Add a first commit to this branch, then refresh. Other branches may
      already contain commits.
    </p>
    <Button
      class="mt-3"
      variant="outline"
      size="sm"
      onClick={() => void props.onRefresh()}
    >
      <Icon name="rotate-ccw" /> Refresh repository
    </Button>
  </section>
);
