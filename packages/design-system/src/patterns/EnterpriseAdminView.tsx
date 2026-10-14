import { Component, createSignal, For } from "solid-js";
import { Badge } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";
import { Avatar } from "../primitives/TabsAndAvatar";

export interface EnterpriseUser {
  id: number;
  username: string;
  email: string;
  department: string;
  role: "Site Admin" | "Maintainer" | "Developer";
  twoFactorEnabled: boolean;
  status: "active" | "suspended";
}

export interface EnterpriseAdminViewProps {
  users: EnterpriseUser[];
  onToggleRole: (userId: number) => void;
  onToggleStatus: (userId: number) => void;
  onPolicyFeedback: (msg: string) => void;
}

/**
 * Self-Hosted Enterprise Site Admin & IAM/Auth Control Console.
 * Demonstrates:
 * - On-Premise Sign-In / SSO / Registration Domain Lock
 * - Site Admin User Role & Security Policy Management with 0ms Optimistic UI
 */
export const EnterpriseAdminView: Component<EnterpriseAdminViewProps> = (
  props
) => {
  const [inviteOnly, setInviteOnly] = createSignal(true);
  const [require2FA, setRequire2FA] = createSignal(true);
  const [ssoEnforced, setSsoEnforced] = createSignal(false);
  const [allowedDomain, setAllowedDomain] = createSignal("company.internal");

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
      {/* Left Column: Enterprise Auth / Sign-In Card & Security Policies */}
      <div
        style={{
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-4)",
        }}
      >
        {/* Self-Hosted Auth & SSO Sign-In Preview Card */}
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
            <Badge variant="success">On-Premise</Badge>
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
                Internal Git & CI/CD Server · Allowed:{" "}
                <span class="ds-mono">@{allowedDomain()}</span>
              </div>
            </div>

            <Input
              icon={<Icon name="users" size={14} />}
              placeholder={`username@${allowedDomain()}`}
            />
            <Input
              type="password"
              icon={<Icon name="key" size={14} />}
              placeholder="Password or Personal Access Token"
            />

            <Button
              variant="primary"
              icon={<Icon name="shield-check" size={14} />}
              onClick={() =>
                props.onPolicyFeedback(
                  "Authenticated via Argon2id + Ed25519 Session Token (< 2ms)"
                )
              }
            >
              Sign In to Private Instance
            </Button>

            <Button
              variant="secondary"
              icon={<Icon name="lock" size={14} />}
              onClick={() =>
                props.onPolicyFeedback(
                  "Redirecting to Corporate OIDC / Okta / Keycloak SSO..."
                )
              }
            >
              Continue with Corporate SSO (OIDC / LDAP)
            </Button>
          </div>
        </div>

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
              <span>Registration & Security Policy</span>
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
                  Only Site Admins or Corporate SSO can provision accounts
                </div>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={inviteOnly()}
                data-checked={inviteOnly()}
                class="ds-switch"
                onClick={() =>
                  toggleSwitch(
                    "Invite-Only Mode",
                    inviteOnly,
                    setInviteOnly
                  )
                }
              >
                <span class="ds-switch-thumb" />
              </button>
            </div>

            <div class="ds-toggle-row">
              <div>
                <div style={{ "font-weight": 500 }}>
                  Mandatory 2FA / Passkey & Signed Commits
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
                  Disable password login for non-admin accounts
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
                  "margin-bottom": "4px",
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
        {/* System Health & Embedded Services */}
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
                Embedded russh SSH :2222 · HTTP :3000
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
                <span>Gitust Actions Runners</span>
                <Icon name="play" size={14} />
              </div>
              <div
                class="ds-mono"
                style={{
                  "font-size": "var(--ds-font-size-lg)",
                  "font-weight": 700,
                }}
              >
                4 Online / 0 Idle
              </div>
              <div
                style={{
                  "font-size": "var(--ds-font-size-2xs)",
                  color: "var(--ds-fg-muted)",
                }}
              >
                act_runner compatible · Private Docker Pool
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
                <span>Security & Audit</span>
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

        {/* User & Permission Matrix Table */}
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
                Organization Users & Access Control ({props.users.length})
              </span>
            </span>
            <span
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Click role or status for 0ms Optimistic Admin Mutation
            </span>
          </div>

          <div class="ds-issue-list" style={{ border: "none", "border-radius": 0 }}>
            <For each={props.users}>
              {(user) => (
                <div class="ds-issue-row">
                  <div class="ds-issue-left">
                    <Avatar name={user.username} size="sm" />
                    <div style={{ "min-width": "150px" }}>
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
                      <span>{user.twoFactorEnabled ? "2FA+SSH" : "No 2FA"}</span>
                    </Badge>
                  </div>

                  <div class="ds-issue-right">
                    <Button
                      size="xs"
                      variant={
                        user.role === "Site Admin" ? "primary" : "secondary"
                      }
                      onClick={() => props.onToggleRole(user.id)}
                    >
                      {user.role}
                    </Button>
                    <Button
                      size="xs"
                      variant={
                        user.status === "active" ? "ghost" : "danger"
                      }
                      onClick={() => props.onToggleStatus(user.id)}
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
