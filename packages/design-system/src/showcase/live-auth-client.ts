import type {
  EnterpriseAdminViewProps,
  LiveAuthSessionInfo,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "../index";
import { formatErr, getLiveToken } from "./live-repo-client";
import {
  createLiveAuthSession,
  type LiveAuthClientCallbacks,
  type LiveAuthSessionController,
} from "./live-auth-session";

export { createLiveAuthSession, type LiveAuthClientCallbacks, type LiveAuthSessionController };

export function extendLiveAuthClient(session: LiveAuthSessionController) {
  const {
    cb,
    getEpoch,
    nextEpoch,
    clearIdentityState,
    clearLocalAuthState,
    hydrateAuthData,
    syncStoredSession,
  } = session;

  const signIn = async (
    identifier: string,
    password: string,
    totpCode?: string,
  ) => {
    if (!identifier || !password) {
      cb.showQuickFeedback("Please enter both username/email and password");
      return;
    }
    const epoch = nextEpoch();
    try {
      const payload = totpCode?.trim()
        ? { identifier, password, totp_code: totpCode.trim() }
        : { identifier, password };
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (epoch !== getEpoch()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch()) return;
        cb.showQuickFeedback(
          `Sign-in failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const data = await res.json();
      if (epoch !== getEpoch()) return;
      if (typeof data.raw_session_token === "string") {
        window.localStorage.setItem("gitust_token", data.raw_session_token);
      }
      clearIdentityState([]);
      cb.onTokenAuthenticated();
      cb.showQuickFeedback(
        `Signed in as @${data.user?.username ?? identifier} (${data.user?.role ?? "User"})`,
      );
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch()) return;
      cb.showQuickFeedback(`Sign-in request failed: ${formatErr(err)}`);
    }
  };

  const signOut = async () => {
    const token = getLiveToken();
    clearLocalAuthState();
    const epoch = getEpoch();
    let revoked = !token;
    if (token) {
      try {
        const res = await fetch("/api/v1/auth/logout", {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        });
        revoked = res.ok;
      } catch {
        revoked = false;
      }
    }
    if (epoch !== getEpoch() || getLiveToken()) return;
    cb.showQuickFeedback(
      revoked
        ? "Signed out and revoked active session token"
        : "Signed out locally; server session revocation could not be confirmed",
    );
  };

  const redeemInvite = async (
    rawToken: string,
    username: string,
    password: string,
  ) => {
    if (!rawToken || !username || !password) {
      cb.showQuickFeedback("Invite token, username, and password are required");
      return;
    }
    const epoch = getEpoch();
    try {
      const res = await fetch("/api/v1/auth/invites/redeem", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ raw_token: rawToken, username, password }),
      });
      if (epoch !== getEpoch()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch()) return;
        cb.showQuickFeedback(
          `Invite redemption failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const profile = await res.json();
      if (epoch !== getEpoch()) return;
      cb.setRedeemToken("");
      cb.showQuickFeedback(
        `Account @${profile.username} created via single-use invite! Signing in...`,
      );
      await signIn(profile.username, password);
    } catch (err) {
      if (epoch !== getEpoch()) return;
      cb.showQuickFeedback(`Invite redemption error: ${formatErr(err)}`);
    }
  };

  const redeemPasswordReset = async (rawToken: string, newPassword: string) => {
    if (!rawToken || !newPassword) {
      cb.showQuickFeedback("Reset token and new password are required");
      return;
    }
    const epoch = getEpoch();
    try {
      const res = await fetch("/api/v1/auth/password-reset/redeem", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          raw_token: rawToken,
          new_password: newPassword,
        }),
      });
      if (epoch !== getEpoch()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch()) return;
        cb.showQuickFeedback(
          `Password reset failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const profile = await res.json();
      if (epoch !== getEpoch()) return;
      cb.setRedeemToken("");
      cb.showQuickFeedback(
        `Password rotated for @${profile.username} & prior sessions revoked! Signing in...`,
      );
      await signIn(profile.username, newPassword);
    } catch (err) {
      if (epoch !== getEpoch()) return;
      cb.showQuickFeedback(`Password reset error: ${formatErr(err)}`);
    }
  };

  const issueInvite = async (
    targetEmail: string,
    assignedRole: "SiteAdmin" | "Maintainer" | "Developer",
  ) => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback("Sign in as SiteAdmin to issue invite links");
      return;
    }
    const epoch = getEpoch();
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
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `Invite issuance failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const issued = await res.json();
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.setLastIssuedInviteUrl(String(issued.invite_url_path ?? ""));
      cb.showQuickFeedback(
        `Issued 72h single-use invite link for ${targetEmail}`,
      );
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`Invite issuance error: ${formatErr(err)}`);
    }
  };

  const revokeInvite = async (inviteId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = getEpoch();
    try {
      const res = await fetch(
        `/api/v1/admin/invites/${encodeURIComponent(inviteId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `Invite revocation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      cb.showQuickFeedback("Revoked single-use invite link");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`Invite revocation error: ${formatErr(err)}`);
    }
  };

  const issuePasswordReset = async (userId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = getEpoch();
    try {
      const res = await fetch(
        `/api/v1/admin/users/${encodeURIComponent(userId)}/password-reset`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `Reset link failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const issued = await res.json();
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.setLastIssuedInviteUrl(String(issued.invite_url_path ?? ""));
      cb.showQuickFeedback("Issued single-use 72h password-reset link");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`Reset link error: ${formatErr(err)}`);
    }
  };

  const deactivateUser = async (userId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = getEpoch();
    try {
      const res = await fetch(
        `/api/v1/admin/users/${encodeURIComponent(userId)}/deactivate`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `Deactivation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const profile = await res.json();
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(
        `Deactivated @${profile.username} & revoked all sessions, PATs, and SSH keys`,
      );
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`Deactivation error: ${formatErr(err)}`);
    }
  };

  const createPat = async (name: string) => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback("Sign in first to generate a Personal Access Token");
      return;
    }
    const epoch = getEpoch();
    try {
      const res = await fetch("/api/v1/auth/pats", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ name, ttl_days: 90 }),
      });
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `PAT creation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      const issued = await res.json();
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.setLastIssuedPatToken(String(issued.raw_token ?? ""));
      cb.showQuickFeedback(`Generated PAT '${name}' (90d expiry)`);
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`PAT creation error: ${formatErr(err)}`);
    }
  };

  const revokePat = async (patId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = getEpoch();
    try {
      const res = await fetch(
        `/api/v1/auth/pats/${encodeURIComponent(patId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `PAT revocation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      cb.showQuickFeedback("Revoked Personal Access Token");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`PAT revocation error: ${formatErr(err)}`);
    }
  };

  const registerSshKey = async (title: string, publicKeyOpenssh: string) => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback("Sign in first to register an SSH public key");
      return;
    }
    const epoch = getEpoch();
    try {
      const res = await fetch("/api/v1/auth/ssh-keys", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title, public_key_openssh: publicKeyOpenssh }),
      });
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `SSH key registration failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      cb.showQuickFeedback(`Registered SSH key '${title}'`);
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`SSH key error: ${formatErr(err)}`);
    }
  };

  const revokeSshKey = async (keyId: string) => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = getEpoch();
    try {
      const res = await fetch(
        `/api/v1/auth/ssh-keys/${encodeURIComponent(keyId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== getEpoch() || token !== getLiveToken()) return;
        cb.showQuickFeedback(
          `SSH key revocation failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`,
        );
        return;
      }
      cb.showQuickFeedback("Revoked SSH public key");
      await hydrateAuthData();
    } catch (err) {
      if (epoch !== getEpoch() || token !== getLiveToken()) return;
      cb.showQuickFeedback(`SSH key revocation error: ${formatErr(err)}`);
    }
  };

  return {
    hydrateAuthData,
    syncStoredSession,
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

export function createLiveAuthClient(cb: LiveAuthClientCallbacks) {
  return extendLiveAuthClient(createLiveAuthSession(cb));
}

export interface AdminAuthPropsSource {
  liveMode: boolean;
  liveSession: LiveAuthSessionInfo | null;
  redeemMode: "invite" | "reset";
  setRedeemMode: (mode: "invite" | "reset") => void;
  redeemToken: string;
  setRedeemToken: (token: string) => void;
  liveInvites: LiveInviteItem[];
  lastIssuedInviteUrl: string | null;
  livePats: LivePatItem[];
  lastIssuedPatToken: string | null;
  liveSshKeys: LiveSshKeyItem[];
  session: LiveAuthSessionController;
}

export const buildAdminAuthProps = (
  source: AdminAuthPropsSource,
): Partial<EnterpriseAdminViewProps> => {
  const client = extendLiveAuthClient(source.session);
  return {
    liveMode: source.liveMode,
    liveSession: source.liveSession,
    onSignIn: (id, pw, totp) => void client.signIn(id, pw, totp),
    onSignOut: () => void client.signOut(),
    redeemMode: source.redeemMode,
    onChangeRedeemMode: source.setRedeemMode,
    redeemToken: source.redeemToken,
    onRedeemTokenInput: source.setRedeemToken,
    onRedeemInvite: (tok, user, pw) => void client.redeemInvite(tok, user, pw),
    onRedeemPasswordReset: (tok, pw) =>
      void client.redeemPasswordReset(tok, pw),
    liveInvites: source.liveInvites,
    lastIssuedInviteUrl: source.lastIssuedInviteUrl,
    onIssueInvite: (email, role) => void client.issueInvite(email, role),
    onRevokeInvite: (id) => void client.revokeInvite(id),
    onIssuePasswordReset: (id) => void client.issuePasswordReset(id),
    onDeactivateLiveUser: (id) => void client.deactivateUser(id),
    livePats: source.livePats,
    lastIssuedPatToken: source.lastIssuedPatToken,
    onCreatePat: (name) => void client.createPat(name),
    onRevokePat: (id) => void client.revokePat(id),
    liveSshKeys: source.liveSshKeys,
    onRegisterSshKey: (title, key) => void client.registerSshKey(title, key),
    onRevokeSshKey: (id) => void client.revokeSshKey(id),
  };
};
