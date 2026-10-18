// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IconSpriteSheet } from "../src/components/workspace/icon";
import { RepositoryAdministrationPanel } from "../src/patterns/RepositoryAdministrationPanel";
import { SyncTopologyAndRollbackPanel } from "../src/patterns/SyncTopologyAndRollbackPanel";
import {
  fetchSyncTopology,
  rollbackSyncSnapshot,
  triggerBackupMirrorPush,
  updateSyncTopology,
  type BackupMirrorPushReport,
  type BackupMirrorSnapshot,
  type LiveSyncTopologyState,
  type RollbackVerificationReport,
} from "../src/showcase/live-sync-topology-client";

const REPO_ID = "11111111-1111-4111-8111-111111111111";
const SNAPSHOT_1 = "snap-20261018-0001";
const SNAPSHOT_2 = "snap-20261018-0002";
const SNAPSHOT_3 = "snap-20261018-0003";
const SHA_MAIN = "a".repeat(40);
const SHA_RELEASE = "b".repeat(40);

const makeSnapshot = (
  overrides: Partial<BackupMirrorSnapshot> = {},
): BackupMirrorSnapshot => ({
  snapshot_id: SNAPSHOT_1,
  repo_id: REPO_ID,
  mode: "inbound_mirror",
  backup_remote_url: "https://github.com/org/core-engine-backup.git",
  captured_at: "2026-10-18T06:00:00Z",
  refs: [
    { ref_name: "refs/heads/main", target_sha: SHA_MAIN },
    { ref_name: "refs/heads/release/v1", target_sha: SHA_RELEASE },
  ],
  filtered_agent_refs: 3,
  ...overrides,
});

const makePushReport = (
  overrides: Partial<BackupMirrorPushReport> = {},
): BackupMirrorPushReport => ({
  repo_id: REPO_ID,
  mode: "source_of_truth",
  backup_remote_url: "https://github.com/org/core-engine-backup.git",
  snapshot_id: SNAPSHOT_3,
  pushed_refs: [
    { ref_name: "refs/heads/main", target_sha: SHA_MAIN },
    { ref_name: "refs/heads/release/v1", target_sha: SHA_RELEASE },
  ],
  filtered_agent_refs: 4,
  verified_remote_match: true,
  synced_at: "2026-10-18T06:30:00Z",
  ...overrides,
});

const makeTopologyState = (
  overrides: Partial<LiveSyncTopologyState> = {},
): LiveSyncTopologyState => ({
  repo_id: REPO_ID,
  mode: "inbound_mirror",
  backup_remote_url: "https://github.com/org/core-engine-backup.git",
  snapshots: [makeSnapshot()],
  last_push_report: null,
  ...overrides,
});

