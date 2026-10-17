import { getLiveToken } from "./live-repo-context";
import type {
  VisibilityActions,
  VisibilityIntent,
} from "./repository-visibility";

export interface GroupMember {
  repositoryId: string;
  ownerTeamId: string;
  userId: string;
  username: string;
  role: "Developer" | "Maintainer" | "SiteAdmin";
  status: "active" | "suspended";
  isMember: boolean;
  version: string;
}
type Result =
  | { kind: "member"; member: GroupMember }
  | { kind: "stale" }
  | { kind: "error"; error: string };
const invalid = () => new Error("invalid membership response");
const uuid = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(value) &&
  !/^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(value);
export const normalizeMemberUsername = (value: string) =>
  value.trim().replace(/[A-Z]/g, (letter) => letter.toLowerCase());
export const validMemberUsername = (value: string) =>
  /^[a-z0-9_-]{2,39}$/i.test(value);
const version = (value: unknown): value is string =>
  typeof value === "string" &&
  /^(?:0|[1-9][0-9]{0,19})$/.test(value) &&
  BigInt(value) <= 18446744073709551615n;
const parse = (
  value: unknown,
  intent: VisibilityIntent,
  username: string,
): GroupMember => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw invalid();
  const row = value as Record<string, unknown>;
  if (
    Object.keys(row).sort().join() !==
      "is_member,membership_version,owner_team_id,repository_id,role,status,user_id,username" ||
    !uuid(row.repository_id) ||
    row.repository_id.toLowerCase() !== intent.repoId ||
    !uuid(row.owner_team_id) ||
    !uuid(row.user_id) ||
    typeof row.username !== "string" ||
    !validMemberUsername(row.username) ||
    normalizeMemberUsername(row.username) !== username ||
    typeof row.role !== "string" ||
    !["Developer", "Maintainer", "SiteAdmin"].includes(row.role) ||
    typeof row.status !== "string" ||
    !["active", "suspended"].includes(row.status) ||
    typeof row.is_member !== "boolean" ||
    !version(row.membership_version)
  )
    throw invalid();
  return {
    repositoryId: intent.repoId,
    ownerTeamId: row.owner_team_id.toLowerCase(),
    userId: row.user_id.toLowerCase(),
    username: row.username,
    role: row.role as GroupMember["role"],
    status: row.status as GroupMember["status"],
    isMember: row.is_member,
    version: row.membership_version,
  };
};
async function read(response: Response, current: () => boolean) {
  const reader = response.body?.getReader();
  if (!reader) {
    const text = await response.text();
    if (!current() || new TextEncoder().encode(text).length > 4096)
      throw invalid();
    return text;
  }
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let size = 0,
    text = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (!current()) throw invalid();
      if (done) break;
      size += value.byteLength;
      if (size > 4096) throw invalid();
      text += decoder.decode(value, { stream: true });
    }
    return text + decoder.decode();
  } finally {
    await reader.cancel();
  }
}
export const createGroupMemberClient = (actions: VisibilityActions) => {
  let pending: AbortController | undefined;
  const cancel = () => {
    pending?.abort();
    pending = undefined;
  };
  const request = async (
    intent: VisibilityIntent,
    username: string,
    origin: () => boolean,
    previous?: GroupMember,
    desired?: boolean,
  ): Promise<Result> => {
    const allowed = () =>
      origin() &&
      actions.isCurrent(intent) &&
      actions.canManageMembers?.(intent) === true &&
      intent.token === getLiveToken();
    if (!allowed()) return { kind: "stale" };
    if (pending || !validMemberUsername(username))
      return {
        kind: "error",
        error: "Enter an existing username (2–39 letters, numbers, - or _).",
      };
    const own = new AbortController();
    pending = own;
    const current = () => pending === own && allowed();
    const writing = previous !== undefined;
    const path = `/api/v1/repos/${encodeURIComponent(intent.repoId)}/group-members`;
    try {
      const response = await fetch(
        writing
          ? `${path}/${encodeURIComponent(previous.userId)}`
          : `${path}?${new URLSearchParams({ username })}`,
        {
          cache: "no-store",
          signal: own.signal,
          headers: {
            Authorization: `Bearer ${intent.token}`,
            ...(writing ? { "Content-Type": "application/json" } : {}),
          },
          ...(writing
            ? {
                method: "PATCH",
                body: JSON.stringify({
                  expected_membership_version: previous.version,
                  is_member: desired,
                }),
              }
            : {}),
        },
      );
      if (!current()) return { kind: "stale" };
      if (response.status === 401) {
        actions.unavailable(intent);
        return { kind: "stale" };
      }
      const text = await read(response, current);
      if (!current()) return { kind: "stale" };
      let raw: unknown;
      try {
        raw = JSON.parse(text);
      } catch {
        if (response.status === 403) {
          actions.unavailable(intent);
          return { kind: "stale" };
        }
        throw invalid();
      }
      if (response.status === 200) {
        const member = parse(raw, intent, username);
        if (
          previous &&
          (member.userId !== previous.userId ||
            member.ownerTeamId !== previous.ownerTeamId ||
            member.username !== previous.username ||
            member.isMember !== desired ||
            BigInt(member.version) !==
              BigInt(previous.version) +
                (desired === previous.isMember ? 0n : 1n))
        )
          throw invalid();
        return current() ? { kind: "member", member } : { kind: "stale" };
      }
      const row =
        raw && typeof raw === "object" && !Array.isArray(raw)
          ? (raw as Record<string, unknown>)
          : {};
      const code = Object.keys(row).join() === "code" ? row.code : undefined;
      if (response.status === 404 && code === "group_member_not_found")
        return {
          kind: "error",
          error: "Account not found. Check the exact username.",
        };
      if (
        (response.status === 404 && code === "repository_not_hosted") ||
        (response.status === 403 && code !== "group_member_inactive")
      ) {
        actions.unavailable(intent);
        return { kind: "stale" };
      }
      const errors: Record<string, [number, string]> = {
        group_member_inactive: [
          403,
          "This account is inactive. It can be removed from the group but cannot be added.",
        ],
        membership_conflict: [
          409,
          "Membership changed. Find this account again before another change.",
        ],
        membership_limit: [
          409,
          "This account has reached its group limit. Find it again to check current membership.",
        ],
        membership_version_exhausted: [
          409,
          "This account's membership version cannot advance. Contact the operator.",
        ],
        membership_not_published: [
          503,
          "This attempt did not publish a membership change. Find the account again before retrying.",
        ],
        membership_unconfirmed: [
          503,
          "The change could not be confirmed. Find the account again; it may already have applied.",
        ],
      };
      if (
        typeof code === "string" &&
        Object.hasOwn(errors, code) &&
        errors[code]![0] === response.status
      )
        return { kind: "error", error: errors[code]![1] };
      throw invalid();
    } catch {
      return current()
        ? {
            kind: "error",
            error: writing
              ? "The change could not be confirmed. Find the account again; it may already have applied."
              : "Could not look up this account. Try Find again.",
          }
        : { kind: "stale" };
    } finally {
      if (pending === own) cancel();
    }
  };
  return {
    cancel,
    lookup: (
      intent: VisibilityIntent,
      username: string,
      current: () => boolean,
    ) => request(intent, normalizeMemberUsername(username), current),
    save: (
      intent: VisibilityIntent,
      member: GroupMember,
      desired: boolean,
      current: () => boolean,
    ) =>
      request(
        intent,
        normalizeMemberUsername(member.username),
        current,
        member,
        desired,
      ),
  };
};
