import { formatErr, getLiveToken } from "./live-repo-client";
import {
  validRepositoryName,
  type RepositoryRenameIntent,
  type RepositoryRenameResult,
} from "./repository-rename-types";
import {
  createConnectedRepositorySession,
  EMPTY_REPOSITORY_STATE,
  invalidRepositoryMetadata,
  type ConnectedRepository,
  type ConnectedRepositorySession,
  type ConnectedRepositoryState,
  type PendingRename,
} from "./connected-repository-session";

export {
  canCloneRepository,
  type ConnectedRepository,
  type ConnectedRepositoryState,
} from "./connected-repository-session";

export const executeConnectedRepositoryRename = async (
  sessionClient: ConnectedRepositorySession,
  intent: RepositoryRenameIntent,
  rawName: string,
  isCurrent: () => boolean,
): Promise<RepositoryRenameResult> => {
  const readToken = sessionClient.readToken;
  const stale = (): RepositoryRenameResult => {
    if (
      sessionClient.getSession() === intent.token &&
      readToken() !== intent.token
    )
      sessionClient.reset();
    return { kind: "stale" };
  };
  const name = rawName.trim();
  const original = sessionClient
    .getState()
    .repositories.find((row) => row.id === intent.repoId);
  if (
    !isCurrent() ||
    intent.token !== readToken() ||
    sessionClient.getSession() !== intent.token ||
    !original?.canRename
  )
    return stale();
  if (sessionClient.getPendingRename())
    return { kind: "error", error: "A rename is already in progress." };
  if (!validRepositoryName(name))
    return {
      kind: "error",
      error:
        "Use 1–100 letters, numbers, dots, underscores or hyphens; avoid edge or repeated dots.",
    };
  sessionClient.invalidate();
  const ownEpoch = sessionClient.getEpoch();
  const controller = new AbortController();
  const sessionCurrent = () =>
    ownEpoch === sessionClient.getEpoch() &&
    sessionClient.getSession() === intent.token &&
    readToken() === intent.token;
  const own: PendingRename = {
    key: intent.key,
    id: intent.repoId,
    name,
    restoreName: original.name,
    controller,
    current: () =>
      sessionClient.getPendingRename() === own &&
      sessionCurrent() &&
      isCurrent(),
    rollback: () => {
      const row = sessionClient
        .getState()
        .repositories.find((entry) => entry.id === intent.repoId);
      if (sessionCurrent() && row?.canRename && row.name === name)
        sessionClient.replaceName(intent.repoId, own.restoreName);
    },
  };
  sessionClient.setPendingRename(own);
  sessionClient.replaceName(intent.repoId, name);
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
        sessionClient.setPendingRename(undefined);
        sessionClient.update({
          ...EMPTY_REPOSITORY_STATE,
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
      throw invalidRepositoryMetadata();
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw invalidRepositoryMetadata();
    const result = value as Record<string, unknown>;
    if (
      Object.keys(result).length !== 2 ||
      typeof result.id !== "string" ||
      result.id.toLowerCase() !== intent.repoId ||
      typeof result.name !== "string" ||
      !validRepositoryName(result.name) ||
      result.name !== result.name.trim()
    )
      throw invalidRepositoryMetadata();
    if (!own.current()) return stale();
    sessionClient.setPendingRename(undefined);
    sessionClient.replaceName(intent.repoId, result.name);
    return { kind: "saved" };
  } catch {
    if (!own.current()) return stale();
    return {
      kind: "error",
      error: "Could not confirm the name change. Try again.",
    };
  } finally {
    if (sessionClient.getPendingRename() === own)
      sessionClient.cancelRename(intent.key);
  }
};

/** A bounded hosted catalog; metadata-only ACL records are not workspaces. */
export const createConnectedRepositoryClient = (
  publish: (state: ConnectedRepositoryState) => void,
) => {
  const sessionClient = createConnectedRepositorySession(publish, {
    getToken: getLiveToken,
    formatError: formatErr,
  });
  return {
    ...sessionClient,
    rename: (
      intent: RepositoryRenameIntent,
      rawName: string,
      isCurrent: () => boolean,
    ) =>
      executeConnectedRepositoryRename(
        sessionClient,
        intent,
        rawName,
        isCurrent,
      ),
  };
};
