import { getLiveToken } from "./live-repo-context";
import { isUuid } from "./live-automatic-runs";
import {
  invalidLocalChecks,
  parseLocalChecks,
  type LocalChecksRecord,
} from "./live-local-checks";

export interface LocalChecksState {
  status: "idle" | "loading" | "ready" | "error";
  records: LocalChecksRecord[];
  error?: string;
}

export async function readLocalChecksJson(
  response: Response,
): Promise<unknown> {
  const limit = 3 * 1024 * 1024;
  if (Number(response.headers.get("content-length")) > limit)
    return invalidLocalChecks();
  const reader = response.body?.getReader();
  if (!reader) return invalidLocalChecks();
  let size = 0;
  let text = "";
  const decoder = new TextDecoder("utf-8", { fatal: true });
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) return invalidLocalChecks();
      text += decoder.decode(value, { stream: true });
    }
    return JSON.parse(text + decoder.decode()) as unknown;
  } finally {
    await reader.cancel();
  }
}

export function createLocalChecksClient(
  publish: (state: LocalChecksState) => void,
  context: {
    repositoryId: () => string | undefined;
    selectionEpoch: () => number;
    canRead: () => boolean;
  },
) {
  let generation = 0;
  let controller: AbortController | undefined;
  let isCurrent = () => false;
  const clear = () => {
    generation++;
    controller?.abort();
    controller = undefined;
    isCurrent = () => false;
    publish({ status: "idle", records: [] });
  };
  const load = async () => {
    clear();
    const request = generation;
    const token = getLiveToken();
    const repositoryId = context.repositoryId();
    const epoch = context.selectionEpoch();
    if (!token || !isUuid(repositoryId) || !context.canRead()) {
      publish({
        status: "error",
        records: [],
        error:
          "Select the primary repository as a SiteAdmin to view Local Checks.",
      });
      return;
    }
    const pending = new AbortController();
    controller = pending;
    const current = () => {
      if (request !== generation) return false;
      if (
        token !== getLiveToken() ||
        context.repositoryId() !== repositoryId ||
        epoch !== context.selectionEpoch() ||
        !context.canRead()
      ) {
        clear();
        return false;
      }
      return true;
    };
    isCurrent = current;
    publish({ status: "loading", records: [] });
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(repositoryId)}/local-check-runs`,
        {
          headers: { Authorization: `Bearer ${token}` },
          cache: "no-store",
          signal: pending.signal,
        },
      );
      if (!current()) return;
      if (!response.ok) {
        pending.abort();
        publish({
          status: "error",
          records: [],
          error: `Unable to load Local Checks (HTTP ${response.status}). Check your session and repository access.`,
        });
        return;
      }
      const raw = await readLocalChecksJson(response);
      if (!current()) return;
      const records = parseLocalChecks(raw, repositoryId);
      if (current()) publish({ status: "ready", records });
    } catch {
      if (current()) {
        pending.abort();
        publish({
          status: "error",
          records: [],
          error: "Unable to load recorded Local Checks. Refresh to try again.",
        });
      }
    }
  };
  return { load, clear, isCurrent: () => isCurrent() };
}
