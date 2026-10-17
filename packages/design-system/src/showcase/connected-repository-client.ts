import { formatErr, getLiveToken } from "./live-repo-client";

export interface ConnectedRepository {
  id: string;
  name: string;
  branch: string;
  headCommit: string;
  capability: "workbench" | "browse";
  isDefault: boolean;
}
export interface ConnectedRepositoryState {
  repositories: ConnectedRepository[];
  loading: boolean;
  error: string | null;
}
const EMPTY: ConnectedRepositoryState = {
  repositories: [],
  loading: false,
  error: null,
};
const uuid = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;
const invalid = () => new Error("Server returned invalid repository metadata");
const parseRepositories = (value: unknown): ConnectedRepository[] => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw invalid();
  const envelope = value as Record<string, unknown>;
  if (
    Object.keys(envelope).length !== 1 ||
    !Array.isArray(envelope.repositories) ||
    envelope.repositories.length > 32
  )
    throw invalid();
  const ids = new Set<string>();
  let workbenches = 0;
  return envelope.repositories.map((value: unknown) => {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw invalid();
    const raw = value as Record<string, unknown>;
    if (
      Object.keys(raw).length !== 6 ||
      typeof raw.id !== "string" ||
      !uuid.test(raw.id) ||
      /^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(raw.id) ||
      typeof raw.name !== "string" ||
      !raw.name.trim() ||
      raw.name.length > 1024 ||
      new TextEncoder().encode(raw.name).length > 1024 ||
      typeof raw.branch !== "string" ||
      !raw.branch.trim() ||
      raw.branch.length > 32768 ||
      new TextEncoder().encode(raw.branch).length > 32768 ||
      typeof raw.head_commit !== "string" ||
      !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(raw.head_commit) ||
      /^0+$/.test(raw.head_commit) ||
      (raw.capability !== "workbench" && raw.capability !== "browse") ||
      typeof raw.is_default !== "boolean" ||
      (raw.capability === "workbench") !== raw.is_default
    )
      throw invalid();
    const id = raw.id.toLowerCase();
    if (ids.has(id) || (raw.is_default && ++workbenches > 1)) throw invalid();
    ids.add(id);
    return {
      id,
      name: raw.name,
      branch: raw.branch,
      headCommit: raw.head_commit.toLowerCase(),
      capability: raw.capability,
      isDefault: raw.is_default,
    };
  });
};

/** A bounded hosted catalog; metadata-only ACL records are not workspaces. */
export const createConnectedRepositoryClient = (
  publish: (state: ConnectedRepositoryState) => void,
) => {
  let epoch = 0;
  let requestId = 0;
  let session: string | null = null;
  let abort: AbortController | undefined;
  let state = EMPTY;
  const update = (next: ConnectedRepositoryState) => {
    state = next;
    publish(next);
  };
  const invalidate = () => {
    epoch++;
    requestId++;
    abort?.abort();
    update({ ...state, loading: false });
  };
  const reset = () => {
    invalidate();
    session = null;
    update({ ...EMPTY });
  };
  const refresh = async () => {
    const token = getLiveToken();
    if (!token) {
      reset();
      return;
    }
    if (session !== token) {
      reset();
      session = token;
    }
    abort?.abort();
    const controller = new AbortController();
    abort = controller;
    const ownEpoch = epoch;
    const ownRequest = ++requestId;
    const current = () =>
      ownEpoch === epoch &&
      ownRequest === requestId &&
      token === getLiveToken();
    update({ ...state, loading: true, error: null });
    try {
      const response = await fetch("/api/v1/repos", {
        headers: { Authorization: `Bearer ${token}` },
        signal: controller.signal,
      });
      if (!current()) return;
      if (!response.ok)
        throw new Error(
          `Unable to load repositories (HTTP ${response.status})`,
        );
      const text = await response.text();
      if (!current()) return;
      if (
        text.length > 4 * 1024 * 1024 ||
        new TextEncoder().encode(text).length > 4 * 1024 * 1024
      )
        throw invalid();
      const repositories = parseRepositories(JSON.parse(text));
      if (!current()) return;
      update({ repositories, loading: false, error: null });
    } catch (error) {
      if (current())
        update({ repositories: [], loading: false, error: formatErr(error) });
    }
  };
  return {
    refresh,
    invalidate,
    reset,
    isCurrentSession: () => session === getLiveToken(),
  };
};
