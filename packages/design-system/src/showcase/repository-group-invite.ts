import { getLiveToken } from "./live-repo-context";
import type {
  VisibilityActions,
  VisibilityIntent,
} from "./repository-visibility";

export type InviteRole = "Developer" | "Maintainer" | "SiteAdmin";
export interface GroupInviteDraft {
  email: string;
  role: InviteRole;
  id?: string;
  status?: "uncertain" | "issued" | "conflict";
  expiresAt?: string;
}
export type GroupInviteResult =
  | { kind: "stale" }
  | { kind: "issued"; expiresAt: string; path?: string }
  | { kind: "error"; conflict?: boolean; error: string };
const uuid = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(value) &&
  !/^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(value);
const object = (value: unknown) => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("invalid");
  return value as Record<string, unknown>;
};
const exact = (row: Record<string, unknown>, keys: string[]) => {
  if (Object.keys(row).sort().join() !== keys.sort().join())
    throw new Error("invalid");
};
const time = (value: unknown) => {
  if (
    typeof value !== "string" ||
    value.length > 40 ||
    !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,9})?(?:Z|\+00:00)$/.test(
      value,
    ) ||
    !Number.isFinite(Date.parse(value))
  )
    throw new Error("invalid");
  return value;
};
export const normalizeInviteEmail = (value: string) =>
  value.trim().replace(/[A-Z]/g, (letter) => letter.toLowerCase());
const parse = (
  value: unknown,
  status: number,
  intent: VisibilityIntent,
  draft: GroupInviteDraft,
): GroupInviteResult => {
  const row = object(value);
  if (status === 503) {
    exact(row, ["code", "invite_id"]);
    if (
      !["invite_not_published", "invite_unconfirmed"].includes(
        String(row.code),
      ) ||
      !uuid(row.invite_id) ||
      row.invite_id.toLowerCase() !== draft.id
    )
      throw new Error("invalid");
    return {
      kind: "error",
      error:
        row.code === "invite_not_published"
          ? "This attempt did not publish a new invitation. Retry the same invitation; an earlier attempt may already have issued it."
          : "Issuance could not be confirmed. Retry the same invitation; a link may already have been issued.",
    };
  }
  if (status === 409 && row.code === "invite_request_conflict") {
    exact(row, ["code"]);
    return {
      kind: "error",
      conflict: true,
      error:
        "This invitation ID has different details. Check invitations in Access before explicitly creating another.",
    };
  }
  const created = status === 201;
  exact(row, [
    "repository_id",
    "owner_team_id",
    "invite",
    created ? "invite_url_path" : "code",
  ]);
  if (
    (!created && (status !== 409 || row.code !== "invite_already_issued")) ||
    !uuid(row.repository_id) ||
    row.repository_id.toLowerCase() !== intent.repoId ||
    !uuid(row.owner_team_id)
  )
    throw new Error("invalid");
  const invite = object(row.invite);
  exact(invite, [
    "id",
    "purpose",
    "target_email",
    "assigned_role",
    "created_by",
    "created_at",
    "expires_at",
    "used_at",
    "revoked_at",
    "team_ids",
    "repository_id",
  ]);
  if (
    !uuid(invite.id) ||
    invite.id.toLowerCase() !== draft.id ||
    invite.purpose !== "account_invite" ||
    invite.target_email !== draft.email ||
    invite.assigned_role !== draft.role ||
    !uuid(invite.created_by) ||
    !uuid(invite.repository_id) ||
    invite.repository_id.toLowerCase() !== intent.repoId ||
    !Array.isArray(invite.team_ids) ||
    invite.team_ids.length !== 1 ||
    !uuid(invite.team_ids[0]) ||
    invite.team_ids[0].toLowerCase() !== row.owner_team_id.toLowerCase()
  )
    throw new Error("invalid");
  const createdAt = time(invite.created_at),
    expiresAt = time(invite.expires_at);
  if (Date.parse(expiresAt) - Date.parse(createdAt) !== 72 * 60 * 60 * 1000)
    throw new Error("invalid");
  for (const used of [invite.used_at, invite.revoked_at])
    if (used !== null) time(used);
  if (
    created &&
    (invite.used_at !== null ||
      invite.revoked_at !== null ||
      typeof row.invite_url_path !== "string" ||
      !/^\/auth\/accept-invite\?token=gitust_inv_[0-9a-f]{64}$/.test(
        row.invite_url_path,
      ))
  )
    throw new Error("invalid");
  return {
    kind: "issued",
    expiresAt,
    ...(created ? { path: row.invite_url_path as string } : {}),
  };
};
async function readResponse(response: Response, current: () => boolean) {
  const reader = response.body?.getReader();
  if (!reader) return response.text();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let size = 0,
    text = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (!current()) throw new Error("stale");
      if (done) break;
      size += value.byteLength;
      if (size > 16384) throw new Error("invalid");
      text += decoder.decode(value, { stream: true });
    }
    return text + decoder.decode();
  } finally {
    await reader.cancel();
  }
}
export const createGroupInviteMutation = (actions: VisibilityActions) => {
  let pending: AbortController | undefined;
  const cancel = () => {
    pending?.abort();
    pending = undefined;
  };
  const save = async (
    intent: VisibilityIntent,
    draft: GroupInviteDraft,
  ): Promise<GroupInviteResult> => {
    const allowed = () =>
      actions.isCurrent(intent) &&
      actions.canInvite?.(intent) === true &&
      intent.token === getLiveToken();
    if (!allowed()) return { kind: "stale" };
    if (pending || !uuid(draft.id))
      return { kind: "error", error: "An invitation is already in progress." };
    const own = new AbortController();
    pending = own;
    const current = () => pending === own && allowed();
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(intent.repoId)}/group-invites`,
        {
          method: "POST",
          cache: "no-store",
          signal: own.signal,
          headers: {
            Authorization: `Bearer ${intent.token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            invite_id: draft.id,
            target_email: draft.email,
            assigned_role: draft.role,
          }),
        },
      );
      if (!current()) return { kind: "stale" };
      if ([401, 403, 404].includes(response.status)) {
        actions.unavailable(intent);
        return { kind: "stale" };
      }
      if (![201, 409, 503].includes(response.status))
        throw new Error("unconfirmed");
      const text = await readResponse(response, current);
      if (!current()) return { kind: "stale" };
      if (text.length > 16384 || new TextEncoder().encode(text).length > 16384)
        throw new Error("invalid");
      const result = parse(JSON.parse(text), response.status, intent, draft);
      return current() ? result : { kind: "stale" };
    } catch {
      return current()
        ? {
            kind: "error",
            error:
              "Issuance could not be confirmed. Retry the same invitation; a link may already have been issued.",
          }
        : { kind: "stale" };
    } finally {
      if (pending === own) cancel();
    }
  };
  return { save, cancel };
};
