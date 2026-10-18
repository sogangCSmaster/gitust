import type {
  EnterpriseUser,
  LiveAuthSessionInfo,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "../index";
import { formatErr, getLiveToken } from "./live-repo-context";

export interface LiveAuthClientCallbacks {
  setLiveSession: (session: LiveAuthSessionInfo | null) => void;
  setEnterpriseUsers: (users: EnterpriseUser[]) => void;
  setLiveInvites: (invites: LiveInviteItem[]) => void;
  setLastIssuedInviteUrl: (url: string | null) => void;
  setLivePats: (pats: LivePatItem[]) => void;
  setLastIssuedPatToken: (token: string | null) => void;
  setLiveSshKeys: (keys: LiveSshKeyItem[]) => void;
  setRedeemToken: (token: string) => void;
  fallbackUsers?: EnterpriseUser[];
  onTokenAuthenticated: () => void;
  onTokenCleared: () => void;
  showQuickFeedback: (msg: string) => void;
}

const mapRoleLabel = (
  role: string,
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
  }>,
): EnterpriseUser[] =>
  rawUsers.map((u, idx) => ({
    id: idx + 1,
    liveId: u.id,
    username: u.username,
    email: u.email,
    department: "Live account",
    role: mapRoleLabel(u.role),
    twoFactorEnabled: false,
    status: u.status === "active" ? "active" : "suspended",
  }));

export function createLiveAuthSession(cb: LiveAuthClientCallbacks) {
  let authEpoch = 0;
  let hydrationId = 0;

  const clearIdentityState = (users: EnterpriseUser[]) => {
    cb.setLiveSession(null);
    cb.setLiveInvites([]);
    cb.setLastIssuedInviteUrl(null);
    cb.setLivePats([]);
    cb.setLastIssuedPatToken(null);
    cb.setLiveSshKeys([]);
    cb.setRedeemToken("");
    cb.setEnterpriseUsers(users);
  };

  const clearLocalAuthState = () => {
    authEpoch++;
    try {
      if (typeof window !== "undefined") {
        window.localStorage.removeItem("gitust_token");
      }
    } catch {
      // Ignore storage error in restricted environments
    }
    clearIdentityState(cb.fallbackUsers ?? []);
    cb.onTokenCleared();
  };

  const hydrateAuthData = async () => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = authEpoch;
    const requestId = ++hydrationId;
    const current = () =>
      epoch === authEpoch &&
      requestId === hydrationId &&
      token === getLiveToken();
    const headers = { Authorization: `Bearer ${token}` };

    try {
      const meRes = await fetch("/api/v1/auth/me", { headers });
      if (!current()) return;
      if (!meRes.ok) {
        if (meRes.status === 401 || meRes.status === 403) {
          clearLocalAuthState();
        }
        return;
      }
      const me = (await meRes.json()) as LiveAuthSessionInfo;
      if (!current()) return;
      cb.setLiveSession(me);

      const [patsRes, sshRes] = await Promise.all([
        fetch("/api/v1/auth/pats", { headers }),
        fetch("/api/v1/auth/ssh-keys", { headers }),
      ]);
      if (!current()) return;
      if (patsRes.ok) {
        const pats = await patsRes.json();
        if (current() && Array.isArray(pats)) {
          cb.setLivePats(pats);
        }
      }
      if (sshRes.ok) {
        const keys = await sshRes.json();
        if (current() && Array.isArray(keys)) {
          cb.setLiveSshKeys(keys);
        }
      }

      if (me.role === "SiteAdmin") {
        const [usersRes, invitesRes] = await Promise.all([
          fetch("/api/v1/admin/users", { headers }),
          fetch("/api/v1/admin/invites", { headers }),
        ]);
        if (!current()) return;
        if (usersRes.ok) {
          const rawUsers = await usersRes.json();
          if (current() && Array.isArray(rawUsers)) {
            cb.setEnterpriseUsers(mapLiveUsers(rawUsers));
          }
        }
        if (invitesRes.ok) {
          const invites = await invitesRes.json();
          if (current() && Array.isArray(invites)) {
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
              status: "active",
            },
          ]),
        );
      }
    } catch (err) {
      if (!current()) return;
      console.warn("Live auth hydration failed:", formatErr(err));
    }
  };

  const syncStoredSession = async () => {
    authEpoch++;
    const token = getLiveToken();
    clearIdentityState(token ? [] : (cb.fallbackUsers ?? []));
    if (token) {
      cb.onTokenAuthenticated();
      await hydrateAuthData();
    } else cb.onTokenCleared();
  };

  return {
    cb,
    getEpoch: () => authEpoch,
    nextEpoch: () => ++authEpoch,
    clearIdentityState,
    clearLocalAuthState,
    hydrateAuthData,
    syncStoredSession,
  };
}

export type LiveAuthSessionController = ReturnType<typeof createLiveAuthSession>;
