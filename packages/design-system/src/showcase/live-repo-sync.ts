import {
  formatErr,
  getLiveToken,
  type LiveRepoClientCallbacks,
} from "./live-repo-context";

/** Sync authority remains independent of the selected read-only branch snapshot. */
export const createLiveRepositorySync = (
  cb: LiveRepoClientCallbacks,
  getEpoch: () => number,
  sessionCurrent: (
    epoch: number,
    token: string | null,
    repository: string | null | undefined,
  ) => boolean,
  hydrateFromLiveBackend: () => Promise<void>,
) => {
  return () => {
    if (!cb.canWorkbench()) return;
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback(
        "Sign in to fetch from the configured GitHub mirror",
      );
      return;
    }
    const epoch = getEpoch();
    const repositoryId = cb.getRepositoryId();
    const t0 = performance.now();
    cb.showQuickFeedback(
      "GitHub Read-Only Mirror Sync: Synchronizing upstream refs (Agent branches kept private)...",
    );
    void (async () => {
      try {
        const res = await fetch("/api/v1/sync/github", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
          body: "{}",
        });
        if (!sessionCurrent(epoch, token, repositoryId)) return;
        const dt = (performance.now() - t0).toFixed(1);
        if (res.ok) {
          const report = await res.json();
          if (!sessionCurrent(epoch, token, repositoryId)) return;
          const sha = String(report.last_synced_commit ?? "").slice(0, 7);
          const priv = Number(report.private_agent_branches_filtered ?? 0);
          const skippedCount = Array.isArray(report.skipped_branches)
            ? report.skipped_branches.length
            : 0;
          cb.showQuickFeedback(
            `GitHub Mirror Synced @ ${sha} in ${dt}ms (${priv} private agent branches isolated, ${skippedCount} skipped)`,
          );
          if (cb.onRepositoryChanged) await cb.onRepositoryChanged();
          else await hydrateFromLiveBackend();
        } else {
          const errText = (await res.text().catch(() => "")).trim();
          if (!sessionCurrent(epoch, token, repositoryId)) return;
          cb.showQuickFeedback(
            `GitHub Mirror Sync rejected (HTTP ${res.status} in ${dt}ms${errText ? `: ${errText}` : ""})`,
          );
        }
      } catch (err) {
        if (!sessionCurrent(epoch, token, repositoryId)) return;
        cb.showQuickFeedback(`GitHub Mirror Sync failed: ${formatErr(err)}`);
      }
    })();
  };
};
