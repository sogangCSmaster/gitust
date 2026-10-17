import { getLiveToken } from "./live-repo-client";
import { isUuid } from "./live-automatic-runs";
import {
  parseActionsPage,
  type AutomaticActionsPage,
} from "./live-actions-page";
export interface LiveActionsState {
  status: "idle" | "loading" | "ready" | "error";
  page: AutomaticActionsPage | null;
  error?: string;
}

/** One bounded page, scoped to its repository, session and navigation generation. */
export function createLiveActionsClient(
  publish: (state: LiveActionsState) => void,
  context: { repositoryId: () => string | undefined; canRead: () => boolean },
) {
  let generation = 0;
  let controller: AbortController | undefined;
  let isCurrent = () => false;
  const clear = () => {
    generation++;
    controller?.abort();
    controller = undefined;
    isCurrent = () => false;
    publish({ status: "idle", page: null });
  };
  const load = async (repositoryId: string, cursor: string | null = null) => {
    clear();
    const request = generation;
    const token = getLiveToken();
    if (
      !token ||
      !isUuid(repositoryId) ||
      context.repositoryId() !== repositoryId ||
      !context.canRead()
    ) {
      publish({
        status: "error",
        page: null,
        error: "Sign in as a SiteAdmin to view automatic Actions.",
      });
      return;
    }
    const pending = new AbortController();
    controller = pending;
    const current = () => {
      if (generation !== request) return false;
      if (
        token !== getLiveToken() ||
        context.repositoryId() !== repositoryId ||
        !context.canRead()
      ) {
        clear();
        return false;
      }
      return true;
    };
    isCurrent = current;
    publish({ status: "loading", page: null });
    try {
      const query = new URLSearchParams({ limit: "10" });
      if (cursor !== null) query.set("cursor", cursor);
      const response = await fetch(`/api/v1/repo/automatic-runs?${query}`, {
        headers: { Authorization: `Bearer ${token}` },
        cache: "no-store",
        signal: pending.signal,
      });
      if (!current()) return;
      if (!response.ok)
        throw new Error(
          `Unable to load Actions (HTTP ${response.status}). Check your session and repository access.`,
        );
      const body = await response.text();
      if (!current()) return;
      if (new TextEncoder().encode(body).length > 2 * 1024 * 1024)
        throw new Error("Automatic Actions page exceeds the response limit");
      let value: unknown;
      try {
        value = JSON.parse(body);
      } catch {
        throw new Error("Invalid automatic Actions page from server");
      }
      if (!current()) return;
      const page = parseActionsPage(value, repositoryId, cursor);
      if (current()) publish({ status: "ready", page });
    } catch (error) {
      if (current()) {
        pending.abort();
        publish({
          status: "error",
          page: null,
          error:
            error instanceof Error
              ? error.message
              : "Unable to load automatic Actions.",
        });
      }
    }
  };
  return { load, clear, isCurrent: () => isCurrent() };
}
