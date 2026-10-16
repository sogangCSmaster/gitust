import { Component, createSignal, For, Show } from "solid-js";
import { Badge } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";
import { Avatar } from "../primitives/TabsAndAvatar";
import {
  EnterpriseCredentialsPanel,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "./EnterpriseCredentialsPanel";

export interface EnterpriseUser {
  id: number;
  liveId?: string;
  username: string;
  email: string;
  department: string;
  role: "Site Admin" | "Maintainer" | "Developer";
  twoFactorEnabled: boolean;
  status: "active" | "suspended";
}

export interface LiveAuthSessionInfo {
  user_id: string;
  username: string;
  email: string;
  role: "SiteAdmin" | "Maintainer" | "Developer";
}

export interface EnterpriseAdminViewProps {
  users: EnterpriseUser[];
  onToggleRole: (userId: number) => void;
  onToggleStatus: (userId: number) => void;
  onPolicyFeedback: (msg: string) => void;
  liveSession?: LiveAuthSessionInfo | null;
  onSignIn?: (identifier: string, password: string) => void;
  onSignOut?: () => void;
  redeemMode?: "invite" | "reset";
  onChangeRedeemMode?: (mode: "invite" | "reset") => void;
  redeemToken?: string;
  onRedeemTokenInput?: (val: string) => void;
  onRedeemInvite?: (token: string, username: string, password: string) => void;
  onRedeemPasswordReset?: (token: string, newPassword: string) => void;
  liveInvites?: LiveInviteItem[];
  lastIssuedInviteUrl?: string | null;
  onIssueInvite?: (
    targetEmail: string,
    role: "SiteAdmin" | "Maintainer" | "Developer"
  ) => void;
  onRevokeInvite?: (inviteId: string) => void;
  onIssuePasswordReset?: (userId: string) => void;
  onDeactivateLiveUser?: (userId: string) => void;
  livePats?: LivePatItem[];
  lastIssuedPatToken?: string | null;
  onCreatePat?: (name: string) => void;
  onRevokePat?: (patId: string) => void;
  liveSshKeys?: LiveSshKeyItem[];
  onRegisterSshKey?: (title: string, publicKeyOpenssh: string) => void;
  onRevokeSshKey?: (keyId: string) => void;
}

/**
 * Self-Hosted Enterprise Site Admin & IAM/Auth Control Console.
 * Supports both live `gitust serve` backend operations (Sign-In, Sign-Out, Single-Use Invite
 * Issuance/Redemption, Password Reset, PATs, SSH Keys, User Deactivation) and 0ms Showcase UI.
 */
export const EnterpriseAdminView: Component<EnterpriseAdminViewProps> = (
  props
) => {
  const [inviteOnly, setInviteOnly] = createSignal(true);
  const [require2FA, setRequire2FA] = createSignal(true);
  const [ssoEnforced, setSsoEnforced] = createSignal(false);
  const [allowedDomain, setAllowedDomain] = createSignal("company.internal");

  const [loginIdentifier, setLoginIdentifier] = createSignal("");
  const [loginPassword, setLoginPassword] = createSignal("");
  const [localRedeemMode, setLocalRedeemMode] = createSignal<
    "invite" | "reset"
  >("invite");
  const [localRedeemToken, setLocalRedeemToken] = createSignal("");

  const activeRedeemMode = () => props.redeemMode ?? localRedeemMode();
  const activeRedeemToken = () => props.redeemToken ?? localRedeemToken();

  const toggleSwitch = (
    label: string,
    getter: () => boolean,
    setter: (v: boolean) => void
  ) => {
    const next = !getter();
    setter(next);
    props.onPolicyFeedback(
      `Enterprise Security Policy Updated: ${label} → ${next ? "ENABLED" : "DISABLED"} (0ms Optimistic)`
    );
  };

  return (
    <div class="ds-admin-grid">
      {/* Left Column: Enterprise Auth Portal, Credentials & Security Policies */}
      <div
        style={{
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-4)",
        }}
      >
        {/* Self-Hosted Auth & Session Sign-In Portal Card */}
        <div class="ds-card">
          <div class="ds-card-header">
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--ds-space-1-5)",
              }}
            >
              <Icon name="lock" size={13} />
              <span>Enterprise Self-Hosted Auth Portal</span>
            </span>
            <Show
              when={props.liveSession}
              fallback={<Badge variant="success">On-Premise</Badge>}
            >
              {(ses) => (
                <Badge variant="accent" mono>
                  @{ses().username} · {ses().role}
                </Badge>
              )}
            </Show>
          </div>
          <div class="ds-card-body">
            <div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-md)",
                  "font-weight": 600,
                }}
              >
                Sign in to Gitust Enterprise
              </div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-xs)",
                  color: "var(--ds-fg-muted)",
                }}
              >
                Invite-Only On-Premise Git &amp; Sandbox Server · Allowed:{" "}
                <span class="ds-mono">@{allowedDomain()}</span>
              </div>
            </div>

            <Input
              icon={<Icon name="users" size={14} />}
              value={loginIdentifier()}
              onInput={(e) => setLoginIdentifier(e.currentTarget.value)}
              placeholder={`username or email@${allowedDomain()}`}
            />
            <Input
              type="password"
              icon={<Icon name="key" size={14} />}
              value={loginPassword()}
              onInput={(e) => setLoginPassword(e.currentTarget.value)}
              placeholder="Password (Argon2id verified)"
            />

            <div style={{ display: "flex", gap: "var(--ds-space-2)" }}>
              <Button
                variant="primary"
                icon={<Icon name="shield-check" size={14} />}
                onClick={() => {
                  if (props.onSignIn) {
                    props.onSignIn(loginIdentifier().trim(), loginPassword());
                    setLoginPassword("");
                  } else {
                    props.onPolicyFeedback(
                      "Authenticated via Argon2id + CSPRNG Session Token (< 2ms)"
                    );
                  }
                }}
              >
                Sign In to Private Instance
              </Button>
              <Show when={props.liveSession && props.onSignOut}>
                <Button
                  variant="danger"
                  icon={<Icon name="x" size={14} />}
                  onClick={() => props.onSignOut?.()}
                >
                  Sign Out
                </Button>
              </Show>
            </div>
          </div>
        </div>

        <EnterpriseCredentialsPanel
          allowedDomain={allowedDomain()}
          redeemMode={activeRedeemMode()}
          onChangeRedeemMode={(m) =>
            props.onChangeRedeemMode
              ? props.onChangeRedeemMode(m)
              : setLocalRedeemMode(m)
          }
          redeemToken={activeRedeemToken()}
          onRedeemTokenInput={(v) =>
            props.onRedeemTokenInput
              ? props.onRedeemTokenInput(v)
              : setLocalRedeemToken(v)
          }
          onRedeemInvite={props.onRedeemInvite}
          onRedeemPasswordReset={props.onRedeemPasswordReset}
          liveInvites={props.liveInvites}
          lastIssuedInviteUrl={props.lastIssuedInviteUrl}
          onIssueInvite={props.onIssueInvite}
          onRevokeInvite={props.onRevokeInvite}
          livePats={props.livePats}
          lastIssuedPatToken={props.lastIssuedPatToken}
          onCreatePat={props.onCreatePat}
          onRevokePat={props.onRevokePat}
          liveSshKeys={props.liveSshKeys}
          onRegisterSshKey={props.onRegisterSshKey}
          onRevokeSshKey={props.onRevokeSshKey}
          onPolicyFeedback={props.onPolicyFeedback}
        />

        {/* Site Admin Registration & Security Controls */}
        <div class="ds-card">
          <div class="ds-card-header">
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--ds-space-1-5)",
              }}
            >
              <Icon name="shield-check" size={13} />
              <span>Registration &amp; Security Policy</span>
            </span>
            <Badge variant="accent">Site Admin</Badge>
          </div>
          <div class="ds-card-body">
            <div class="ds-toggle-row">
              <div>
                <div style={{ "font-weight": 500 }}>
                  Disable Public Registration (Invite-Only)
                </div>
                <div
                  style={{
                    "font-size": "var(--ds-font-size-2xs)",
                    color: "var(--ds-fg-muted)",
                  }}
                >
                  Accounts are created strictly via Site Admin single-use links
                </div>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={inviteOnly()}
                data-checked={inviteOnly()}
                class="ds-switch"
                onClick={() =>
                  toggleSwitch("Invite-Only Mode", inviteOnly, setInviteOnly)
                }
              >
                <span class="ds-switch-thumb" />
              </button>
            </div>

            <div class="ds-toggle-row">
              <div>
                <div style={{ "font-weight": 500 }}>
                  Mandatory 2FA / Passkey &amp; Signed Commits
                </div>
                <div
                  style={{
                    "font-size": "var(--ds-font-size-2xs)",
                    color: "var(--ds-fg-muted)",
                  }}
                >
                  Require hardware WebAuthn/TOTP and SSH/GPG commit signatures
                </div>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={require2FA()}
                data-checked={require2FA()}
                class="ds-switch"
                onClick={() =>
                  toggleSwitch(
                    "Mandatory 2FA & Signed Commits",
                    require2FA,
                    setRequire2FA
                  )
                }
              >
                <span class="ds-switch-thumb" />
              </button>
            </div>

            <div class="ds-toggle-row">
              <div>
                <div style={{ "font-weight": 500 }}>
                  Enforce Corporate SSO (OIDC / SAML) Only
                </div>
                <div
                  style={{
                    "font-size": "var(--ds-font-size-2xs)",
                    color: "var(--ds-fg-muted)",
                  }}
                >
                  Phase 4 Enterprise SSO integration toggle
                </div>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={ssoEnforced()}
                data-checked={ssoEnforced()}
                class="ds-switch"
                onClick={() =>
                  toggleSwitch(
                    "Enforce Corporate SSO",
                    ssoEnforced,
                    setSsoEnforced
                  )
                }
              >
                <span class="ds-switch-thumb" />
              </button>
            </div>

            <div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-xs)",
                  "margin-bottom": "var(--ds-space-1)",
                  color: "var(--ds-fg-secondary)",
                }}
              >
                Allowed Email Domain Whitelist
              </div>
              <Input
                value={allowedDomain()}
                onInput={(e) => setAllowedDomain(e.currentTarget.value)}
              />
            </div>
          </div>
        </div>
      </div>

      {/* Right Column: Site Admin User Directory & Self-Hosted Infrastructure */}
      <div
        style={{
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-4)",
        }}
      >
        <div
          style={{
            display: "grid",
            "grid-template-columns": "repeat(3, 1fr)",
            gap: "var(--ds-space-3)",
          }}
        >
          <div class="ds-card">
            <div class="ds-card-body">
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "space-between",
                  color: "var(--ds-fg-secondary)",
                  "font-size": "var(--ds-font-size-xs)",
                }}
              >
                <span>Git Engine (gitoxide)</span>
                <Icon name="server" size={14} />
              </div>
              <div
                class="ds-mono"
                style={{
                  "font-size": "var(--ds-font-size-lg)",
                  "font-weight": 700,
                  color: "var(--ds-success-fg)",
                }}
              >
                In-Process Zero-Copy
              </div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-2xs)",
                  color: "var(--ds-fg-muted)",
                }}
              >
                Single Binary · HTTP :3000
              </div>
            </div>
          </div>

          <div class="ds-card">
            <div class="ds-card-body">
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "space-between",
                  color: "var(--ds-fg-secondary)",
                  "font-size": "var(--ds-font-size-xs)",
                }}
              >
                <span>Sandbox Isolation</span>
                <Icon name="play" size={14} />
              </div>
              <div
                class="ds-mono"
                style={{
                  "font-size": "var(--ds-font-size-lg)",
                  "font-weight": 700,
                }}
              >
                CoW + OS Sandbox
              </div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-2xs)",
                  color: "var(--ds-fg-muted)",
                }}
              >
                Zero Cloud Leakage
              </div>
            </div>
          </div>

          <div class="ds-card">
            <div class="ds-card-body">
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "space-between",
                  color: "var(--ds-fg-secondary)",
                  "font-size": "var(--ds-font-size-xs)",
                }}
              >
                <span>Auth &amp; Onboarding</span>
                <Icon name="shield-check" size={14} />
              </div>
              <div
                class="ds-mono"
                style={{
                  "font-size": "var(--ds-font-size-lg)",
                  "font-weight": 700,
                }}
              >
                {inviteOnly() ? "Invite-Only" : "Domain-Locked"}
              </div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-2xs)",
                  color: "var(--ds-fg-muted)",
                }}
              >
                Domain: @{allowedDomain()}
              </div>
            </div>
          </div>
        </div>

        <div class="ds-card">
          <div class="ds-card-header">
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--ds-space-1-5)",
              }}
            >
              <Icon name="users" size={14} />
              <span>
                Organization Users &amp; Access Control ({props.users.length})
              </span>
            </span>
            <span
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Deactivation immediately revokes all sessions, PATs &amp; SSH keys
            </span>
          </div>

          <div
            class="ds-issue-list"
            style={{ border: "none", "border-radius": 0 }}
          >
            <For each={props.users}>
              {(user) => (
                <div class="ds-issue-row">
                  <div class="ds-issue-left">
                    <Avatar name={user.username} size="sm" />
                    <div style={{ flex: "1" }}>
                      <div
                        style={{
                          "font-weight": 600,
                          "font-size": "var(--ds-font-size-xs)",
                        }}
                      >
                        {user.username}
                      </div>
                      <div
                        class="ds-mono"
                        style={{
                          "font-size": "var(--ds-font-size-2xs)",
                          color: "var(--ds-fg-muted)",
                        }}
                      >
                        {user.email}
                      </div>
                    </div>
                    <Badge variant="neutral">{user.department}</Badge>
                    <Badge
                      variant={user.twoFactorEnabled ? "success" : "warning"}
                      mono
                    >
                      <Icon name="shield-check" size={11} />
                      <span>
                        {user.twoFactorEnabled ? "Argon2+SSH" : "No 2FA"}
                      </span>
                    </Badge>
                  </div>

                  <div class="ds-issue-right">
                    <Show when={user.liveId && props.onIssuePasswordReset}>
                      <Button
                        size="xs"
                        variant="ghost"
                        onClick={() =>
                          user.liveId &&
                          props.onIssuePasswordReset?.(user.liveId)
                        }
                      >
                        Reset Link
                      </Button>
                    </Show>
                    <Button
                      size="xs"
                      variant={
                        user.role === "Site Admin" ? "primary" : "secondary"
                      }
                      onClick={() =>
                        user.liveId
                          ? props.onPolicyFeedback(
                              `Role '${user.role}' for @${user.username} is bound at invite creation`
                            )
                          : props.onToggleRole(user.id)
                      }
                    >
                      {user.role}
                    </Button>
                    <Button
                      size="xs"
                      variant={user.status === "active" ? "ghost" : "danger"}
                      onClick={() => {
                        if (user.liveId) {
                          if (
                            user.status === "active" &&
                            props.onDeactivateLiveUser
                          ) {
                            props.onDeactivateLiveUser(user.liveId);
                          } else {
                            props.onPolicyFeedback(
                              `Account @${user.username} is deactivated; issue a new single-use invite to onboard`
                            );
                          }
                        } else {
                          props.onToggleStatus(user.id);
                        }
                      }}
                    >
                      {user.status === "active" ? "Active" : "Suspended"}
                    </Button>
                  </div>
                </div>
              )}
            </For>
          </div>
        </div>
      </div>
    </div>
  );
};