const makeRollbackReport = (
  overrides: Partial<RollbackVerificationReport> = {},
): RollbackVerificationReport => ({
  repo_id: REPO_ID,
  snapshot_id: SNAPSHOT_1,
  restored_refs: [
    { ref_name: "refs/heads/main", target_sha: SHA_MAIN },
    { ref_name: "refs/heads/release/v1", target_sha: SHA_RELEASE },
  ],
  fsck_passed: true,
  rolled_back_at: "2026-10-18T07:00:00Z",
  ...overrides,
});

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("Slice 6: Source-of-Truth Mode, Upstream Backup Mirror & Verified Rollback", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "gitust_ses_sync_admin");
    window.localStorage.setItem("gitust_csrf_token", "csrf_sync_token_777");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("calls GET/PUT /sync/topology, POST /sync/backup-push, and POST /sync/rollback and rejects malformed responses", async () => {
    const calls: Array<{
      url: string;
      method: string;
      csrf: string | null;
      body?: unknown;
    }> = [];

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const headers = new Headers(init?.headers);
      const csrf = headers.get("X-CSRF-Token");
      const body =
        typeof init?.body === "string" && init.body.length > 0
          ? JSON.parse(init.body)
          : undefined;
      calls.push({ url, method, csrf, body });

      if (url === `/api/v1/repos/${REPO_ID}/sync/topology` && method === "GET") {
        return jsonResponse(makeTopologyState());
      }
      if (url === `/api/v1/repos/${REPO_ID}/sync/topology` && method === "PUT") {
        const payload = body as {
          mode: "inbound_mirror" | "source_of_truth";
          backup_remote_url?: string;
        };
        return jsonResponse(
          makeTopologyState({
            mode: payload.mode,
            backup_remote_url:
              payload.backup_remote_url ??
              "https://github.com/org/core-engine-backup.git",
            snapshots: [
              makeSnapshot({
                snapshot_id: SNAPSHOT_2,
                mode: payload.mode,
              }),
              makeSnapshot(),
            ],
          }),
        );
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/sync/backup-push` &&
        method === "POST"
      ) {
        return jsonResponse(makePushReport());
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/sync/rollback` &&
        method === "POST"
      ) {
        const payload = body as { snapshot_id: string };
        return jsonResponse(
          makeRollbackReport({ snapshot_id: payload.snapshot_id }),
        );
      }
      return jsonResponse({}, 404);
    });

    const topology = await fetchSyncTopology(REPO_ID);
    expect(topology.mode).toBe("inbound_mirror");
    expect(topology.snapshots).toHaveLength(1);
    expect(topology.snapshots[0]?.filtered_agent_refs).toBe(3);

    const updated = await updateSyncTopology(REPO_ID, {
      mode: "source_of_truth",
      backup_remote_url: "https://github.com/org/core-engine-dr.git",
    });
    expect(updated.mode).toBe("source_of_truth");
    expect(updated.backup_remote_url).toBe(
      "https://github.com/org/core-engine-dr.git",
    );
    expect(updated.snapshots).toHaveLength(2);

    const pushReport = await triggerBackupMirrorPush(REPO_ID);
    expect(pushReport.verified_remote_match).toBe(true);
    expect(pushReport.pushed_refs).toHaveLength(2);
    expect(pushReport.filtered_agent_refs).toBe(4);

    const rollbackReport = await rollbackSyncSnapshot(REPO_ID, SNAPSHOT_1);
    expect(rollbackReport.snapshot_id).toBe(SNAPSHOT_1);
    expect(rollbackReport.fsck_passed).toBe(true);
    expect(rollbackReport.restored_refs).toHaveLength(2);

    expect(calls).toHaveLength(4);
    expect(calls[1]?.csrf).toBe("csrf_sync_token_777");
    expect(calls[2]?.csrf).toBe("csrf_sync_token_777");
    expect(calls[3]?.csrf).toBe("csrf_sync_token_777");

    // Reject malformed topology mode
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      jsonResponse({ ...makeTopologyState(), mode: "invalid_mode" }),
    );
    await expect(fetchSyncTopology(REPO_ID)).rejects.toThrow(
      /invalid sync topology/i,
    );
  });

  it("renders SyncTopologyAndRollbackPanel, switches to source_of_truth, pushes backup mirror, and rolls back to a snapshot", async () => {
    let currentState = makeTopologyState();
    let rolledBackCount = 0;
    const calls: Array<{ url: string; method: string; body?: unknown }> = [];

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const body =
        typeof init?.body === "string" && init.body.length > 0
          ? JSON.parse(init.body)
          : undefined;
      calls.push({ url, method, body });

      if (url === `/api/v1/repos/${REPO_ID}/sync/topology` && method === "GET") {
        return jsonResponse(currentState);
      }
      if (url === `/api/v1/repos/${REPO_ID}/sync/topology` && method === "PUT") {
        const payload = body as {
          mode: "inbound_mirror" | "source_of_truth";
          backup_remote_url?: string;
        };
        currentState = makeTopologyState({
          mode: payload.mode,
          backup_remote_url:
            payload.backup_remote_url ?? currentState.backup_remote_url,
          snapshots: [
            makeSnapshot({
              snapshot_id: SNAPSHOT_2,
              mode: payload.mode,
              filtered_agent_refs: 5,
            }),
            ...currentState.snapshots,
          ],
          last_push_report: currentState.last_push_report,
        });
        return jsonResponse(currentState);
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/sync/backup-push` &&
        method === "POST"
      ) {
        const report = makePushReport({
          backup_remote_url: currentState.backup_remote_url,
        });
        currentState = {
          ...currentState,
          last_push_report: report,
        };
        return jsonResponse(report);
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/sync/rollback` &&
        method === "POST"
      ) {
        const payload = body as { snapshot_id: string };
        return jsonResponse(
          makeRollbackReport({ snapshot_id: payload.snapshot_id }),
        );
      }
      return jsonResponse({}, 404);
    });

    render(() => (
      <>
        <IconSpriteSheet />
        <SyncTopologyAndRollbackPanel
          repositoryId={REPO_ID}
          onRolledBack={() => {
            rolledBackCount++;
          }}
        />
      </>
    ));

    // Wait for initial topology state to render
    await waitFor(() => {
      expect(screen.getByText("Inbound Mirror")).toBeTruthy();
      expect(screen.getByText(SNAPSHOT_1)).toBeTruthy();
    });

    expect(
      (
        screen.getByLabelText("Backup mirror remote URL") as HTMLInputElement
      ).value,
    ).toBe("https://github.com/org/core-engine-backup.git");
    expect(screen.getByText(/2 pinned refs/i)).toBeTruthy();
    expect(screen.getByText(/3 agent refs filtered/i)).toBeTruthy();

    // Update backup remote URL and switch topology mode to source_of_truth
    await fireEvent.input(screen.getByLabelText("Backup mirror remote URL"), {
      target: { value: "https://github.com/org/core-engine-dr.git" },
    });
    await fireEvent.change(screen.getByLabelText("Sync topology mode"), {
      target: { value: "source_of_truth" },
    });
    await fireEvent.click(
      screen.getByRole("button", { name: /Apply sync topology/i }),
    );

    await waitFor(() => {
      expect(screen.getByText("Source of Truth")).toBeTruthy();
      expect(screen.getByText(SNAPSHOT_2)).toBeTruthy();
    });

    const putCall = calls.find(
      (c) =>
        c.url === `/api/v1/repos/${REPO_ID}/sync/topology` &&
        c.method === "PUT",
    );
    expect(putCall?.body).toEqual({
      mode: "source_of_truth",
      backup_remote_url: "https://github.com/org/core-engine-dr.git",
    });

    // Trigger backup mirror push
    await fireEvent.click(
      screen.getByRole("button", { name: /Push Backup Mirror/i }),
    );

    await waitFor(() => {
      expect(screen.getByText("Remote Match Verified")).toBeTruthy();
      expect(screen.getAllByText(SNAPSHOT_3)).toHaveLength(2);
    });

    const pushCall = calls.find(
      (c) =>
        c.url === `/api/v1/repos/${REPO_ID}/sync/backup-push` &&
        c.method === "POST",
    );
    expect(pushCall).toBeTruthy();

    // Rollback to SNAPSHOT_1 (now at index 2 after SNAPSHOT_3 was prepended)
    const rollbackButtons = screen.getAllByRole("button", {
      name: /Rollback to Snapshot/i,
    });
    expect(rollbackButtons).toHaveLength(3);
    await fireEvent.click(rollbackButtons[2]!);

    await waitFor(() => {
      expect(screen.getByText(/fsck passed/i)).toBeTruthy();
      expect(rolledBackCount).toBe(1);
    });

    const rollbackCall = calls.find(
      (c) =>
        c.url === `/api/v1/repos/${REPO_ID}/sync/rollback` &&
        c.method === "POST",
    );
    expect(rollbackCall?.body).toEqual({ snapshot_id: SNAPSHOT_1 });
  });

  it("mounts SyncTopologyAndRollbackPanel inside RepositoryAdministrationPanel", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = typeof input === "string" ? input : input.url;
      if (url === `/api/v1/repos/${REPO_ID}/settings`) {
        return jsonResponse({
          id: REPO_ID,
          name: "core-engine",
          description: "Primary engine",
          default_branch: "main",
          changeable_branches: ["main"],
          is_private: true,
          archived: false,
          git_write: true,
          workbench_enabled: true,
          is_default: true,
        });
      }
      if (url === `/api/v1/repos/${REPO_ID}/sync/topology`) {
        return jsonResponse(
          makeTopologyState({
            mode: "source_of_truth",
            last_push_report: makePushReport(),
          }),
        );
      }
      return jsonResponse({}, 404);
    });

    render(() => (
      <>
        <IconSpriteSheet />
        <RepositoryAdministrationPanel
          repositoryId={REPO_ID}
          repositoryName="core-engine"
        />
      </>
    ));

    await waitFor(() => {
      expect(screen.getByText("Source of Truth")).toBeTruthy();
      expect(screen.getByText("Remote Match Verified")).toBeTruthy();
    });
  });
});
