import { createSignal } from "solid-js";
import { getLiveToken } from "./live-repo-context";
export { parseBranchPage } from "./live-branch-parser";

export interface RepositoryBranch {
  name: string;
  ref_name: string;
  commit_sha: string;
}

export const createRepositoryBranches = (options: {
  getRepositoryId: () => string | null | undefined;
  onSelect: () => void;
  onUnavailable?: (id: string) => void;
}) => {
  const [state, setState] = createSignal({
    branches: [] as RepositoryBranch[],
    next_cursor: null as string | null,
    selected: null as RepositoryBranch | null,
    loading: false,
    error: null as string | null,
    cursor: null as string | null,
  });
  let epoch = 0;
  let request = 0;
  let token: string | null = null;
  let repository: string | null | undefined;
  let initialized = false;
  let controller: AbortController | undefined;
  const reset = () => {
    epoch++;
    request++;
    controller?.abort();
    initialized = false;
    token = null;
    repository = undefined;
    setState({
      branches: [],
      next_cursor: null,
      selected: null,
      loading: false,
      error: null,
      cursor: null,
    });
  };
  const load = async (cursor: string | null = null) => {
    const ownToken = getLiveToken();
    const ownRepo = options.getRepositoryId();
    if (!ownToken || !ownRepo) {
      reset();
      return;
    }
    if (token !== ownToken || repository !== ownRepo) reset();
    token = ownToken;
    repository = ownRepo;
    initialized = true;
    const ownEpoch = epoch,
      ownRequest = ++request;
    controller?.abort();
    controller = new AbortController();
    const ownController = controller;
    const current = () =>
      ownEpoch === epoch &&
      ownRequest === request &&
      ownToken === getLiveToken() &&
      ownRepo === options.getRepositoryId();
    setState((value) => ({ ...value, loading: true, error: null }));
    try {
      const query = new URLSearchParams({ limit: "50" });
      if (cursor) query.set("cursor", cursor);
      const [response, { boundedBranchJson, parseBranchPage }] =
        await Promise.all([
          fetch(
            `/api/v1/repos/${encodeURIComponent(ownRepo)}/branches?${query}`,
            {
              headers: { Authorization: `Bearer ${ownToken}` },
              signal: ownController.signal,
            },
          ),
          import("./live-branch-parser"),
        ]);
      if (!current()) {
        ownController.abort();
        return;
      }
      if ([401, 403, 404].includes(response.status)) {
        setState((value) => ({ ...value, selected: null }));
        options.onUnavailable?.(ownRepo);
      }
      if (!response.ok)
        throw new Error(`Branches unavailable (HTTP ${response.status})`);
      const raw = await boundedBranchJson(response);
      if (!current()) return;
      const page = parseBranchPage(raw);
      if (!current()) return;
      setState((value) => ({
        ...value,
        ...page,
        cursor,
        loading: false,
        error: null,
      }));
    } catch {
      ownController.abort();
      if (!current()) return;
      setState((value) => ({
        ...value,
        branches: [],
        next_cursor: null,
        cursor,
        loading: false,
        error: "Branches are unavailable. Refresh to retry.",
      }));
    }
  };
  const refresh = () => {
    if (
      initialized &&
      (token !== getLiveToken() || repository !== options.getRepositoryId())
    ) {
      reset();
      return Promise.resolve();
    }
    return load();
  };
  return {
    state,
    reset,
    ensure: () => {
      if (
        !initialized ||
        token !== getLiveToken() ||
        repository !== options.getRepositoryId()
      )
        void load();
    },
    refresh,
    next: () => {
      if (
        token !== getLiveToken() ||
        repository !== options.getRepositoryId()
      ) {
        reset();
        return Promise.resolve();
      }
      const cursor = state().next_cursor;
      return cursor ? load(cursor) : Promise.resolve();
    },
    select: (branch: RepositoryBranch | null) => {
      if (
        !getLiveToken() ||
        token !== getLiveToken() ||
        repository !== options.getRepositoryId()
      ) {
        reset();
        return;
      }
      if (
        branch &&
        !state().branches.some(
          (row) =>
            row.ref_name === branch.ref_name &&
            row.commit_sha === branch.commit_sha,
        )
      )
        return;
      const previous = state().selected;
      if (
        previous?.ref_name === branch?.ref_name &&
        previous?.commit_sha === branch?.commit_sha
      )
        return;
      setState((value) => ({ ...value, selected: branch }));
      options.onSelect();
    },
  };
};
export type RepositoryBranches = ReturnType<typeof createRepositoryBranches>;
