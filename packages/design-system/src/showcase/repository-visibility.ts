import type { GroupInviteDraft } from "./repository-group-invite";
import { getLiveToken } from "./live-repo-context";

export interface VisibilityIntent {
  repoId: string;
  name: string;
  token: string;
  epoch: number;
  key: number;
  expected: boolean;
}
export type VisibilityResult =
  { kind: "saved" } | { kind: "stale" } | { kind: "error"; error: string };
export interface VisibilityActions {
  begin: () => VisibilityIntent | null;
  canInvite?: (intent: VisibilityIntent, original?: boolean) => boolean;
  inviteDraft?: (intent: VisibilityIntent) => GroupInviteDraft | undefined;
  setInviteDraft?: (intent: VisibilityIntent, draft: GroupInviteDraft) => void;
  isCurrent: (intent: VisibilityIntent) => boolean;
  invalidateReads: () => void;
  confirmed: (intent: VisibilityIntent, value: boolean) => void;
  cancel: (intent: VisibilityIntent) => void;
  refresh: (intent: VisibilityIntent) => Promise<void>;
  unavailable: (intent: VisibilityIntent) => void;
}

export const createVisibilityMutation = (actions: VisibilityActions) => {
  let pending: AbortController | undefined;
  const cancel = () => {
    pending?.abort();
    pending = undefined;
  };
  const save = async (
    intent: VisibilityIntent,
    value: boolean,
  ): Promise<VisibilityResult> => {
    if (!actions.isCurrent(intent) || intent.token !== getLiveToken())
      return { kind: "stale" };
    if (pending)
      return {
        kind: "error",
        error: "An access change is already in progress.",
      };
    actions.invalidateReads();
    const own = new AbortController();
    pending = own;
    const current = () =>
      pending === own &&
      actions.isCurrent(intent) &&
      intent.token === getLiveToken();
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(intent.repoId)}/visibility`,
        {
          method: "PATCH",
          cache: "no-store",
          headers: {
            Authorization: `Bearer ${intent.token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            expected_is_private: intent.expected,
            is_private: value,
          }),
          signal: own.signal,
        },
      );
      if (!current()) return { kind: "stale" };
      if ([401, 403, 404].includes(response.status)) {
        actions.unavailable(intent);
        return { kind: "stale" };
      }
      if (response.status === 409)
        return {
          kind: "error",
          error: "Repository access changed. Refresh before retrying.",
        };
      if (response.status !== 200) throw new Error("unconfirmed");
      const text = await response.text();
      if (!current()) return { kind: "stale" };
      if (text.length > 4096 || new TextEncoder().encode(text).length > 4096)
        throw new Error("invalid response");
      const raw: unknown = JSON.parse(text);
      if (!raw || typeof raw !== "object" || Array.isArray(raw))
        throw new Error("invalid response");
      const row = raw as Record<string, unknown>;
      if (
        Object.keys(row).sort().join() !== "id,is_private" ||
        typeof row.id !== "string" ||
        row.id.toLowerCase() !== intent.repoId ||
        row.is_private !== value
      )
        throw new Error("invalid response");
      if (!current()) return { kind: "stale" };
      pending = undefined;
      actions.confirmed(intent, value);
      return { kind: "saved" };
    } catch {
      if (!current()) return { kind: "stale" };
      return {
        kind: "error",
        error: "Could not confirm the access change. Refresh before retrying.",
      };
    } finally {
      if (pending === own) cancel();
    }
  };
  return { save, cancel };
};
