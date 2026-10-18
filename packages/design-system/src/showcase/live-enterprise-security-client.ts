import { getLiveToken } from "./live-repo-context";

export interface LiveTotpStatus {
  enrolled: boolean;
  verified: boolean;
  enrolled_at?: string | null;
  last_used_step?: number | null;
}

export interface LiveTotpEnrollmentChallenge {
  secret_base32: string;
  otpauth_uri: string;
  digits: number;
  period_secs: number;
}

export type LiveSsoProtocol = "oidc" | "ldap";

export interface LiveSsoProviderRecord {
  id: string;
  name: string;
  protocol: LiveSsoProtocol;
  issuer_or_url: string;
  client_id: string;
  allowed_domains: string[];
  enabled: boolean;
  require_invite_binding: boolean;
  created_at: string;
}

export interface LiveAuditEventRecord {
  seq: number;
  timestamp: string;
  actor_user_id?: string | null;
  actor_username: string;
  action: string;
  target: string;
  prev_hash: string;
  entry_hash: string;
}

export interface LiveAuditChainVerificationReport {
  verified: boolean;
  entries_checked: number;
  head_hash: string;
  first_invalid_seq?: number | null;
  reason?: string | null;
}

export interface LiveAuditLogResponse {
  entries: LiveAuditEventRecord[];
  verification: LiveAuditChainVerificationReport;
}

const getCsrfHeader = (token: string): string => {
  try {
    if (typeof window !== "undefined") {
      const s =
        window.localStorage.getItem("gitust_csrf_token") ??
        window.localStorage.getItem("gitust_csrf");
      if (s && s.trim()) return s.trim();
    }
  } catch {
    // Ignore storage read errors
  }
  return `csrf-${token.slice(0, 16)}`;
};

const isObj = (v: unknown): v is Record<string, unknown> =>
  Boolean(v && typeof v === "object" && !Array.isArray(v));
const isStr = (v: unknown): v is string => typeof v === "string";
const isProtocol = (v: unknown): v is LiveSsoProtocol =>
  v === "oidc" || v === "ldap";

const parseTotpStatus = (raw: unknown): LiveTotpStatus => {
  if (
    !isObj(raw) ||
    typeof raw.enrolled !== "boolean" ||
    typeof raw.verified !== "boolean"
  ) {
    throw new Error("Invalid TOTP status payload");
  }
  return {
    enrolled: raw.enrolled,
    verified: raw.verified,
    enrolled_at: isStr(raw.enrolled_at) ? raw.enrolled_at : null,
    last_used_step:
      typeof raw.last_used_step === "number" ? raw.last_used_step : null,
  };
};

const parseTotpChallenge = (raw: unknown): LiveTotpEnrollmentChallenge => {
  if (
    !isObj(raw) ||
    !isStr(raw.secret_base32) ||
    !isStr(raw.otpauth_uri) ||
    typeof raw.digits !== "number" ||
    typeof raw.period_secs !== "number"
  ) {
    throw new Error("Invalid TOTP enrollment challenge payload");
  }
  return {
    secret_base32: raw.secret_base32,
    otpauth_uri: raw.otpauth_uri,
    digits: raw.digits,
    period_secs: raw.period_secs,
  };
};

const parseSsoProvider = (raw: unknown): LiveSsoProviderRecord => {
  if (
    !isObj(raw) ||
    !isStr(raw.id) ||
    !isStr(raw.name) ||
    !isProtocol(raw.protocol) ||
    !isStr(raw.issuer_or_url) ||
    !isStr(raw.client_id) ||
    !Array.isArray(raw.allowed_domains) ||
    typeof raw.enabled !== "boolean" ||
    typeof raw.require_invite_binding !== "boolean" ||
    !isStr(raw.created_at)
  ) {
    throw new Error("Invalid SSO provider record");
  }
  return {
    id: raw.id,
    name: raw.name,
    protocol: raw.protocol,
    issuer_or_url: raw.issuer_or_url,
    client_id: raw.client_id,
    allowed_domains: raw.allowed_domains.filter(isStr),
    enabled: raw.enabled,
    require_invite_binding: raw.require_invite_binding,
    created_at: raw.created_at,
  };
};

