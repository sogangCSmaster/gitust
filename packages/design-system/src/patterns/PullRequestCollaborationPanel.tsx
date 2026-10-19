import {
  createEffect,
  createSignal,
  For,
  onCleanup,
  Show,
  type Component,
} from "solid-js";
import { Alert, AlertDescription } from "../components/ui/alert";
import { Button } from "../components/ui/button";
import { TextField, TextFieldInput } from "../components/ui/text-field";
import { Icon } from "../components/workspace/icon";
import { EmptyState, StatusBadge } from "../components/workspace/presentation";
import {
  createLivePullRequestClient,
  INITIAL_PULL_REQUEST_STATE,
  type LivePullRequestState,
} from "../showcase/live-pull-request-client";
import { PullRequestDetailPanel } from "./PullRequestDetailPanel";

export interface PullRequestCollaborationPanelProps {
  repositoryId: string | undefined;
  defaultBranch?: string;
  selectedBranch?: string;
  selectionEpoch?: number;
}

export const PullRequestCollaborationPanel: Component<
  PullRequestCollaborationPanelProps
> = (props) => {
  const [state, setState] = createSignal<LivePullRequestState>(
    INITIAL_PULL_REQUEST_STATE,
  );
  const [showCreateForm, setShowCreateForm] = createSignal(false);
  const [showProtectionForm, setShowProtectionForm] = createSignal(false);

  const [prTitle, setPrTitle] = createSignal("");
  const [prDescription, setPrDescription] = createSignal("");
  const [baseBranch, setBaseBranch] = createSignal(
    props.defaultBranch || "main",
  );
  const [headBranch, setHeadBranch] = createSignal(
    props.selectedBranch || "feat/changes",
  );
  const [isDraft, setIsDraft] = createSignal(false);

  const [branchPattern, setBranchPattern] = createSignal(
    props.defaultBranch || "main",
  );
  const [requirePr, setRequirePr] = createSignal(true);
  const [requiredApprovals, setRequiredApprovals] = createSignal("1");
  const [requirePassingChecks, setRequirePassingChecks] = createSignal(true);
  const [restrictDirectPushes, setRestrictDirectPushes] = createSignal(true);

  const client = createLivePullRequestClient(state, setState, {
    repositoryId: () => props.repositoryId,
    selectionEpoch: () => props.selectionEpoch ?? 0,
  });

  createEffect(() => {
    const repoId = props.repositoryId;
    const epoch = props.selectionEpoch ?? 0;
    void epoch;
    if (repoId) {
      void client.loadAll();
    } else {
      client.clear();
    }
  });

  onCleanup(() => {
    client.clear();
  });

  const handleCreatePull = async (e: Event) => {
    e.preventDefault();
    const title = prTitle().trim();
    const base = baseBranch().trim();
    const head = headBranch().trim();
    if (!title || !base || !head) return;
    const created = await client.createPull({
      title,
      description: prDescription().trim(),
      base_branch: base,
      head_branch: head,
      draft: isDraft(),
    });
    if (created) {
      setPrTitle("");
      setPrDescription("");
      setShowCreateForm(false);
    }
  };

  const handleSaveProtection = (e: Event) => {
    e.preventDefault();
    const pattern = branchPattern().trim();
    if (!pattern) return;
    const parsedApprovals = Number.parseInt(requiredApprovals().trim(), 10);
    void client.saveBranchProtection({
      branch_pattern: pattern,
      require_pull_request: requirePr(),
      required_approvals:
        Number.isFinite(parsedApprovals) && parsedApprovals >= 0
          ? parsedApprovals
          : 0,
      require_passing_checks: requirePassingChecks(),
      restrict_direct_pushes: restrictDirectPushes(),
    });
  };

  return (
    <section
      class="min-w-0 space-y-4"
      aria-label="Pull request collaboration and branch protection"
    >
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2 text-xs text-muted-foreground">
          <Icon name="git-pull-request" size={14} class="text-primary" />
          <span>
            {state().pulls.length} pull request(s) · {state().rules.length}{" "}
            protection rule(s)
          </span>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant={showCreateForm() ? "secondary" : "default"}
            onClick={() => setShowCreateForm((v) => !v)}
          >
            <Icon name="plus" />
            New pull request
          </Button>
          <Button
            size="sm"
            variant={showProtectionForm() ? "secondary" : "outline"}
            onClick={() => setShowProtectionForm((v) => !v)}
          >
            <Icon name="shield-check" />
            Branch protection
          </Button>
          <Button
            size="sm"
            variant="ghost"
            disabled={state().loadingList}
            onClick={() => void client.loadAll()}
          >
            <Icon name="rotate-ccw" />
            Refresh
          </Button>
        </div>
      </div>

      <Show when={state().error}>
        {(err) => (
          <Alert variant="destructive">
            <AlertDescription>{err()}</AlertDescription>
          </Alert>
        )}
      </Show>

      <Show when={state().feedback}>
        {(msg) => (
          <p role="status" class="text-xs text-muted-foreground">
            {msg()}
          </p>
        )}
      </Show>

      {/* New Pull Request Form */}
      <Show when={showCreateForm()}>
        <form
          class="space-y-3 rounded-lg border bg-card p-4"
          onSubmit={handleCreatePull}
        >
          <h3 class="text-sm font-semibold">Open a pull request</h3>
          <div class="grid gap-2 sm:grid-cols-2">
            <TextField>
              <TextFieldInput
                aria-label="Base branch"
                placeholder="Base branch (e.g. main)"
                value={baseBranch()}
                onInput={(e) => setBaseBranch(e.currentTarget.value)}
              />
            </TextField>
            <TextField>
              <TextFieldInput
                aria-label="Head branch"
                placeholder="Head branch (e.g. feat/branch)"
                value={headBranch()}
                onInput={(e) => setHeadBranch(e.currentTarget.value)}
              />
            </TextField>
          </div>
          <TextField>
            <TextFieldInput
              aria-label="Pull request title"
              placeholder="Pull request title"
              value={prTitle()}
              onInput={(e) => setPrTitle(e.currentTarget.value)}
            />
          </TextField>
          <TextField>
            <TextFieldInput
              aria-label="Pull request description"
              placeholder="Describe the changes and verification steps..."
              value={prDescription()}
              onInput={(e) => setPrDescription(e.currentTarget.value)}
            />
          </TextField>
          <div class="flex flex-wrap items-center justify-between gap-2">
            <label class="flex items-center gap-2 text-xs text-muted-foreground">
              <input
                type="checkbox"
                aria-label="Create as draft"
                checked={isDraft()}
                onChange={(e) => setIsDraft(e.currentTarget.checked)}
              />
              Create as draft
            </label>
            <Button
              type="submit"
              size="sm"
              disabled={state().submitting || !prTitle().trim()}
            >
              <Icon name="git-pull-request" />
              Create pull request
            </Button>
          </div>
        </form>
      </Show>

      {/* Branch Protection Rules Configuration */}
      <Show when={showProtectionForm()}>
        <form
          class="space-y-3 rounded-lg border bg-card p-4"
          onSubmit={handleSaveProtection}
        >
          <h3 class="text-sm font-semibold">Branch protection rules</h3>
          <div class="grid gap-2 sm:grid-cols-2">
            <TextField>
              <TextFieldInput
                aria-label="Protected branch pattern"
                placeholder="Branch pattern (e.g. main or release/*)"
                value={branchPattern()}
                onInput={(e) => setBranchPattern(e.currentTarget.value)}
              />
            </TextField>
            <TextField>
              <TextFieldInput
                aria-label="Required approvals"
                placeholder="Required approvals (e.g. 1)"
                value={requiredApprovals()}
                onInput={(e) => setRequiredApprovals(e.currentTarget.value)}
              />
            </TextField>
          </div>
          <div class="flex flex-wrap items-center gap-4 text-xs">
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Require pull request"
                checked={requirePr()}
                onChange={(e) => setRequirePr(e.currentTarget.checked)}
              />
              Require pull request
            </label>
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Require passing checks"
                checked={requirePassingChecks()}
                onChange={(e) =>
                  setRequirePassingChecks(e.currentTarget.checked)
                }
              />
              Require passing checks
            </label>
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Restrict direct pushes"
                checked={restrictDirectPushes()}
                onChange={(e) =>
                  setRestrictDirectPushes(e.currentTarget.checked)
                }
              />
              Restrict direct pushes
            </label>
          </div>
          <div class="flex justify-end">
            <Button type="submit" size="sm" disabled={state().submitting}>
              <Icon name="shield-check" />
              Save branch protection
            </Button>
          </div>
          <Show when={state().rules.length > 0}>
            <ul class="space-y-1.5 border-t pt-3 text-xs">
              <For each={state().rules}>
                {(rule) => (
                  <li class="flex flex-wrap items-center justify-between gap-2 rounded border px-2.5 py-1.5">
                    <code class="font-medium">{rule.branch_pattern}</code>
                    <span class="text-muted-foreground">
                      {rule.required_approvals} approval(s) ·{" "}
                      {rule.require_passing_checks
                        ? "Checks required"
                        : "Checks optional"}{" "}
                      ·{" "}
                      {rule.restrict_direct_pushes
                        ? "Direct push blocked"
                        : "Direct push allowed"}
                    </span>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </form>
      </Show>

      <Show
        when={state().pulls.length > 0}
        fallback={
          <EmptyState
            title="No pull requests"
            description="Create a pull request to review branch comparisons, leave inline comments, and verify branch protection gates."
          />
        }
      >
        <div class="grid min-w-0 gap-4 lg:grid-cols-[16rem_minmax(0,1fr)]">
          <ul
            class="max-h-96 space-y-1.5 overflow-y-auto"
            aria-label="Pull requests"
          >
            <For each={state().pulls}>
              {(pr) => (
                <li>
                  <Button
                    variant={
                      state().selectedPullId === pr.id ? "secondary" : "ghost"
                    }
                    class="h-auto w-full flex-col items-start gap-1 px-3 py-2.5 text-left"
                    aria-pressed={state().selectedPullId === pr.id}
                    onClick={() => void client.selectPull(pr.id)}
                  >
                    <div class="flex w-full items-center justify-between gap-2 text-xs">
                      <span class="font-mono text-muted-foreground">
                        #{pr.number}
                      </span>
                      <StatusBadge
                        tone={
                          pr.status === "merged"
                            ? "info"
                            : pr.status === "open"
                              ? "success"
                              : pr.status === "closed"
                                ? "danger"
                                : "neutral"
                        }
                      >
                        {pr.status}
                      </StatusBadge>
                    </div>
                    <span class="w-full truncate text-xs font-medium">
                      {pr.title}
                    </span>
                    <span class="w-full truncate font-mono text-[11px] text-muted-foreground">
                      {pr.head_branch} → {pr.base_branch}
                    </span>
                  </Button>
                </li>
              )}
            </For>
          </ul>

          <div class="min-w-0">
            <Show
              when={state().detail}
              keyed
              fallback={
                <p role="status" class="text-xs text-muted-foreground">
                  {state().loadingDetail
                    ? "Loading pull request details..."
                    : "Select a pull request to inspect reviews and merge status."}
                </p>
              }
            >
              {(detail) => (
                <PullRequestDetailPanel
                  detail={detail}
                  submitting={state().submitting}
                  onSubmitReview={(input) =>
                    void client.submitReview(detail.pull.id, input)
                  }
                  onAddComment={(input) =>
                    void client.addComment(detail.pull.id, input)
                  }
                  onUpdateStatus={(status) =>
                    void client.updateStatus(detail.pull.id, status)
                  }
                  onMerge={(input) =>
                    void client.mergePull(detail.pull.id, input)
                  }
                />
              )}
            </Show>
          </div>
        </div>
      </Show>
    </section>
  );
};
