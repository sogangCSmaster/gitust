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
import { StatusBadge } from "../components/workspace/presentation";
import { RunnerFleetAndCachePanel } from "../patterns/RunnerFleetAndCachePanel";
import { formatErr } from "./live-repo-context";
import {
  deleteWorkflowSecret,
  fetchRepositoryWorkflows,
  fetchWorkflowArtifacts,
  fetchWorkflowSecrets,
  putWorkflowSecret,
  triggerWorkflowRun,
  type LiveRepositoryWorkflowsOverview,
  type LiveWorkflowArtifact,
  type LiveWorkflowSecretItem,
} from "./live-workflows-client";

export interface LiveWorkflowsViewProps {
  repositoryId?: string;
  role?: string;
  defaultRef?: string;
  defaultCommit?: string | null;
}

const formatMatrix = (matrix?: Record<string, string[]> | null) =>
  matrix
    ? Object.entries(matrix)
        .map(([k, vals]) => `${k}: ${vals.join(", ")}`)
        .join(" · ")
    : null;

export const LiveWorkflowsView: Component<LiveWorkflowsViewProps> = (props) => {
  const [overview, setOverview] = createSignal<LiveRepositoryWorkflowsOverview | null>(null);
  const [secrets, setSecrets] = createSignal<LiveWorkflowSecretItem[]>([]);
  const [selectedWorkflow, setSelectedWorkflow] = createSignal("");
  const [inspectedArtifacts, setInspectedArtifacts] = createSignal<{
    runId: string;
    items: LiveWorkflowArtifact[];
  } | null>(null);
  const [secretName, setSecretName] = createSignal("");
  const [secretValue, setSecretValue] = createSignal("");
  const [loading, setLoading] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let loadGen = 0;
  let loadAbort: AbortController | undefined;

  const loadAll = async (repoId: string) => {
    const reqId = ++loadGen;
    loadAbort?.abort();
    const controller = new AbortController();
    loadAbort = controller;
    setLoading(true);
    setError(null);
    setInspectedArtifacts(null);
    try {
      const [wfData, secData] = await Promise.all([
        fetchRepositoryWorkflows(repoId, controller.signal),
        fetchWorkflowSecrets(repoId, controller.signal),
      ]);
      if (reqId !== loadGen) return;
      setOverview(wfData);
      setSecrets(secData);
      if (!selectedWorkflow() && wfData.workflows[0]) setSelectedWorkflow(wfData.workflows[0].name);
      setLoading(false);
    } catch (err) {
      if (reqId !== loadGen) return;
      setLoading(false);
      setError(formatErr(err));
    }
  };

  createEffect(() => {
    const id = props.repositoryId;
    if (id && props.role === "SiteAdmin") {
      void loadAll(id);
    } else {
      loadGen++;
      loadAbort?.abort();
      setOverview(null);
      setSecrets([]);
      setInspectedArtifacts(null);
    }
  });

  onCleanup(() => {
    loadGen++;
    loadAbort?.abort();
  });

  const runAction = async (fn: (reqId: number) => Promise<void>) => {
    if (busy() || !props.repositoryId) return;
    const reqId = ++loadGen;
    loadAbort?.abort();
    setBusy(true);
    setError(null);
    try {
      await fn(reqId);
    } catch (err) {
      if (reqId === loadGen) setError(formatErr(err));
    } finally {
      if (reqId === loadGen) setBusy(false);
    }
  };

  const handleRunWorkflow = () =>
    void runAction(async (reqId) => {
      const run = await triggerWorkflowRun(props.repositoryId!, {
        expected_commit: props.defaultCommit ?? "0".repeat(40),
        source_ref: props.defaultRef ?? "refs/heads/main",
        workflow_name: selectedWorkflow() || overview()?.workflows[0]?.name,
      });
      if (reqId === loadGen) {
        setOverview((prev) => (prev ? { ...prev, runs: [run, ...prev.runs] } : prev));
      }
    });

  const handleInspectArtifacts = (runId: string) =>
    void runAction(async (reqId) => {
      const items = await fetchWorkflowArtifacts(props.repositoryId!, runId);
      if (reqId === loadGen) setInspectedArtifacts({ runId, items });
    });

  const handleSaveSecret = (e: Event) => {
    e.preventDefault();
    const name = secretName().trim();
    const value = secretValue();
    if (!name || !value) return;
    setSecretValue("");
    void runAction(async (reqId) => {
      const saved = await putWorkflowSecret(props.repositoryId!, name, value);
      if (reqId !== loadGen) return;
      setSecretName("");
      setSecrets((prev) => [...prev.filter((s) => s.name !== saved.name), saved]);
    });
  };

  const handleDeleteSecret = (name: string) =>
    void runAction(async (reqId) => {
      await deleteWorkflowSecret(props.repositoryId!, name);
      if (reqId === loadGen) setSecrets((prev) => prev.filter((s) => s.name !== name));
    });

  return (
    <section class="space-y-4 rounded-xl border bg-card p-4" aria-label="GitHub Actions-compatible workflows">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="space-y-0.5">
          <h2 class="flex items-center gap-2 text-sm font-semibold">
            <Icon name="layers" size={14} class="text-primary" />
            Workflows (DAG, Matrices, Secrets, Artifacts & Caches)
          </h2>
          <p class="text-xs text-muted-foreground">
            Execute multi-job dependency graphs with matrix expansion, encrypted secret redaction, artifact retention, and content-addressed caches.
          </p>
        </div>
        <Show when={overview()}>
          <Button type="button" size="sm" disabled={busy()} onClick={handleRunWorkflow}>
            <Icon name="play" size={12} />
            Run Workflow
          </Button>
        </Show>
      </div>

      <Show when={loading()}>
        <p role="status" class="text-xs text-muted-foreground">Loading workflows...</p>
      </Show>
      <Show when={error()}>
        {(err) => (
          <Alert variant="destructive">
            <AlertDescription>{err()}</AlertDescription>
          </Alert>
        )}
      </Show>

      <Show when={overview()}>
        {(data) => (
          <>
            <div class="space-y-2">
              <h3 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                Committed workflow definitions ({data().workflows.length})
              </h3>
              <For each={data().workflows}>
                {(wf) => (
                  <div class="space-y-2 rounded-lg border p-3 text-xs">
                    <div class="flex flex-wrap items-center justify-between gap-2">
                      <span class="font-semibold">{wf.name}</span>
                      <span class="text-muted-foreground">on: {wf.on.join(", ")}</span>
                    </div>
                    <div class="grid gap-2 sm:grid-cols-2">
                      <For each={wf.jobs}>
                        {(job) => (
                          <div class="space-y-1 rounded-md border bg-muted/20 p-2">
                            <div class="flex items-center justify-between gap-1 font-medium">
                              <span>{job.name}</span>
                              <span class="font-mono text-[11px] text-muted-foreground">{job.id}</span>
                            </div>
                            <div class="flex flex-wrap gap-1.5 text-[11px] text-muted-foreground">
                              <span>runs-on: {job.runs_on ?? "local"}</span>
                              <span>tier: {job.isolation_tier ?? "host_os"}</span>
                              <Show when={job.needs.length > 0}>
                                <span>needs: {job.needs.join(", ")}</span>
                              </Show>
                              <Show when={formatMatrix(job.matrix)}>{(m) => <span>{m()}</span>}</Show>
                              <Show when={job.cache_key}>{(ck) => <span>cache: {ck()}</span>}</Show>
                              <Show when={job.artifact_name}>{(art) => <span>artifact: {art()}</span>}</Show>
                            </div>
                          </div>
                        )}
                      </For>
                    </div>
                  </div>
                )}
              </For>
            </div>

            <div class="space-y-2 border-t pt-3">
              <h3 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                Workflow run history ({data().runs.length})
              </h3>
              <For each={data().runs}>
                {(run) => (
                  <div class="space-y-2 rounded-lg border p-3 text-xs">
                    <div class="flex flex-wrap items-center justify-between gap-2">
                      <div class="flex flex-wrap items-center gap-2">
                        <StatusBadge tone={run.status === "passed" ? "success" : "danger"}>{run.status}</StatusBadge>
                        <span class="font-semibold">{run.workflow_name}</span>
                        <span class="font-mono text-[11px] text-muted-foreground">{run.run_id.slice(0, 8)}</span>
                      </div>
                      <Button type="button" size="sm" variant="outline" disabled={busy()} onClick={() => handleInspectArtifacts(run.run_id)}>
                        <Icon name="folder" size={12} />
                        Inspect artifacts
                      </Button>
                    </div>
                    <div class="space-y-1.5">
                      <For each={run.jobs}>
                        {(job) => (
                          <div class="space-y-1 rounded-md border bg-muted/20 p-2">
                            <div class="flex flex-wrap items-center justify-between gap-2">
                              <div class="flex flex-wrap items-center gap-2">
                                <span class="font-medium">{job.display_name}</span>
                                <span class="text-[11px] text-muted-foreground">
                                  tier: {job.isolation_tier ?? "host_os"}
                                </span>
                                <Show when={job.runner_id}>
                                  {(r) => (
                                    <span class="text-[11px] text-muted-foreground">
                                      runner: {r()}
                                    </span>
                                  )}
                                </Show>
                              </div>
                              <div class="flex items-center gap-1.5">
                                <StatusBadge tone={job.cache_hit ? "info" : "neutral"}>
                                  {job.cache_hit ? "Cache Hit" : "Cache Miss"}
                                </StatusBadge>
                                <StatusBadge tone={job.status === "passed" ? "success" : job.status === "skipped" ? "neutral" : "danger"}>
                                  {job.status}
                                </StatusBadge>
                              </div>
                            </div>
                            <pre class="overflow-x-auto rounded bg-background p-1.5 font-mono text-[11px]">{job.redacted_output}</pre>
                          </div>
                        )}
                      </For>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </>
        )}
      </Show>

      <Show when={inspectedArtifacts()}>
        {(art) => (
          <div class="space-y-1.5 rounded-lg border bg-muted/30 p-3 text-xs">
            <div class="font-semibold">Artifacts for run {art().runId.slice(0, 8)} ({art().items.length})</div>
            <For each={art().items}>
              {(item) => (
                <div class="space-y-0.5 rounded border bg-background p-2">
                  <div class="flex items-center justify-between font-mono text-[11px]">
                    <span>{item.name}</span>
                    <span>{item.path} ({item.size_bytes} B)</span>
                  </div>
                  <Show when={item.content_preview}>
                    {(preview) => <pre class="font-mono text-[11px] text-muted-foreground">{preview()}</pre>}
                  </Show>
                </div>
              )}
            </For>
          </div>
        )}
      </Show>

      <div class="space-y-2 border-t pt-3">
        <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <Icon name="lock" size={12} />
          Encrypted workflow secrets ({secrets().length})
        </h3>
        <form class="grid gap-2 sm:grid-cols-3" onSubmit={handleSaveSecret}>
          <TextField>
            <TextFieldInput aria-label="Secret name" placeholder="Secret name (e.g. DEPLOY_TOKEN)" value={secretName()} onInput={(e) => setSecretName(e.currentTarget.value)} />
          </TextField>
          <TextField>
            <TextFieldInput type="password" aria-label="Secret value" placeholder="Write-only secret value" value={secretValue()} onInput={(e) => setSecretValue(e.currentTarget.value)} />
          </TextField>
          <div class="flex justify-end">
            <Button type="submit" size="sm" variant="secondary" disabled={busy()}>
              <Icon name="key" size={12} />
              Save secret
            </Button>
          </div>
        </form>
        <div class="space-y-1">
          <For each={secrets()}>
            {(sec) => (
              <div class="flex items-center justify-between rounded border px-2.5 py-1.5 text-xs">
                <span class="font-mono font-medium">{sec.name}</span>
                <Button type="button" size="sm" variant="outline" aria-label={`Delete secret ${sec.name}`} disabled={busy()} onClick={() => handleDeleteSecret(sec.name)}>
                  <Icon name="x" size={12} />
                  Delete
                </Button>
              </div>
            )}
          </For>
        </div>
      </div>

      <RunnerFleetAndCachePanel
        repositoryId={props.repositoryId}
        role={props.role}
        refreshEpoch={overview()?.runs.length ?? 0}
      />
    </section>
  );
};
