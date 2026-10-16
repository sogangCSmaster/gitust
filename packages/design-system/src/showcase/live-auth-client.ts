import type {
  EnterpriseUser,
  LiveAuthSessionInfo,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "../index";
import { formatErr, getLiveToken } from "./live-repo-client";

export interface LiveAuthClientCallbacks {
  setLiveSession: (session: LiveAuthSessionInfo | null) => void;
  setEnterpriseUsers: (users: EnterpriseUser[]) => void;
  setLiveInvites: (invites: LiveInviteItem[]) => void;
  setLastIssuedInviteUrl: (url: string | null) => void;
  setLivePats: (pats: LivePatItem[]) => void;
  setLastIssuedPatToken: (token: string | null) => void;
  setLiveSshKeys: (keys: LiveSshKeyItem[]) => void;
  setRedeemToken: (token: string) => void;
  fallbackUsers: EnterpriseUser[];
  onTokenAuthenticated: () => void;
  onTokenCleared: () => void;
  showQuickFeedback: (msg: string) => void;
}

const mapRoleLabel = (
  role: string
): "Site Admin" | "Maintainer" | "Developer" => {
  if (role === "SiteAdmin") return "Site Admin";
  if (role === "Maintainer") return "Maintainer";
  return "Developer";
};

const mapLiveUsers = (
  rawUsers: Array<{
    id: string;
    username: string;
    email: string;
    role: string;
    status: string;
  }>
): EnterpriseUser[] =>
  rawUsers.map((u, idx) => ({
    id: idx + 1,
    liveId: u.id,
    username: u.username,
    email: u.email,
    department: u.role === "SiteAdmin" ? "Platform Security" : "Engineering",
    role: mapRoleLabel(u.role),
    twoFactorEnabled: true,
    status: u.status === "Active" ? "active" : "suspended",
  }));

export function createLiveAuthClient(cb: LiveAuthClientCallbacks) {
  let authEpoch = 0;

  const clearLocalAuthState = () => {
    authEpoch++;
    try {
      if (typeof window !== "undefined") {
        window.localStorage.removeItem("gitust_token");
      }
    } catch {
      // Ignore storage error in restricted environments
    }
    cb.setLiveSession(null);
    cb.setLiveInvites([]);
    cb.setLastIssuedInviteUrl(null);
    cb.setLivePats([]);
    cb.setLastIssuedPatToken(null);
    cb.setLiveSshKeys([]);
    cb.setEnterpriseUsers(cb.fallbackUsers);
    cb.onTokenCleared();
  };

  const hydrateAuthData = async () => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = ++authEpoch;
    const headers = { Authorization: `Bearer ${token}` };

    try {
      const meRes = await fetch("/api/v1/auth/me", { headers });
      if (epoch !== authEpoch) return;
      if (!meRes.ok) {
        if (meRes.status === 401 || meRes.status === 403) {
          clearLocalAuthState();
        }
        return;
      }
      const me = (await meRes.json()) as LiveAuthSessionInfo;
      if (epoch !== authEpoch) return;
      cb.setLiveSession(me);

      const [patsRes, sshRes] = await Promise.all([
        fetch("/api/v1/auth/pats", { headers }),
        fetch("/api/v1/auth/ssh-keys", { headers }),
      ]);
      if (epoch !== authEpoch) return;
      if (patsRes.ok) {
        const pats = await patsRes.json();
        if (epoch === authEpoch && Array.isArray(pats)) {
          cb.setLivePats(pats);
        }
      }
      if (sshRes.ok) {
        const keys = await sshRes.json();
        if (epoch === authEpoch && Array.isArray(keys)) {
          cb.setLiveSshKeys(keys);
        }
      }

      if (me.role === "SiteAdmin") {
        const [usersRes, invitesRes] = await Promise.all([
          fetch("/api/v1/admin/users", { headers }),
          fetch("/api/v1/admin/invites", { headers }),
        ]);
        if (epoch !== authEpoch) return;
        if (usersRes.ok) {
          const rawUsers = await usersRes.json();
          if (epoch === authEpoch && Array.isArray(rawUsers)) {
            cb.setEnterpriseUsers(mapLiveUsers(rawUsers));
          }
        }
        if (invitesRes.ok) {
          const invites = await invitesRes.json();
          if (epoch === authEpoch && Array.isArray(invites)) {
            cb.setLiveInvites(invites);
          }
        }
      } else {
        cb.setLiveInvites([]);
        cb.setEnterpriseUsers(
          mapLiveUsers([
            {
              id: me.user_id,
              username: me.username,
              email: me.email,
              role: me.role,
              status: "Active",
            },
          ])
        );
      }
    } catch (err) {
      if (epoch !== authEpoch) return;
      console.warn("Live auth hydration failed:", formatErr(err));
    }
  };

  const signIn = async (identifier: string, password: string) => {
    if (!identifier || !password) {
      cb.showQuickFeedback("Please enter both username/email and password");
      return;
    }
    const epoch = ++authEpoch;
    try {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ identifier, password }),
      });
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Sign-in failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const data = await res.json();
      if (epoch !== authEpoch) return;
      if (typeof data.raw_session_token === "string") {
        window.localStorage.setItem("gitust_token", data.raw_session_token);
      }
      cb.setLastIssuedInviteUrl(null);
      cb.setLastIssuedPatToken(null);
      cb.showQuickFeedback(
        `Signed in as @${data.user?.username ?? identifier} (${data.user?.role ?? "User"})`
      );
      await hydrateAuthData();
      if (!getLiveToken()) return;
      cb.onTokenAuthenticated();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Sign-in request failed: ${formatErr(err)}`);
    }
  };

  const signOut = async () => {
    const token = getLiveToken();
    clearLocalAuthState();
    if (token) {
      try {
        await fetch("/api/v1/auth/logout", {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        });
      } catch {
        // Server session revocation best-effort after local token scrub
      }
    }
    cb.showQuickFeedback("Signed out and revoked active session token");
  };

  const redeemInvite = async (
    rawToken: string,
    username: string,
    password: string
  ) => {
    if (!rawToken || !username || !password) {
      cb.showQuickFeedback("Invite token, username, and password are required");
      return;
    }
    const epoch = authEpoch;
    try {
      const res = await fetch("/api/v1/auth/invites/redeem", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ raw_token: rawToken, username, password }),
      });
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Invite redemption failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const profile = await res.json();
      if (epoch !== authEpoch) return;
      cb.setRedeemToken("");
      cb.showQuickFeedback(
        `Account @${profile.username} created via single-use invite! Signing in...`
      );
      await signIn(profile.username, password);
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Invite redemption error: ${formatErr(err)}`);
    }
  };

  const redeemPasswordReset = async (rawToken: string, newPassword: string) => {
    if (!rawToken || !newPassword) {
      cb.showQuickFeedback("Reset token and new password are required");
      return;
    }
    const epoch = authEpoch;
    try {
      const res = await fetch("/api/v1/auth/password-reset/redeem", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          raw_token: rawToken,
          new_password: newPassword,
        }),
      });
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Password reset failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const profile = await res.json();
      if (epoch !== authEpoch) return;
      cb.setRedeemToken("");
      cb.showQuickFeedback(
        `Password rotated for @${profile.username} & prior sessions revoked! Signing in...`
      );
      await signIn(profile.username, newPassword);
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Password reset error: ${formatErr(err)}`);
    }
  };

  const issueInvite = async (
    targetEmail: string,
    assignedRole: "SiteAdmin" | "Maintainer" | "Developer"
  ) => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback("Sign in as SiteAdmin to issue invite links");
      return;
    }
    const epoch = authEpoch;
    try {
      const res = await fetch("/api/v1/admin/invites", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          target_email: targetEmail,
          assigned_role: assignedRole,
          team_ids: [],
        }),
      });
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Invite issuance failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const issued = await res.json();
      if (epoch !== authEpoch) return;
      cb.setLastIssuedInviteUrl(String(issued.invite_url_path ?? ""));
      cb.showQuickFeedback(
        `Issued 72h single-use invite link for ${targetEmail}`
      );
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Invite issuance error: ${formatErr(err)}`);
    }
  };

  const revokeInvite = async (inviteId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = authEpoch;
    try {
      const res = await fetch(
        `/api/v1/admin/invites/${encodeURIComponent(inviteId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Invite revocation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      cb.showQuickFeedback("Revoked single-use invite link");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Invite revocation error: ${formatErr(err)}`);
    }
  };

  const issuePasswordReset = async (userId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = authEpoch;
    try {
      const res = await fetch(
        `/api/v1/admin/users/${encodeURIComponent(userId)}/password-reset`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Reset link failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const issued = await res.json();
      if (epoch !== authEpoch) return;
      cb.setLastIssuedInviteUrl(String(issued.invite_url_path ?? ""));
      cb.showQuickFeedback("Issued single-use 72h password-reset link");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Reset link error: ${formatErr(err)}`);
    }
  };

  const deactivateUser = async (userId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = authEpoch;
    try {
      const res = await fetch(
        `/api/v1/admin/users/${encodeURIComponent(userId)}/deactivate`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `Deactivation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const profile = await res.json();
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(
        `Deactivated @${profile.username} & revoked all sessions, PATs, and SSH keys`
      );
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`Deactivation error: ${formatErr(err)}`);
    }
  };

  const createPat = async (name: string) => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback("Sign in first to generate a Personal Access Token");
      return;
    }
    const epoch = authEpoch;
    try {
      const res = await fetch("/api/v1/auth/pats", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ name, ttl_days: 90 }),
      });
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `PAT creation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const issued = await res.json();
      if (epoch !== authEpoch) return;
      cb.setLastIssuedPatToken(String(issued.raw_token ?? ""));
      cb.showQuickFeedback(`Generated PAT '${name}' (90d expiry)`);
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`PAT creation error: ${formatErr(err)}`);
    }
  };

  const revokePat = async (patId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = authEpoch;
    try {
      const res = await fetch(
        `/api/v1/auth/pats/${encodeURIComponent(patId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `PAT revocation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      cb.showQuickFeedback("Revoked Personal Access Token");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`PAT revocation error: ${formatErr(err)}`);
    }
  };

  const registerSshKey = async (title: string, publicKeyOpenssh: string) => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback("Sign in first to register an SSH public key");
      return;
    }
    const epoch = authEpoch;
    try {
      const res = await fetch("/api/v1/auth/ssh-keys", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title, public_key_openssh: publicKeyOpenssh }),
      });
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `SSH key registration failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      cb.showQuickFeedback(`Registered SSH key '${title}'`);
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`SSH key error: ${formatErr(err)}`);
    }
  };

  const revokeSshKey = async (keyId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = authEpoch;
    try {
      const res = await fetch(
        `/api/v1/auth/ssh-keys/${encodeURIComponent(keyId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== authEpoch) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== authEpoch) return;
        cb.showQuickFeedback(
          `SSH key revocation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      cb.showQuickFeedback("Revoked SSH public key");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== authEpoch) return;
      cb.showQuickFeedback(`SSH key revocation error: ${formatErr(err)}`);
    }
  };

  return {
    hydrateAuthData,
    signIn,
    signOut,
    redeemInvite,
    redeemPasswordReset,
    issueInvite,
    revokeInvite,
    issuePasswordReset,
    deactivateUser,
    createPat,
    revokePat,
    registerSshKey,
    revokeSshKey,
  };
}
