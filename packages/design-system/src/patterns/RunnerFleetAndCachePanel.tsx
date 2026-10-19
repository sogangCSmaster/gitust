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
import { formatErr } from "../showcase/live-repo-context";
import {
  deleteRunnerNode,
  evictRepositoryCache,
  fetchRepositoryCaches,
  fetchRunnerFleet,
  registerRunnerNode,
  updateRunnerHeartbeat,
  type LiveIsolationTier,
  type LiveRunnerNodeRecord,
  type LiveSharedCacheEntry,
} from "../showcase/live-runners-client";

export interface RunnerFleetAndCachePanelProps {
  repositoryId?: string;
  role?: string;
  refreshEpoch?: number;
}

export const RunnerFleetAndCachePanel: Component<
  RunnerFleetAndCachePanelProps
> = (props) => {
  const [runners, setRunners] = createSignal<LiveRunnerNodeRecord[]>([]);
  const [caches, setCaches] = createSignal<LiveSharedCacheEntry[]>([]);
  const [runnerName, setRunnerName] = createSignal("");
  const [runnerLabels, setRunnerLabels] = createSignal("linux-vm");
  const [isolationTier, setIsolationTier] =
    createSignal<LiveIsolationTier>("host_os");
  const [maxConcurrency, setMaxConcurrency] = createSignal("2");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let loadGen = 0;
  let loadAbort: AbortController | undefined;

  const loadData = async (repoId: string, isAdmin: boolean) => {
    const reqId = ++loadGen;
    loadAbort?.abort();
    const controller = new AbortController();
    loadAbort = controller;
    const [cacheList, runnerList] = await Promise.all([
      fetchRepositoryCaches(repoId, controller.signal).catch(
        () => [] as LiveSharedCacheEntry[],
      ),
      isAdmin
        ? fetchRunnerFleet(controller.signal).catch(
            () => [] as LiveRunnerNodeRecord[],
          )
        : Promise.resolve([] as LiveRunnerNodeRecord[]),
    ]);
    if (reqId !== loadGen) return;
    setCaches(cacheList);
    setRunners(runnerList);
  };

  createEffect(() => {
    const repoId = props.repositoryId;
    const role = props.role;
    void props.refreshEpoch;
    if (repoId && (role === "SiteAdmin" || role === "Maintainer" || role === "Developer")) {
      void loadData(repoId, role === "SiteAdmin");
    } else {
      loadGen++;
      loadAbort?.abort();
      setRunners([]);
      setCaches([]);
    }
  });

  onCleanup(() => {
    loadGen++;
    loadAbort?.abort();
  });

  const runAction = async (fn: (reqId: number) => Promise<void>) => {
    if (busy()) return;
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

  const handleRegisterRunner = (e: Event) => {
    e.preventDefault();
    const name = runnerName().trim();
    if (!name) return;
    const labels = runnerLabels()
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const concurrency = Math.max(1, Number.parseInt(maxConcurrency(), 10) || 1);
    const tier = isolationTier();
    void runAction(async (reqId) => {
      const created = await registerRunnerNode({
        name,
        labels: labels.length > 0 ? labels : ["local"],
        supported_tiers: [tier],
        max_concurrency: concurrency,
      });
      if (reqId !== loadGen) return;
      setRunnerName("");
      setRunners((prev) => [...prev.filter((r) => r.id !== created.id), created]);
    });
  };

  const handleToggleDrain = (runner: LiveRunnerNodeRecord) => {
    const nextStatus = runner.status === "draining" ? "online" : "draining";
    void runAction(async (reqId) => {
      const updated = await updateRunnerHeartbeat(runner.id, nextStatus);
      if (reqId === loadGen) {
        setRunners((prev) =>
          prev.map((r) => (r.id === updated.id ? updated : r)),
        );
      }
    });
  };

  const handleDeleteRunner = (runner: LiveRunnerNodeRecord) =>
    void runAction(async (reqId) => {
      await deleteRunnerNode(runner.id);
      if (reqId === loadGen) {
        setRunners((prev) => prev.filter((r) => r.id !== runner.id));
      }
    });

  const handleEvictCache = (key: string) =>
    void runAction(async (reqId) => {
      if (!props.repositoryId) return;
      await evictRepositoryCache(props.repositoryId, key);
      if (reqId === loadGen) {
        setCaches((prev) => prev.filter((c) => c.key !== key));
      }
    });

  return (
    <div class="space-y-4 border-t pt-3" aria-label="Runner fleet and shared caches">
      <Show when={error()}>
        {(err) => (
          <Alert variant="destructive">
            <AlertDescription>{err()}</AlertDescription>
          </Alert>
        )}
      </Show>

      <Show when={props.role === "SiteAdmin"}>
        <div class="space-y-2">
          <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            <Icon name="cpu" size={12} />
            Runner inventory ({runners().length})
          </h3>
          <p class="text-xs text-muted-foreground">
            Workflows run on built-in-local with host_os isolation. Registered remote
            runners are inventory only; remote execution and container_vm are unavailable.
          </p>
          <form class="grid gap-2 sm:grid-cols-5" onSubmit={handleRegisterRunner}>
            <TextField>
              <TextFieldInput
                aria-label="Runner name"
                placeholder="Runner name (e.g. vm-runner-1)"
                value={runnerName()}
                onInput={(e) => setRunnerName(e.currentTarget.value)}
              />
            </TextField>
            <TextField>
              <TextFieldInput
                aria-label="Runner labels"
                placeholder="Labels (e.g. linux-vm)"
                value={runnerLabels()}
                onInput={(e) => setRunnerLabels(e.currentTarget.value)}
              />
            </TextField>
            <select
              aria-label="Isolation tier"
              class="h-8 rounded-md border bg-background px-2 text-xs"
              value={isolationTier()}
              onChange={(e) =>
                setIsolationTier(e.currentTarget.value as LiveIsolationTier)
              }
            >
              <option value="container_vm" disabled>container_vm (unavailable)</option>
              <option value="host_os">host_os</option>
            </select>
            <TextField>
              <TextFieldInput
                type="number"
                aria-label="Max concurrency"
                placeholder="Concurrency"
                value={maxConcurrency()}
                onInput={(e) => setMaxConcurrency(e.currentTarget.value)}
              />
            </TextField>
            <div class="flex justify-end">
              <Button type="submit" size="sm" variant="secondary" disabled={busy()}>
                <Icon name="plus" size={12} />
                Register runner
              </Button>
            </div>
          </form>
          <div class="space-y-1.5">
            <For each={runners()}>
              {(runner) => (
                <div class="flex flex-wrap items-center justify-between gap-2 rounded border px-2.5 py-1.5 text-xs">
                  <div class="flex flex-wrap items-center gap-2">
                    <span class="font-mono font-semibold">{runner.name}</span>
                    <StatusBadge
                      tone={
                        runner.status === "online"
                          ? "success"
                          : runner.status === "draining"
                            ? "warning"
                            : "neutral"
                      }
                    >
                      {runner.status}
                    </StatusBadge>
                    <span class="text-muted-foreground">
                      tiers: {runner.supported_tiers.join(", ")} · labels:{" "}
                      {runner.labels.join(", ")} · leases: {runner.active_leases}/
                      {runner.max_concurrency}
                    </span>
                  </div>
                  <div class="flex items-center gap-1.5">
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      aria-label={`Toggle drain ${runner.name}`}
                      disabled={busy()}
                      onClick={() => handleToggleDrain(runner)}
                    >
                      {runner.status === "draining" ? "Resume" : "Drain"}
                    </Button>
                    <Show when={runner.name !== "built-in-local" && runner.id !== "built-in-local"}>
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        aria-label={`Delete runner ${runner.name}`}
                        disabled={busy()}
                        onClick={() => handleDeleteRunner(runner)}
                      >
                        <Icon name="x" size={12} />
                        Delete
                      </Button>
                    </Show>
                  </div>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>

      <div class="space-y-1.5">
        <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <Icon name="server" size={12} />
          Shared build caches ({caches().length})
        </h3>
        <For each={caches()}>
          {(cache) => (
            <div class="flex flex-wrap items-center justify-between gap-2 rounded border px-2.5 py-1.5 text-xs">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono font-medium">{cache.key}</span>
                <span class="text-muted-foreground">
                  {cache.size_bytes} B · {cache.files_count} files
                </span>
                <Show when={cache.source_job_id}>
                  {(src) => (
                    <span class="text-muted-foreground">source: {src()}</span>
                  )}
                </Show>
              </div>
              <Show
                when={props.role === "SiteAdmin" || props.role === "Maintainer"}
              >
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  aria-label={`Evict cache ${cache.key}`}
                  disabled={busy()}
                  onClick={() => handleEvictCache(cache.key)}
                >
                  <Icon name="x" size={12} />
                  Evict
                </Button>
              </Show>
            </div>
          )}
        </For>
      </div>
    </div>
  );
};
