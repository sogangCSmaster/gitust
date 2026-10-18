import { createMemo, createSignal, Show, type Component } from "solid-js";
import { Alert, AlertDescription } from "../components/ui/alert";
import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Icon } from "../components/workspace/icon";
import { PullRequestCollaborationPanel } from "../patterns/PullRequestCollaborationPanel";
import {
  VirtualDiffViewer,
  type DiffLine,
} from "../patterns/VirtualDiffViewer";
import type { LiveCommitSummary, LiveWorktreeReview } from "./live-repo-client";
import { RepositoryFilesView } from "./RepositoryFilesView";

export const RepositoryChangesView: Component<{
  liveMode: boolean;
  repositoryId?: string;
  defaultBranch?: string;
  selectedBranch?: string;
  selectionEpoch?: number;
  commits: LiveCommitSummary[];
  activeCommitRev: string;
  onSelectCommit: (sha: string) => void;
  lookupMicros: number | null;
  review?: LiveWorktreeReview | null;
  selectedFilePath: string;
  lines: DiffLine[];
  files: string[];
  truncated: boolean;
  onSelectFile: (path: string) => void;
}> = (props) => {
  const [mode, setMode] = createSignal<"commits" | "pulls">("commits");
  const selected = createMemo(() =>
    props.commits.find(
      (commit) =>
        props.activeCommitRev === commit.sha ||
        props.activeCommitRev === commit.short_sha,
    ),
  );
  return (
    <div class="min-w-0 space-y-3">
      <Show when={props.liveMode && props.repositoryId}>
        <div class="flex flex-wrap items-center gap-1.5">
          <Button
            size="sm"
            variant={mode() === "commits" ? "secondary" : "ghost"}
            aria-pressed={mode() === "commits"}
            onClick={() => setMode("commits")}
          >
            <Icon name="diff" />
            Commits & diff
          </Button>
          <Button
            size="sm"
            variant={mode() === "pulls" ? "secondary" : "ghost"}
            aria-pressed={mode() === "pulls"}
            onClick={() => setMode("pulls")}
          >
            <Icon name="git-pull-request" />
            Pull requests
          </Button>
        </div>
      </Show>
      <Show
        when={!props.liveMode || !props.repositoryId || mode() === "commits"}
        fallback={
          <PullRequestCollaborationPanel
            repositoryId={props.repositoryId}
            defaultBranch={props.defaultBranch}
            selectedBranch={props.selectedBranch}
            selectionEpoch={props.selectionEpoch}
          />
        }
      >
      <Show when={!props.review || props.commits.length > 0}>
        <div class="flex min-w-0 flex-wrap items-center gap-2">
          <Select<LiveCommitSummary>
            class="w-full min-w-0"
            sameWidth
            options={props.commits}
            optionValue="sha"
            optionTextValue="summary"
            value={selected() ?? null}
            onChange={(commit) => {
              if (commit) props.onSelectCommit(commit.sha);
            }}
            placeholder={
              props.review ? "Choose another commit" : "Select commit"
            }
            itemComponent={(item) => (
              <SelectItem item={item.item} class="max-w-[min(32rem,85vw)]">
                <span class="flex min-w-0 items-center gap-2">
                  <code class="shrink-0 text-xs">
                    {item.item.rawValue.short_sha}
                  </code>
                  <span class="truncate" title={item.item.rawValue.summary}>
                    {item.item.rawValue.summary}
                  </span>
                </span>
              </SelectItem>
            )}
          >
            <SelectTrigger aria-label="Select commit" class="min-w-0">
              <SelectValue<LiveCommitSummary> class="min-w-0 truncate">
                {(state) => {
                  const commit = state.selectedOption();
                  return commit
                    ? `${commit.short_sha} · ${commit.summary}`
                    : "Select commit";
                }}
              </SelectValue>
            </SelectTrigger>
            <SelectContent class="max-h-80 max-w-[calc(100vw-2rem)] overflow-y-auto" />
          </Select>
        </div>
      </Show>
      <Show when={props.review}>
        {(review) => (
          <div class="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
            <Icon name="git-branch" size={12} />
            <span
              class="truncate"
              title={`${review().branch} → ${review().baseBranch || "Unknown merge target"}`}
            >
              {review().branch} →{" "}
              {review().baseBranch || "Unknown merge target"}
            </span>
            <Show when={review().headSha}>
              {(sha) => (
                <code class="ml-auto shrink-0" title={sha()}>
                  {sha().slice(0, 8)}
                </code>
              )}
            </Show>
          </div>
        )}
      </Show>
      <Show when={props.review || props.lookupMicros !== null}>
        <Collapsible class="min-w-0 rounded-md border px-3">
          <CollapsibleTrigger
            as={Button}
            variant="ghost"
            size="sm"
            class="-ml-2"
          >
            <Icon name="chevron-right" /> Comparison details
          </CollapsibleTrigger>
          <CollapsibleContent class="space-y-2 pb-3 text-xs">
            <Show when={props.review}>
              {(review) => (
                <>
                  <p class="font-medium">Committed worktree changes</p>
                  <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                    <span class="break-all">Head {review().branch}</span>
                    <code class="max-w-full break-all">
                      {review().headSha ?? "Unresolved"}
                    </code>
                  </p>
                  <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                    <span class="break-all">
                      Base {review().baseBranch || "Unknown merge target"}
                    </span>
                    <code class="max-w-full break-all">
                      {review().baseSha ?? "Unresolved"}
                    </code>
                  </p>
                  <p class="text-muted-foreground">
                    {review().source === "verified"
                      ? "Requested verified head snapshot. "
                      : "Committed branch comparison. "}
                    Comparing against the merge target at review time.
                    Uncommitted files are not included.
                  </p>
                </>
              )}
            </Show>
            <Show when={props.lookupMicros !== null}>
              <p class="text-muted-foreground">
                Lookup: {props.lookupMicros}µs
              </p>
            </Show>
          </CollapsibleContent>
        </Collapsible>
      </Show>
      <Show when={props.review?.state === "loading"}>
        <p role="status" class="text-xs text-muted-foreground">
          Resolving committed revisions...
        </p>
      </Show>
      <Show when={props.review?.error}>
        {(error) => (
          <p role="status" class="text-xs text-error-foreground">
            {error()}
          </p>
        )}
      </Show>
      <Show when={props.truncated}>
        <Alert>
          <AlertDescription>
            This diff preview is incomplete because the server limit was
            reached. Some changed files or lines may be missing.
          </AlertDescription>
        </Alert>
      </Show>
      <Show
        when={props.files.length > 1}
        fallback={
          <VirtualDiffViewer
            filePath={props.selectedFilePath}
            lines={props.lines}
            height={420}
          />
        }
      >
        <RepositoryFilesView
          files={props.files.map((path) => ({ path, kind: "file" }))}
          selectedFilePath={props.selectedFilePath}
          lines={props.lines}
          label="Changed files"
          initialDetail
          onSelect={(file) => props.onSelectFile(file.path)}
        />
      </Show>
      </Show>
    </div>
  );
};
