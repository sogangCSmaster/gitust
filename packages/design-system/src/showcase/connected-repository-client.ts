import { formatErr, getLiveToken } from "./live-repo-client";

export interface ConnectedRepository {
  id: string;
  name: string;
  branch: string;
  headCommit: string;
}
export interface ConnectedRepositoryState {
  repository: ConnectedRepository | null;
  loading: boolean;
  error: string | null;
}
const EMPTY: ConnectedRepositoryState = {
  repository: null,
  loading: false,
  error: null,
};
const parseRepository = (value: unknown): ConnectedRepository => {
  const raw = value as Record<string, unknown> | null;
  if (
    !raw ||
    typeof raw !== "object" ||
    typeof raw.id !== "string" ||
    !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(raw.id) ||
    typeof raw.name !== "string" ||
    !raw.name.trim() ||
    raw.name.length > 1024 ||
    typeof raw.branch !== "string" ||
    !raw.branch.trim() ||
    raw.branch.length > 1024 ||
    typeof raw.head_commit !== "string" ||
    !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(raw.head_commit)
  )
    throw new Error("Server returned invalid repository metadata");
  return {
    id: raw.id,
    name: raw.name,
    branch: raw.branch,
    headCommit: raw.head_commit,
  };
};

/** One configured repository, scoped to the current session and navigation epoch. */
export const createConnectedRepositoryClient = (
  publish: (state: ConnectedRepositoryState) => void,
) => {
  let epoch = 0;
  let requestId = 0;
  let state = EMPTY;
  const update = (next: ConnectedRepositoryState) => {
    state = next;
    publish(next);
  };
  const invalidate = () => {
    epoch++;
    requestId++;
    update({ ...state, loading: false });
  };
  const reset = () => {
    invalidate();
    update({ ...EMPTY });
  };
  const refresh = async () => {
    const token = getLiveToken();
    if (!token) {
      reset();
      return;
    }
    const ownEpoch = epoch;
    const ownRequest = ++requestId;
    const current = () =>
      ownEpoch === epoch &&
      ownRequest === requestId &&
      token === getLiveToken();
    update({ ...state, loading: true, error: null });
    try {
      const response = await fetch("/api/v1/repo", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!current()) return;
      if (!response.ok)
        throw new Error(`Unable to load repository (HTTP ${response.status})`);
      const value: unknown = await response.json();
      if (!current()) return;
      update({
        repository: parseRepository(value),
        loading: false,
        error: null,
      });
    } catch (error) {
      if (current())
        update({ repository: null, loading: false, error: formatErr(error) });
    }
  };
  return { refresh, invalidate, reset };
};
