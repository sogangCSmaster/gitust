import { formatErr, getLiveToken } from "./live-repo-client";
import {
  validRepositoryName,
  type RepositoryRenameIntent,
  type RepositoryRenameResult,
} from "./repository-rename-types";

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
  canInviteToGroup?: boolean;
  canManageGroupMembers?: boolean;
}
export const canCloneRepository = (repository: ConnectedRepository) =>
  repository.canClone ??
  (repository.capability === "workbench" && repository.isDefault);
export interface ConnectedRepositoryState {
  repositories: ConnectedRepository[];
  loading: boolean;
  error: string | null;
}
interface PendingRename {
  key: number;
  id: string;
  name: string;
  restoreName: string;
  current: () => boolean;
  controller: AbortController;
  rollback: () => void;
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
      Object.keys(raw).length !==
        6 +
          Number(Object.hasOwn(raw, "can_rename")) +
          Number(Object.hasOwn(raw, "can_change_default_branch")) +
          Number(Object.hasOwn(raw, "is_private")) +
          Number(Object.hasOwn(raw, "can_change_visibility")) +
          Number(Object.hasOwn(raw, "can_clone")) +
          Number(Object.hasOwn(raw, "can_invite_to_group")) +
          Number(Object.hasOwn(raw, "can_manage_group_members")) ||
      (Object.hasOwn(raw, "can_rename") &&
        typeof raw.can_rename !== "boolean") ||
      (Object.hasOwn(raw, "can_change_default_branch") &&
        typeof raw.can_change_default_branch !== "boolean") ||
      (Object.hasOwn(raw, "is_private") &&
        typeof raw.is_private !== "boolean") ||
      (Object.hasOwn(raw, "can_change_visibility") &&
        typeof raw.can_change_visibility !== "boolean") ||
      (Object.hasOwn(raw, "can_clone") && typeof raw.can_clone !== "boolean") ||
      (Object.hasOwn(raw, "can_manage_group_members") &&
        typeof raw.can_manage_group_members !== "boolean") ||
      (Object.hasOwn(raw, "can_invite_to_group") &&
        typeof raw.can_invite_to_group !== "boolean") ||
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
      throw invalid();
    const id = raw.id.toLowerCase();
    if (ids.has(id) || (raw.is_default && ++workbenches > 1)) throw invalid();
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
      canInviteToGroup: raw.can_invite_to_group === true,
      canManageGroupMembers: raw.can_manage_group_members === true,
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
    update({ ...EMPTY });
  };
  const refreshIfCurrent = async (accept: () => boolean) => {
    if (!accept()) return;
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
    const ownsResponse = () =>
      ownEpoch === epoch &&
      ownRequest === requestId &&
      token === getLiveToken();
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
        throw invalid();
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
        update({ repositories: [], loading: false, error: formatErr(error) });
    } finally {
      // An expired view intent still releases its loading state, without touching a newer request.
      if (ownsResponse() && !accept()) update({ ...state, loading: false });
    }
  };
  const rename = async (
    intent: RepositoryRenameIntent,
    rawName: string,
    isCurrent: () => boolean,
  ): Promise<RepositoryRenameResult> => {
    const stale = (): RepositoryRenameResult => {
      if (session === intent.token && getLiveToken() !== intent.token) reset();
      return { kind: "stale" };
    };
    const name = rawName.trim();
    const original = state.repositories.find((row) => row.id === intent.repoId);
    if (
      !isCurrent() ||
      intent.token !== getLiveToken() ||
      session !== intent.token ||
      !original?.canRename
    )
      return stale();
    if (pendingRename)
      return { kind: "error", error: "A rename is already in progress." };
    if (!validRepositoryName(name))
      return {
        kind: "error",
        error:
          "Use 1–100 letters, numbers, dots, underscores or hyphens; avoid edge or repeated dots.",
      };
    invalidate();
    const ownEpoch = epoch;
    const controller = new AbortController();
    const sessionCurrent = () =>
      ownEpoch === epoch &&
      session === intent.token &&
      getLiveToken() === intent.token;
    const own: PendingRename = {
      key: intent.key,
      id: intent.repoId,
      name,
      restoreName: original.name,
      controller,
      current: () => pendingRename === own && sessionCurrent() && isCurrent(),
      rollback: () => {
        const row = state.repositories.find(
          (entry) => entry.id === intent.repoId,
        );
        if (sessionCurrent() && row?.canRename && row.name === name)
          replaceName(intent.repoId, own.restoreName);
      },
    };
    pendingRename = own;
    replaceName(intent.repoId, name);
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(intent.repoId)}`,
        {
          method: "PATCH",
          headers: {
            Authorization: `Bearer ${intent.token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ name }),
          signal: controller.signal,
        },
      );
      if (!own.current()) return stale();
      if (response.status !== 200) {
        if ([401, 403, 404].includes(response.status)) {
          pendingRename = undefined;
          update({
            ...EMPTY,
            error: "Repository access changed. Refresh the repository list.",
          });
          return { kind: "stale" };
        }
        throw new Error(
          `Unable to rename repository (HTTP ${response.status})`,
        );
      }
      const text = await response.text();
      if (!own.current()) return stale();
      if (text.length > 4096 || new TextEncoder().encode(text).length > 4096)
        throw invalid();
      const value: unknown = JSON.parse(text);
      if (!value || typeof value !== "object" || Array.isArray(value))
        throw invalid();
      const result = value as Record<string, unknown>;
      if (
        Object.keys(result).length !== 2 ||
        typeof result.id !== "string" ||
        result.id.toLowerCase() !== intent.repoId ||
        typeof result.name !== "string" ||
        !validRepositoryName(result.name) ||
        result.name !== result.name.trim()
      )
        throw invalid();
      if (!own.current()) return stale();
      pendingRename = undefined;
      replaceName(intent.repoId, result.name);
      return { kind: "saved" };
    } catch {
      if (!own.current()) return stale();
      return {
        kind: "error",
        error: "Could not confirm the name change. Try again.",
      };
    } finally {
      if (pendingRename === own) cancelRename(intent.key);
    }
  };
  return {
    refresh: () => refreshIfCurrent(() => true),
    refreshIfCurrent,
    invalidate,
    reset,
    rename,
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
    isCurrentSession: () => session === getLiveToken(),
  };
};
