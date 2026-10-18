import {
  createEffect,
  createSignal,
  lazy,
  onCleanup,
  Show,
  Suspense,
  type Component,
} from "solid-js";
import { Alert, AlertDescription } from "../components/ui/alert";
import { Button } from "../components/ui/button";
import { TextField, TextFieldInput } from "../components/ui/text-field";
import { Icon } from "../components/workspace/icon";
import { StatusBadge } from "../components/workspace/presentation";
import {
  createHostedRepository,
  fetchRepositorySettings,
  updateRepositorySettings,
  type LiveRepositorySettings,
} from "../showcase/live-repo-admin-client";
import { formatErr } from "../showcase/live-repo-context";

const SyncTopologyAndRollbackPanel = lazy(() =>
  import("./SyncTopologyAndRollbackPanel").then((m) => ({
    default: m.SyncTopologyAndRollbackPanel,
  })),
);

export interface RepositoryAdministrationPanelProps {
  repositoryId: string;
  repositoryName: string;
  onCatalogChanged?: () => void;
}

export const RepositoryAdministrationPanel: Component<
  RepositoryAdministrationPanelProps
> = (props) => {
  const [settings, setSettings] = createSignal<LiveRepositorySettings | null>(
    null,
  );
  const [loading, setLoading] = createSignal(false);
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const [description, setDescription] = createSignal("");
  const [defaultBranch, setDefaultBranch] = createSignal("main");
  const [isPrivate, setIsPrivate] = createSignal(true);
  const [workbenchEnabled, setWorkbenchEnabled] = createSignal(false);
  const [archived, setArchived] = createSignal(false);

  const [newRepoName, setNewRepoName] = createSignal("");
  const [ownerTeamId, setOwnerTeamId] = createSignal("");
  const [newRepoBranch, setNewRepoBranch] = createSignal("main");
  const [newRepoDescription, setNewRepoDescription] = createSignal("");
  const [newRepoPrivate, setNewRepoPrivate] = createSignal(true);
  const [newRepoWorkbench, setNewRepoWorkbench] = createSignal(false);

  let loadGen = 0;
  let loadAbort: AbortController | undefined;

  const applySettings = (next: LiveRepositorySettings) => {
    setSettings(next);
    setDescription(next.description ?? "");
    setDefaultBranch(next.default_branch);
    setIsPrivate(next.is_private);
    setWorkbenchEnabled(next.workbench_enabled);
    setArchived(next.archived);
  };

  const loadSettings = async (repoId: string) => {
    const reqId = ++loadGen;
    loadAbort?.abort();
    const controller = new AbortController();
    loadAbort = controller;
    setLoading(true);
    setError(null);
    try {
      const loaded = await fetchRepositorySettings(repoId, controller.signal);
      if (reqId !== loadGen) return;
      applySettings(loaded);
      setLoading(false);
    } catch (err) {
      if (reqId !== loadGen) return;
      setLoading(false);
      setError(formatErr(err));
    }
  };

  createEffect(() => {
    const id = props.repositoryId;
    if (id) {
      void loadSettings(id);
    }
  });

  onCleanup(() => {
    loadGen++;
    loadAbort?.abort();
  });

  const handleSaveSettings = async (e: Event) => {
    e.preventDefault();
    if (submitting()) return;
    setSubmitting(true);
    setError(null);
    setFeedback(null);
    try {
      const updated = await updateRepositorySettings(props.repositoryId, {
        description: description().trim() || null,
        default_branch: defaultBranch().trim() || "main",
        is_private: isPrivate(),
        workbench_enabled: workbenchEnabled(),
        archived: archived(),
      });
      applySettings(updated);
      setFeedback(`Updated settings for ${updated.name}`);
      props.onCatalogChanged?.();
    } catch (err) {
      setError(formatErr(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleProvisionRepository = async (e: Event) => {
    e.preventDefault();
    const name = newRepoName().trim();
    const teamId = ownerTeamId().trim();
    if (!name || !teamId || submitting()) return;
    setSubmitting(true);
    setError(null);
    setFeedback(null);
    try {
      const created = await createHostedRepository({
        name,
        owner_team_id: teamId,
        is_private: newRepoPrivate(),
        default_branch: newRepoBranch().trim() || "main",
        description: newRepoDescription().trim(),
        workbench_enabled: newRepoWorkbench(),
      });
      setNewRepoName("");
      setNewRepoDescription("");
      setFeedback(
        `Provisioned online repository ${created.name} (${created.id.slice(0, 8)})`,
      );
      props.onCatalogChanged?.();
    } catch (err) {
      setError(formatErr(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section
      class="space-y-4 rounded-xl border bg-card p-4"
      aria-label="Repository administration and lifecycle"
    >
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="space-y-0.5">
          <h2 class="flex items-center gap-2 text-sm font-semibold">
            <Icon name="sliders" size={14} class="text-primary" />
            Repository administration & lifecycle
          </h2>
          <p class="text-xs text-muted-foreground">
            Manage repository lifecycle, multi-repository worktree routing, and
            online repository provisioning.
          </p>
        </div>
        <Show when={settings()}>
          {(current) => (
            <div class="flex flex-wrap items-center gap-1.5">
              <StatusBadge tone={current().archived ? "warning" : "success"}>
                {current().archived ? "Archived" : "Active"}
              </StatusBadge>
              <StatusBadge
                tone={current().workbench_enabled ? "info" : "neutral"}
              >
                {current().workbench_enabled
                  ? "Workbench Enabled"
                  : "Browse / Hosting"}
              </StatusBadge>
            </div>
          )}
        </Show>
      </div>

      <Show when={loading()}>
        <p role="status" class="text-xs text-muted-foreground">
          Loading repository settings...
        </p>
      </Show>

      <Show when={error()}>
        {(err) => (
          <Alert variant="destructive">
            <AlertDescription>{err()}</AlertDescription>
          </Alert>
        )}
      </Show>

      <Show when={feedback()}>
        {(msg) => (
          <p role="status" class="text-xs text-muted-foreground">
            {msg()}
          </p>
        )}
      </Show>

      <Show when={settings()}>
        <form class="space-y-3 border-t pt-3" onSubmit={handleSaveSettings}>
          <h3 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Settings for {props.repositoryName}
          </h3>
          <div class="grid gap-2 sm:grid-cols-2">
            <TextField>
              <TextFieldInput
                aria-label="Repository description"
                placeholder="Repository description"
                value={description()}
                onInput={(e) => setDescription(e.currentTarget.value)}
              />
            </TextField>
            <TextField>
              <TextFieldInput
                aria-label="Default branch setting"
                placeholder="Default branch (e.g. main)"
                value={defaultBranch()}
                onInput={(e) => setDefaultBranch(e.currentTarget.value)}
              />
            </TextField>
          </div>
          <div class="flex flex-wrap items-center gap-4 text-xs">
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Private repository visibility"
                checked={isPrivate()}
                onChange={(e) => setIsPrivate(e.currentTarget.checked)}
              />
              Private visibility
            </label>
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Enable worktree execution"
                checked={workbenchEnabled()}
                onChange={(e) => setWorkbenchEnabled(e.currentTarget.checked)}
              />
              Enable worktree & agent routing
            </label>
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Archive repository (read-only)"
                checked={archived()}
                onChange={(e) => setArchived(e.currentTarget.checked)}
              />
              Archive repository (read-only)
            </label>
          </div>
          <div class="flex justify-end">
            <Button type="submit" size="sm" disabled={submitting()}>
              <Icon name="check" />
              Save repository settings
            </Button>
          </div>
        </form>
      </Show>

      <form
        class="space-y-3 border-t pt-3"
        onSubmit={handleProvisionRepository}
      >
        <h3 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Provision online hosted repository
        </h3>
        <div class="grid gap-2 sm:grid-cols-3">
          <TextField>
            <TextFieldInput
              aria-label="New repository name"
              placeholder="Repository name (e.g. service-api)"
              value={newRepoName()}
              onInput={(e) => setNewRepoName(e.currentTarget.value)}
            />
          </TextField>
          <TextField>
            <TextFieldInput
              aria-label="Owner team UUID"
              placeholder="Owner team UUID"
              value={ownerTeamId()}
              onInput={(e) => setOwnerTeamId(e.currentTarget.value)}
            />
          </TextField>
          <TextField>
            <TextFieldInput
              aria-label="New repository default branch"
              placeholder="Default branch (main)"
              value={newRepoBranch()}
              onInput={(e) => setNewRepoBranch(e.currentTarget.value)}
            />
          </TextField>
        </div>
        <TextField>
          <TextFieldInput
            aria-label="New repository description"
            placeholder="Optional description for the new repository"
            value={newRepoDescription()}
            onInput={(e) => setNewRepoDescription(e.currentTarget.value)}
          />
        </TextField>
        <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
          <div class="flex flex-wrap items-center gap-4">
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Private new repository"
                checked={newRepoPrivate()}
                onChange={(e) => setNewRepoPrivate(e.currentTarget.checked)}
              />
              Private repository
            </label>
            <label class="flex items-center gap-1.5">
              <input
                type="checkbox"
                aria-label="Enable worktree execution on new repository"
                checked={newRepoWorkbench()}
                onChange={(e) => setNewRepoWorkbench(e.currentTarget.checked)}
              />
              Enable worktree execution
            </label>
          </div>
          <Button
            type="submit"
            size="sm"
            variant="outline"
            disabled={
              submitting() || !newRepoName().trim() || !ownerTeamId().trim()
            }
          >
            <Icon name="plus" />
            Provision repository
          </Button>
        </div>
      </form>

      <Suspense>
        <SyncTopologyAndRollbackPanel
          repositoryId={props.repositoryId}
          onRolledBack={() => props.onCatalogChanged?.()}
        />
      </Suspense>
    </section>
  );
};
