import { getLiveToken } from "./live-repo-context";

export type SyncTopologyMode = "inbound_mirror" | "source_of_truth";
export interface MirrorRefPin {
  ref_name: string;
  target_sha: string;
}
export interface BackupMirrorSnapshot {
  snapshot_id: string;
  repo_id: string;
  mode: SyncTopologyMode;
  backup_remote_url: string;
  captured_at: string;
  refs: MirrorRefPin[];
  filtered_agent_refs: number;
}
export interface BackupMirrorPushReport {
  repo_id: string;
  mode: SyncTopologyMode;
  backup_remote_url: string;
  snapshot_id: string;
  pushed_refs: MirrorRefPin[];
  filtered_agent_refs: number;
  verified_remote_match: boolean;
  synced_at: string;
}
export interface RollbackVerificationReport {
  repo_id: string;
  snapshot_id: string;
  restored_refs: MirrorRefPin[];
  fsck_passed: boolean;
  rolled_back_at: string;
}
export interface LiveSyncTopologyState {
  repo_id: string;
  mode: SyncTopologyMode;
  backup_remote_url: string;
  snapshots: BackupMirrorSnapshot[];
  last_push_report?: BackupMirrorPushReport | null;
}
export interface UpdateSyncTopologyInput {
  mode: SyncTopologyMode;
  backup_remote_url?: string;
}

const getCsrfHeader = (token: string): string => {
  try {
    if (typeof window !== "undefined") {
      const s =
        window.localStorage.getItem("gitust_csrf_token") ??
        window.localStorage.getItem("gitust_csrf");
      if (s && s.trim()) return s.trim();
    }
  } catch {
    // Ignore storage read errors
  }
  return `csrf-${token.slice(0, 16)}`;
};

const isObj = (v: unknown): v is Record<string, unknown> =>
  Boolean(v && typeof v === "object" && !Array.isArray(v));
const isMode = (v: unknown): v is SyncTopologyMode =>
  v === "inbound_mirror" || v === "source_of_truth";
const isStr = (v: unknown): v is string => typeof v === "string";

const parseRefPins = (raw: unknown): MirrorRefPin[] => {
  if (!Array.isArray(raw)) throw new Error("Invalid sync topology ref pins");
  return raw.map((item) => {
    if (!isObj(item) || !isStr(item.ref_name) || !isStr(item.target_sha))
      throw new Error("Malformed sync topology ref pin fields");
    return { ref_name: item.ref_name, target_sha: item.target_sha };
  });
};

const parseSnapshot = (raw: unknown): BackupMirrorSnapshot => {
  if (!isObj(raw) || !isStr(raw.snapshot_id) || !isStr(raw.repo_id) || !isMode(raw.mode) || !isStr(raw.backup_remote_url) || !isStr(raw.captured_at) || typeof raw.filtered_agent_refs !== "number")
    throw new Error("Malformed sync topology snapshot fields");
  return {
    snapshot_id: raw.snapshot_id,
    repo_id: raw.repo_id,
    mode: raw.mode,
    backup_remote_url: raw.backup_remote_url,
    captured_at: raw.captured_at,
    refs: parseRefPins(raw.refs),
    filtered_agent_refs: raw.filtered_agent_refs,
  };
};

const parsePushReport = (raw: unknown): BackupMirrorPushReport => {
  if (!isObj(raw) || !isStr(raw.repo_id) || !isMode(raw.mode) || !isStr(raw.backup_remote_url) || !isStr(raw.snapshot_id) || typeof raw.filtered_agent_refs !== "number" || typeof raw.verified_remote_match !== "boolean" || !isStr(raw.synced_at))
    throw new Error("Malformed backup mirror push report fields");
  return {
    repo_id: raw.repo_id,
    mode: raw.mode,
    backup_remote_url: raw.backup_remote_url,
    snapshot_id: raw.snapshot_id,
    pushed_refs: parseRefPins(raw.pushed_refs),
    filtered_agent_refs: raw.filtered_agent_refs,
    verified_remote_match: raw.verified_remote_match,
    synced_at: raw.synced_at,
  };
};

const parseRollbackReport = (raw: unknown): RollbackVerificationReport => {
  if (!isObj(raw) || !isStr(raw.repo_id) || !isStr(raw.snapshot_id) || typeof raw.fsck_passed !== "boolean" || !isStr(raw.rolled_back_at))
    throw new Error("Malformed rollback verification report fields");
  return {
    repo_id: raw.repo_id,
    snapshot_id: raw.snapshot_id,
    restored_refs: parseRefPins(raw.restored_refs),
    fsck_passed: raw.fsck_passed,
    rolled_back_at: raw.rolled_back_at,
  };
};

const parseTopologyState = (raw: unknown): LiveSyncTopologyState => {
  if (
    !isObj(raw) ||
    !isStr(raw.repo_id) ||
    !isMode(raw.mode) ||
    !isStr(raw.backup_remote_url) ||
    !Array.isArray(raw.snapshots)
  )
    throw new Error("Invalid sync topology state fields");
  return {
    repo_id: raw.repo_id,
    mode: raw.mode,
    backup_remote_url: raw.backup_remote_url,
    snapshots: raw.snapshots.map(parseSnapshot),
    last_push_report:
      raw.last_push_report != null ? parsePushReport(raw.last_push_report) : null,
  };
};

async function syncRequest<T>(
  path: string,
  parse: (v: unknown) => T,
  label: string,
  init?: { method?: string; body?: unknown; signal?: AbortSignal },
): Promise<T> {
  const token = getLiveToken();
  if (!token) throw new Error("Sign in as SiteAdmin to manage sync topology");
  const method = init?.method ?? "GET";
  const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
  if (method !== "GET") {
    headers["Content-Type"] = "application/json";
    headers["X-CSRF-Token"] = getCsrfHeader(token);
  }
  const res = await fetch(path, {
    method,
    headers,
    body: init?.body !== undefined ? JSON.stringify(init.body) : undefined,
    cache: method === "GET" ? "no-store" : undefined,
    signal: init?.signal,
  });
  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(`Unable to ${label} (HTTP ${res.status}${detail ? `: ${detail}` : ""})`);
  }
  return parse(await res.json());
}

const syncUrl = (id: string, suffix: string) => `/api/v1/repos/${encodeURIComponent(id)}/sync/${suffix}`;
export const fetchSyncTopology = (id: string, signal?: AbortSignal) =>
  syncRequest(syncUrl(id, "topology"), parseTopologyState, "load sync topology", { signal });
export const updateSyncTopology = (id: string, input: UpdateSyncTopologyInput) =>
  syncRequest(syncUrl(id, "topology"), parseTopologyState, "update sync topology", { method: "PUT", body: input });
export const triggerBackupMirrorPush = (id: string) =>
  syncRequest(syncUrl(id, "backup-push"), parsePushReport, "push backup mirror", { method: "POST", body: {} });
export const rollbackSyncSnapshot = (id: string, snapshotId: string) =>
  syncRequest(syncUrl(id, "rollback"), parseRollbackReport, "rollback snapshot", { method: "POST", body: { snapshot_id: snapshotId } });
