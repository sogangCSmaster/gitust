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
  fetchSyncTopology,
  rollbackSyncSnapshot,
  triggerBackupMirrorPush,
  updateSyncTopology,
  type BackupMirrorPushReport,
  type LiveSyncTopologyState,
  type RollbackVerificationReport,
  type SyncTopologyMode,
} from "../showcase/live-sync-topology-client";

export interface SyncTopologyAndRollbackPanelProps {
  repositoryId: string;
  onRolledBack?: () => void;
}

export const SyncTopologyAndRollbackPanel: Component<
  SyncTopologyAndRollbackPanelProps
> = (props) => {
  const [topology, setTopology] = createSignal<LiveSyncTopologyState | null>(null);
  const [modeInput, setModeInput] = createSignal<SyncTopologyMode>("inbound_mirror");
  const [remoteUrlInput, setRemoteUrlInput] = createSignal("");
  const [pushReport, setPushReport] = createSignal<BackupMirrorPushReport | null>(null);
  const [rollbackReport, setRollbackReport] = createSignal<RollbackVerificationReport | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let loadGen = 0;
  let loadAbort: AbortController | undefined;

  const applyTopology = (next: LiveSyncTopologyState) => {
    setTopology(next);
    setModeInput(next.mode);
    setRemoteUrlInput(next.backup_remote_url);
    if (next.last_push_report !== undefined) setPushReport(next.last_push_report ?? null);
  };

  const loadTopology = async (repoId: string) => {
    const reqId = ++loadGen;
    loadAbort?.abort();
    const controller = new AbortController();
    loadAbort = controller;
    setLoading(true);
    setError(null);
    setPushReport(null);
    setRollbackReport(null);
    try {
      const loaded = await fetchSyncTopology(repoId, controller.signal);
      if (reqId !== loadGen) return;
      applyTopology(loaded);
      setLoading(false);
    } catch (err) {
      if (reqId !== loadGen) return;
      setLoading(false);
      setError(formatErr(err));
    }
  };

  createEffect(() => {
    const id = props.repositoryId;
    if (id) void loadTopology(id);
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

  const handleApplyTopology = (e: Event) => {
    e.preventDefault();
    setRollbackReport(null);
    void runAction(async (reqId) => {
      const updated = await updateSyncTopology(props.repositoryId, {
        mode: modeInput(),
        backup_remote_url: remoteUrlInput().trim(),
      });
      if (reqId === loadGen) applyTopology(updated);
    });
  };

  const handlePushBackup = () =>
    void runAction(async (reqId) => {
      const report = await triggerBackupMirrorPush(props.repositoryId);
      if (reqId !== loadGen) return;
      setPushReport(report);
      setTopology((prev) =>
        prev
          ? {
              ...prev,
              last_push_report: report,
              snapshots: [
                {
                  snapshot_id: report.snapshot_id,
                  repo_id: report.repo_id,
                  mode: report.mode,
                  backup_remote_url: report.backup_remote_url,
                  captured_at: report.synced_at,
                  refs: report.pushed_refs,
                  filtered_agent_refs: report.filtered_agent_refs,
                },
                ...prev.snapshots.filter(
                  (s) => s.snapshot_id !== report.snapshot_id,
                ),
              ].slice(0, 32),
            }
          : prev,
      );
    });

  const handleRollback = (snapshotId: string) =>
    void runAction(async (reqId) => {
      const report = await rollbackSyncSnapshot(props.repositoryId, snapshotId);
      if (reqId !== loadGen) return;
      setRollbackReport(report);
      props.onRolledBack?.();
    });

  return (
    <div class="space-y-3 border-t pt-3" aria-label="Sync topology and backup mirror">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="space-y-0.5">
          <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            <Icon name="server" size={13} class="text-primary" />
            Source-of-truth topology & disaster recovery
          </h3>
          <p class="text-xs text-muted-foreground">
            Promote repository to source-of-truth, push outbound backup mirrors, or rollback to a pinned ref snapshot.
          </p>
        </div>
        <Show when={topology()}>
          {(state) => (
            <div class="flex flex-wrap items-center gap-1.5">
              <StatusBadge tone={state().mode === "source_of_truth" ? "success" : "info"}>
                {state().mode === "source_of_truth" ? "Source of Truth" : "Inbound Mirror"}
              </StatusBadge>
              <Button type="button" size="sm" variant="outline" disabled={busy()} onClick={handlePushBackup}>
                <Icon name="shield-check" size={12} />
                Push Backup Mirror
              </Button>
            </div>
          )}
        </Show>
      </div>

      <Show when={loading()}>
        <p role="status" class="text-xs text-muted-foreground">Loading sync topology...</p>
      </Show>
      <Show when={error()}>
        {(err) => (
          <Alert variant="destructive">
            <AlertDescription>{err()}</AlertDescription>
          </Alert>
        )}
      </Show>

      <Show when={pushReport()}>
        {(report) => (
          <div class="flex flex-wrap items-center justify-between gap-2 rounded-lg border bg-muted/30 px-3 py-2 text-xs">
            <div class="flex flex-wrap items-center gap-2">
              <StatusBadge tone={report().verified_remote_match ? "success" : "warning"}>
                {report().verified_remote_match ? "Remote Match Verified" : "Remote Mismatch"}
              </StatusBadge>
              <span>
                Pushed {report().pushed_refs.length} canonical refs ({report().filtered_agent_refs} agent refs filtered)
              </span>
            </div>
            <span class="font-mono text-[11px] text-muted-foreground">{report().snapshot_id}</span>
          </div>
        )}
      </Show>

      <Show when={rollbackReport()}>
        {(report) => (
          <div role="status" class="flex flex-wrap items-center justify-between gap-2 rounded-lg border bg-muted/30 px-3 py-2 text-xs">
            <div class="flex flex-wrap items-center gap-2">
              <StatusBadge tone={report().fsck_passed ? "success" : "danger"}>
                {report().fsck_passed ? "fsck passed" : "fsck failed"}
              </StatusBadge>
              <span>
                Restored {report().restored_refs.length} refs from snapshot <code class="font-mono">{report().snapshot_id}</code>
              </span>
            </div>
            <span class="text-[11px] text-muted-foreground">{report().rolled_back_at}</span>
          </div>
        )}
      </Show>

      <Show when={topology()}>
        <form class="grid gap-2 sm:grid-cols-3" onSubmit={handleApplyTopology}>
          <select
            aria-label="Sync topology mode"
            class="h-8 rounded-md border bg-background px-2.5 text-xs"
            value={modeInput()}
            onChange={(e) => setModeInput(e.currentTarget.value as SyncTopologyMode)}
          >
            <option value="inbound_mirror">inbound_mirror (Read-only)</option>
            <option value="source_of_truth">source_of_truth (Primary)</option>
          </select>
          <TextField>
            <TextFieldInput
              aria-label="Backup mirror remote URL"
              placeholder="https://github.com/org/repo-backup.git"
              value={remoteUrlInput()}
              onInput={(e) => setRemoteUrlInput(e.currentTarget.value)}
            />
          </TextField>
          <div class="flex justify-end">
            <Button type="submit" size="sm" variant="secondary" disabled={busy()}>
              <Icon name="check" />
              Apply sync topology
            </Button>
          </div>
        </form>

        <div class="space-y-1.5">
          <div class="text-[11px] font-medium text-muted-foreground">
            Pre-cutover & backup ref snapshots ({topology()!.snapshots.length})
          </div>
          <For each={topology()!.snapshots}>
            {(snap) => (
              <div class="flex flex-wrap items-center justify-between gap-2 rounded-lg border px-3 py-2 text-xs">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="font-mono font-medium">{snap.snapshot_id}</span>
                  <span class="text-muted-foreground">
                    {snap.refs.length} pinned refs · {snap.filtered_agent_refs} agent refs filtered
                  </span>
                </div>
                <Button type="button" size="sm" variant="outline" disabled={busy()} onClick={() => handleRollback(snap.snapshot_id)}>
                  <Icon name="rotate-ccw" size={12} />
                  Rollback to Snapshot
                </Button>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
};
