import { formatErr, getLiveToken } from "./live-repo-context";

export interface ConnectedRepository {
  id: string;
  name: string;
  branch: string;
  headCommit: string | null;
  capability: "workbench" | "browse";
  isDefault: boolean;
  canRename?: boolean;
  canChangeDefaultBranch?: boolean;
  isPrivate?: boolean;
  canChangeVisibility?: boolean;
  canClone?: boolean;
  canPush?: boolean;
  sshPort?: number;
  canInviteToGroup?: boolean;
  canManageGroupMembers?: boolean;
  canRunLocalChecks?: boolean;
  canViewLocalChecksAutomatic?: boolean;
  canSpawnWorktree?: boolean;
  archived?: boolean;
  canAdminister?: boolean;
  description?: string;
}
export const canCloneRepository = (repository: ConnectedRepository) =>
  repository.canClone ??
  (repository.capability === "workbench" && repository.isDefault);
export interface ConnectedRepositoryState {
  repositories: ConnectedRepository[];
  loading: boolean;
  error: string | null;
}
export interface PendingRename {
  key: number;
  id: string;
  name: string;
  restoreName: string;
  current: () => boolean;
  controller: AbortController;
  rollback: () => void;
}
export const EMPTY_REPOSITORY_STATE: ConnectedRepositoryState = {
  repositories: [],
  loading: false,
  error: null,
};
const uuid = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;
const REQUIRED_REPOSITORY_KEYS = [
  "id",
  "name",
  "branch",
  "head_commit",
  "capability",
  "is_default",
] as const;
const ALLOWED_REPOSITORY_KEYS = new Set<string>([
  ...REQUIRED_REPOSITORY_KEYS,
  "can_rename",
  "can_change_default_branch",
  "is_private",
  "can_change_visibility",
  "can_clone",
  "can_push",
  "ssh_port",
  "can_invite_to_group",
  "can_manage_group_members",
  "can_run_local_checks",
  "can_view_local_checks_automatic",
  "can_spawn_worktree",
  "archived",
  "can_administer",
  "description",
]);
export const invalidRepositoryMetadata = () =>
  new Error("Server returned invalid repository metadata");
const parseRepositories = (value: unknown): ConnectedRepository[] => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw invalidRepositoryMetadata();
  const envelope = value as Record<string, unknown>;
  if (
    Object.keys(envelope).length !== 1 ||
    !Array.isArray(envelope.repositories) ||
    envelope.repositories.length > 32
  )
    throw invalidRepositoryMetadata();
  const ids = new Set<string>();
  let workbenches = 0;
  return envelope.repositories.map((value: unknown) => {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw invalidRepositoryMetadata();
    const raw = value as Record<string, unknown>;
    if (
      !REQUIRED_REPOSITORY_KEYS.every((k) => Object.hasOwn(raw, k)) ||
      !Object.keys(raw).every((k) => ALLOWED_REPOSITORY_KEYS.has(k)) ||
      (Object.hasOwn(raw, "can_rename") &&
        typeof raw.can_rename !== "boolean") ||
      (Object.hasOwn(raw, "can_change_default_branch") &&
        typeof raw.can_change_default_branch !== "boolean") ||
      (Object.hasOwn(raw, "is_private") &&
        typeof raw.is_private !== "boolean") ||
      (Object.hasOwn(raw, "can_change_visibility") &&
        typeof raw.can_change_visibility !== "boolean") ||
      (Object.hasOwn(raw, "can_clone") && typeof raw.can_clone !== "boolean") ||
      (Object.hasOwn(raw, "can_push") && typeof raw.can_push !== "boolean") ||
      (Object.hasOwn(raw, "ssh_port") &&
        (typeof raw.ssh_port !== "number" ||
          !Number.isInteger(raw.ssh_port) ||
          raw.ssh_port < 1 ||
          raw.ssh_port > 65535)) ||
      (Object.hasOwn(raw, "can_view_local_checks_automatic") &&
        typeof raw.can_view_local_checks_automatic !== "boolean") ||
      (Object.hasOwn(raw, "can_run_local_checks") &&
        typeof raw.can_run_local_checks !== "boolean") ||
      (Object.hasOwn(raw, "can_manage_group_members") &&
        typeof raw.can_manage_group_members !== "boolean") ||
      (Object.hasOwn(raw, "can_invite_to_group") &&
        typeof raw.can_invite_to_group !== "boolean") ||
      (Object.hasOwn(raw, "can_spawn_worktree") &&
        typeof raw.can_spawn_worktree !== "boolean") ||
      (Object.hasOwn(raw, "archived") &&
        typeof raw.archived !== "boolean") ||
      (Object.hasOwn(raw, "can_administer") &&
        typeof raw.can_administer !== "boolean") ||
      (Object.hasOwn(raw, "description") &&
        raw.description !== null &&
        (typeof raw.description !== "string" ||
          raw.description.length > 2048)) ||
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
      (raw.head_commit !== null &&
        (typeof raw.head_commit !== "string" ||
          !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(raw.head_commit) ||
          /^0+$/.test(raw.head_commit))) ||
      (raw.capability !== "workbench" && raw.capability !== "browse") ||
      typeof raw.is_default !== "boolean" ||
      (raw.capability === "workbench") !== raw.is_default
    )
      throw invalidRepositoryMetadata();
    const id = raw.id.toLowerCase();
    if (ids.has(id) || (raw.is_default && ++workbenches > 1))
      throw invalidRepositoryMetadata();
    ids.add(id);
    return {
      id,
      name: raw.name,
      branch: raw.branch,
      headCommit:
        raw.head_commit === null ? null : raw.head_commit.toLowerCase(),
      capability: raw.capability,
      isDefault: raw.is_default,
      canRename: raw.can_rename === true,
      canChangeDefaultBranch: raw.can_change_default_branch === true,
      isPrivate:
        typeof raw.is_private === "boolean" ? raw.is_private : undefined,
      canChangeVisibility: raw.can_change_visibility === true,
      canClone: typeof raw.can_clone === "boolean" ? raw.can_clone : undefined,
      canPush: raw.can_push === true,
      sshPort: typeof raw.ssh_port === "number" ? raw.ssh_port : undefined,
      canInviteToGroup: raw.can_invite_to_group === true,
      canManageGroupMembers: raw.can_manage_group_members === true,
      canRunLocalChecks: raw.can_run_local_checks === true,
      canViewLocalChecksAutomatic: raw.can_view_local_checks_automatic === true,
      canSpawnWorktree:
        typeof raw.can_spawn_worktree === "boolean"
          ? raw.can_spawn_worktree
          : undefined,
      archived: typeof raw.archived === "boolean" ? raw.archived : undefined,
      canAdminister:
        typeof raw.can_administer === "boolean"
          ? raw.can_administer
          : undefined,
      description:
        typeof raw.description === "string" ? raw.description : undefined,
    };
  });
};