const parseAuditEvent = (raw: unknown): LiveAuditEventRecord => {
  if (
    !isObj(raw) ||
    typeof raw.seq !== "number" ||
    !isStr(raw.timestamp) ||
    !isStr(raw.actor_username) ||
    !isStr(raw.action) ||
    !isStr(raw.target) ||
    !isStr(raw.prev_hash) ||
    !isStr(raw.entry_hash)
  ) {
    throw new Error("Invalid audit event record");
  }
  return {
    seq: raw.seq,
    timestamp: raw.timestamp,
    actor_user_id: isStr(raw.actor_user_id) ? raw.actor_user_id : null,
    actor_username: raw.actor_username,
    action: raw.action,
    target: raw.target,
    prev_hash: raw.prev_hash,
    entry_hash: raw.entry_hash,
  };
};

const parseAuditLog = (raw: unknown): LiveAuditLogResponse => {
  if (
    !isObj(raw) ||
    !Array.isArray(raw.entries) ||
    !isObj(raw.verification) ||
    typeof raw.verification.verified !== "boolean" ||
    typeof raw.verification.entries_checked !== "number" ||
    !isStr(raw.verification.head_hash)
  ) {
    throw new Error("Invalid audit log payload");
  }
  return {
    entries: raw.entries.map(parseAuditEvent),
    verification: {
      verified: raw.verification.verified,
      entries_checked: raw.verification.entries_checked,
      head_hash: raw.verification.head_hash,
      first_invalid_seq:
        typeof raw.verification.first_invalid_seq === "number"
          ? raw.verification.first_invalid_seq
          : null,
      reason: isStr(raw.verification.reason) ? raw.verification.reason : null,
    },
  };
};

async function securityRequest<T>(
  path: string,
  parse: (v: unknown) => T,
  label: string,
  init?: { method?: string; body?: unknown; signal?: AbortSignal },
): Promise<T> {
  const token = getLiveToken();
  if (!token) {
    throw new Error("Sign in to manage enterprise security and audit settings");
  }
  const method = init?.method ?? "GET";
  const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
  if (method !== "GET") {
    headers["Content-Type"] = "application/json";
    headers["X-CSRF-Token"] = getCsrfHeader(token);
  }
  const res = await fetch(path, {
    method,
    headers,
    body: init?.body !== undefined ? JSON.stringify(init.body) : undefined,
    cache: method === "GET" ? "no-store" : undefined,
    signal: init?.signal,
  });
  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(
      `Unable to ${label} (HTTP ${res.status}${detail ? `: ${detail}` : ""})`,
    );
  }
  return res.status === 204 ? parse(undefined) : parse(await res.json());
}

export const fetchTotpStatus = (signal?: AbortSignal) =>
  securityRequest(
    "/api/v1/auth/totp/status",
    parseTotpStatus,
    "load TOTP 2FA status",
    { signal },
  );

export const enrollTotp = () =>
  securityRequest(
    "/api/v1/auth/totp/enroll",
    parseTotpChallenge,
    "enroll TOTP 2FA",
    { method: "POST", body: {} },
  );

export const verifyTotpEnrollment = (code: string) =>
  securityRequest(
    "/api/v1/auth/totp/verify",
    parseTotpStatus,
    "verify TOTP 2FA code",
    { method: "POST", body: { code } },
  );

export const disableTotp = (code?: string) =>
  securityRequest(
    "/api/v1/auth/totp",
    parseTotpStatus,
    "disable TOTP 2FA",
    { method: "DELETE", body: { code } },
  );

export const fetchSsoProviders = (signal?: AbortSignal) =>
  securityRequest(
    "/api/v1/admin/sso/providers",
    (raw) => {
      if (!isObj(raw) || !Array.isArray(raw.providers)) {
        throw new Error("Invalid SSO providers payload");
      }
      return raw.providers.map(parseSsoProvider);
    },
    "load SSO providers",
    { signal },
  );

export const registerSsoProvider = (input: {
  name: string;
  protocol: LiveSsoProtocol;
  issuer_or_url: string;
  client_id: string;
  allowed_domains: string[];
  signing_secret: string;
}) =>
  securityRequest(
    "/api/v1/admin/sso/providers",
    parseSsoProvider,
    "register SSO provider",
    { method: "POST", body: input },
  );

export const deleteSsoProvider = (id: string) =>
  securityRequest(
    `/api/v1/admin/sso/providers/${encodeURIComponent(id)}`,
    () => undefined,
    "delete SSO provider",
    { method: "DELETE" },
  );

export const fetchAuditLog = (signal?: AbortSignal) =>
  securityRequest(
    "/api/v1/admin/audit-log",
    parseAuditLog,
    "load audit log",
    { signal },
  );
