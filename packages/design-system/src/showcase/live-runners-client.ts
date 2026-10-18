import { getLiveToken } from "./live-repo-context";

export type LiveIsolationTier = "host_os" | "container_vm";
export type LiveRunnerStatus = "online" | "draining" | "offline";

export interface LiveRunnerNodeRecord {
  id: string;
  name: string;
  labels: string[];
  supported_tiers: LiveIsolationTier[];
  max_concurrency: number;
  active_leases: number;
  status: LiveRunnerStatus;
  registered_at: string;
  last_heartbeat_at: string;
}

export interface LiveSharedCacheEntry {
  key: string;
  digest_sha256: string;
  size_bytes: number;
  files_count: number;
  created_at_epoch_secs: number;
  source_job_id?: string | null;
}

export interface RegisterRunnerNodeInput {
  name: string;
  labels: string[];
  supported_tiers: LiveIsolationTier[];
  max_concurrency: number;
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
const isStr = (v: unknown): v is string => typeof v === "string";
const isTier = (v: unknown): v is LiveIsolationTier =>
  v === "host_os" || v === "container_vm";
const isStatus = (v: unknown): v is LiveRunnerStatus =>
  v === "online" || v === "draining" || v === "offline";

const parseRunner = (raw: unknown): LiveRunnerNodeRecord => {
  if (
    !isObj(raw) ||
    !isStr(raw.id) ||
    !isStr(raw.name) ||
    !Array.isArray(raw.labels) ||
    !Array.isArray(raw.supported_tiers) ||
    typeof raw.max_concurrency !== "number" ||
    typeof raw.active_leases !== "number" ||
    !isStatus(raw.status) ||
    !isStr(raw.registered_at) ||
    !isStr(raw.last_heartbeat_at)
  )
    throw new Error("Invalid runner node record");
  return {
    id: raw.id,
    name: raw.name,
    labels: raw.labels.filter(isStr),
    supported_tiers: raw.supported_tiers.filter(isTier),
    max_concurrency: raw.max_concurrency,
    active_leases: raw.active_leases,
    status: raw.status,
    registered_at: raw.registered_at,
    last_heartbeat_at: raw.last_heartbeat_at,
  };
};

const parseCacheEntry = (raw: unknown): LiveSharedCacheEntry => {
  if (
    !isObj(raw) ||
    !isStr(raw.key) ||
    !isStr(raw.digest_sha256) ||
    typeof raw.size_bytes !== "number" ||
    typeof raw.files_count !== "number" ||
    typeof raw.created_at_epoch_secs !== "number"
  )
    throw new Error("Invalid shared cache entry");
  return {
    key: raw.key,
    digest_sha256: raw.digest_sha256,
    size_bytes: raw.size_bytes,
    files_count: raw.files_count,
    created_at_epoch_secs: raw.created_at_epoch_secs,
    source_job_id: isStr(raw.source_job_id) ? raw.source_job_id : null,
  };
};

async function runnerRequest<T>(
  path: string,
  parse: (v: unknown) => T,
  label: string,
  init?: { method?: string; body?: unknown; signal?: AbortSignal },
): Promise<T> {
  const token = getLiveToken();
  if (!token) throw new Error("Sign in to manage runner fleet and build caches");
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
  return res.status === 204 ? parse(undefined) : parse(await res.json());
}

export const fetchRunnerFleet = (signal?: AbortSignal) =>
  runnerRequest("/api/v1/admin/runners", (raw) => {
    if (!isObj(raw) || !Array.isArray(raw.runners)) throw new Error("Invalid runner fleet payload");
    return raw.runners.map(parseRunner);
  }, "load runner fleet", { signal });

export const registerRunnerNode = (input: RegisterRunnerNodeInput) =>
  runnerRequest("/api/v1/admin/runners", parseRunner, "register runner node", {
    method: "POST",
    body: input,
  });

export const updateRunnerHeartbeat = (runnerId: string, status: LiveRunnerStatus) =>
  runnerRequest(
    `/api/v1/admin/runners/${encodeURIComponent(runnerId)}/heartbeat`,
    parseRunner,
    "update runner heartbeat",
    { method: "POST", body: { status } },
  );

export const deleteRunnerNode = (runnerId: string) =>
  runnerRequest(
    `/api/v1/admin/runners/${encodeURIComponent(runnerId)}`,
    () => undefined,
    "delete runner node",
    { method: "DELETE" },
  );

export const fetchRepositoryCaches = (repoId: string, signal?: AbortSignal) =>
  runnerRequest(`/api/v1/repos/${encodeURIComponent(repoId)}/caches`, (raw) => {
    if (!isObj(raw) || !Array.isArray(raw.caches)) throw new Error("Invalid repository caches payload");
    return raw.caches.map(parseCacheEntry);
  }, "load repository caches", { signal });

export const evictRepositoryCache = (repoId: string, key: string) =>
  runnerRequest(
    `/api/v1/repos/${encodeURIComponent(repoId)}/caches/${encodeURIComponent(key)}`,
    () => undefined,
    "evict repository cache",
    { method: "DELETE" },
  );