export const createConnectedRepositorySession = (
  publish: (state: ConnectedRepositoryState) => void,
  deps?: {
    getToken?: () => string | null;
    formatError?: (err: unknown) => string;
  },
) => {
  const readToken = deps?.getToken ?? getLiveToken;
  const formatError = deps?.formatError ?? formatErr;
  let epoch = 0;
  let requestId = 0;
  let session: string | null = null;
  let abort: AbortController | undefined;
  let state = EMPTY_REPOSITORY_STATE;
  let pendingRename: PendingRename | undefined;
  const update = (next: ConnectedRepositoryState) => {
    state = next;
    publish(next);
  };
  const replaceName = (id: string, name: string) =>
    update({
      ...state,
      repositories: state.repositories.map((row) =>
        row.id === id ? { ...row, name } : row,
      ),
    });
  const cancelRename = (key?: number) => {
    if (!pendingRename || (key !== undefined && pendingRename.key !== key))
      return;
    const own = pendingRename;
    pendingRename = undefined;
    own.controller.abort();
    own.rollback();
  };
  const invalidate = () => {
    cancelRename();
    epoch++;
    requestId++;
    abort?.abort();
    update({ ...state, loading: false });
  };
  const reset = () => {
    invalidate();
    session = null;
    update({ ...EMPTY_REPOSITORY_STATE });
  };
  const refreshIfCurrent = async (accept: () => boolean) => {
    if (!accept()) return;
    const token = readToken();
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
    const ownsResponse = () =>
      ownEpoch === epoch &&
      ownRequest === requestId &&
      token === readToken();
    const current = () => ownsResponse() && accept();
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
        throw invalidRepositoryMetadata();
      const repositories = parseRepositories(JSON.parse(text)).map((row) => {
        const pending = pendingRename;
        if (pending?.current() && row.id === pending.id && row.canRename) {
          pending.restoreName = row.name;
          return { ...row, name: pending.name };
        }
        return row;
      });
      if (!current()) return;
      update({ repositories, loading: false, error: null });
    } catch (error) {
      if (current())
        update({
          repositories: [],
          loading: false,
          error: formatError(error),
        });
    } finally {
      if (ownsResponse() && !accept()) update({ ...state, loading: false });
    }
  };
  return {
    refresh: () => refreshIfCurrent(() => true),
    refreshIfCurrent,
    invalidate,
    reset,
    cancelRename,
    confirmVisibility: (id: string, isPrivate: boolean) => {
      invalidate();
      update({
        ...state,
        repositories: state.repositories.map((row) =>
          row.id === id ? { ...row, isPrivate } : row,
        ),
      });
    },
    confirmDefaultBranch: (id: string, branch: string, headCommit: string) => {
      invalidate();
      update({
        ...state,
        repositories: state.repositories.map((row) =>
          row.id === id ? { ...row, branch, headCommit } : row,
        ),
      });
    },
    isCurrentSession: () => session === readToken(),
    readToken,
    getState: () => state,
    getEpoch: () => epoch,
    getSession: () => session,
    getPendingRename: () => pendingRename,
    setPendingRename: (next: PendingRename | undefined) => {
      pendingRename = next;
    },
    update,
    replaceName,
  };
};

export type ConnectedRepositorySession = ReturnType<
  typeof createConnectedRepositorySession
>;
