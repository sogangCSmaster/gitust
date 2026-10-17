import { createSignal } from "solid-js";
import { getLiveToken } from "./live-repo-context";

export interface RepositoryBranch {
  name: string;
  ref_name: string;
  commit_sha: string;
}
interface BranchPage {
  branches: RepositoryBranch[];
  next_cursor: string | null;
}
const bytes = (value: string) => new TextEncoder().encode(value).length;
const invalid = () => new Error("Server returned invalid repository branches");
const ref = (value: unknown): value is string =>
  typeof value === "string" &&
  value.startsWith("refs/heads/") &&
  bytes(value.slice(11)) > 0 &&
  bytes(value.slice(11)) <= 32768 &&
  !/[\x00-\x20\x7f~^:?*\[\\]/u.test(value) &&
  !value.includes("..") &&
  !value.includes("@{") &&
  value
    .split("/")
    .every(
      (part) => part && !part.startsWith(".") && !part.endsWith(".lock"),
    ) &&
  !value.endsWith(".");
const cursorRef = (value: string) => {
  try {
    const encoded = value.slice(3);
    const binary = atob(encoded.replace(/-/g, "+").replace(/_/g, "/"));
    if (
      btoa(binary)
        .replace(/\+/g, "-")
        .replace(/\//g, "_")
        .replace(/=+$/, "") !== encoded
    )
      throw invalid();
    const name = new TextDecoder("utf-8", { fatal: true }).decode(
      Uint8Array.from(binary, (char) => char.charCodeAt(0)),
    );
    if (!ref(name)) throw invalid();
    return name;
  } catch {
    throw invalid();
  }
};
export const parseBranchPage = (raw: unknown): BranchPage => {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw invalid();
  const page = raw as Record<string, unknown>;
  if (
    Object.keys(page).sort().join() !== "branches,next_cursor" ||
    !Array.isArray(page.branches) ||
    page.branches.length > 200 ||
    !(
      page.next_cursor === null ||
      (typeof page.next_cursor === "string" &&
        /^v1\.[A-Za-z0-9_-]+$/.test(page.next_cursor) &&
        page.next_cursor.length <= 43709)
    )
  )
    throw invalid();
  let previous: Uint8Array | undefined;
  const branches = page.branches.map((value): RepositoryBranch => {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw invalid();
    const row = value as Record<string, unknown>;
    if (
      Object.keys(row).sort().join() !== "commit_sha,name,ref_name" ||
      !ref(row.ref_name) ||
      typeof row.name !== "string" ||
      row.ref_name !== `refs/heads/${row.name}` ||
      typeof row.commit_sha !== "string" ||
      !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(row.commit_sha) ||
      /^0+$/.test(row.commit_sha)
    )
      throw invalid();
    const current = new TextEncoder().encode(row.ref_name);
    if (previous) {
      let index = 0;
      while (
        index < Math.min(previous.length, current.length) &&
        previous[index] === current[index]
      )
        index++;
      if (
        index === current.length ||
        (index < previous.length && previous[index]! > current[index]!)
      )
        throw invalid();
    }
    previous = current;
    return {
      name: row.name,
      ref_name: row.ref_name,
      commit_sha: row.commit_sha.toLowerCase(),
    };
  });
  if (
    page.next_cursor !== null &&
    (!branches.length ||
      cursorRef(page.next_cursor as string) !== branches.at(-1)?.ref_name)
  )
    throw invalid();
  return { branches, next_cursor: page.next_cursor as string | null };
};

async function boundedJson(response: Response) {
  const limit = 4 * 1024 * 1024;
  if (Number(response.headers.get("content-length")) > limit) throw invalid();
  const reader = response.body?.getReader();
  if (!reader) throw invalid();
  let size = 0;
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let text = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw invalid();
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return JSON.parse(text) as unknown;
  } finally {
    await reader.cancel();
  }
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
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(ownRepo)}/branches?${query}`,
        {
          headers: { Authorization: `Bearer ${ownToken}` },
          signal: ownController.signal,
        },
      );
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
      const raw = await boundedJson(response);
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
