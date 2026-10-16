import { Component, createSignal, For, Show } from "solid-js";
import { Badge } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";

export interface LiveInviteItem {
  id: string;
  purpose: "AccountInvite" | "PasswordReset";
  target_email: string;
  assigned_role: "SiteAdmin" | "Maintainer" | "Developer";
  expires_at: string;
  used_at: string | null;
  revoked_at: string | null;
}

export interface LivePatItem {
  id: string;
  name: string;
  token_prefix: string;
  expires_at: string | null;
  revoked: boolean;
}

export interface LiveSshKeyItem {
  id: string;
  title: string;
  fingerprint_sha256: string;
  revoked: boolean;
}

export interface EnterpriseCredentialsPanelProps {
  allowedDomain: string;
  redeemMode: "invite" | "reset";
  onChangeRedeemMode: (mode: "invite" | "reset") => void;
  redeemToken: string;
  onRedeemTokenInput: (val: string) => void;
  onRedeemInvite?: (token: string, username: string, password: string) => void;
  onRedeemPasswordReset?: (token: string, newPassword: string) => void;
  liveInvites?: LiveInviteItem[];
  lastIssuedInviteUrl?: string | null;
  onIssueInvite?: (
    targetEmail: string,
    role: "SiteAdmin" | "Maintainer" | "Developer"
  ) => void;
  onRevokeInvite?: (inviteId: string) => void;
  livePats?: LivePatItem[];
  lastIssuedPatToken?: string | null;
  onCreatePat?: (name: string) => void;
  onRevokePat?: (patId: string) => void;
  liveSshKeys?: LiveSshKeyItem[];
  onRegisterSshKey?: (title: string, publicKeyOpenssh: string) => void;
  onRevokeSshKey?: (keyId: string) => void;
  onPolicyFeedback: (msg: string) => void;
}

export const EnterpriseCredentialsPanel: Component<
  EnterpriseCredentialsPanelProps
> = (props) => {
  const [redeemUsername, setRedeemUsername] = createSignal("");
  const [redeemPassword, setRedeemPassword] = createSignal("");

  const [inviteEmail, setInviteEmail] = createSignal("");
  const [inviteRole, setInviteRole] = createSignal<
    "SiteAdmin" | "Maintainer" | "Developer"
  >("Developer");

  const [patName, setPatName] = createSignal("");
  const [sshTitle, setSshTitle] = createSignal("");
  const [sshPubkey, setSshPubkey] = createSignal("");

  const isExpired = (iso: string | null | undefined): boolean =>
    Boolean(iso && Date.parse(iso) <= Date.now());

  const handleRedeemSubmit = () => {
    const token = props.redeemToken.trim();
    const pwd = redeemPassword();
    if (props.redeemMode === "invite") {
      if (props.onRedeemInvite) {
        props.onRedeemInvite(token, redeemUsername().trim(), pwd);
      } else {
        props.onPolicyFeedback(
          "Invite redeemed via single-use SHA-256 token & Argon2id hash"
        );
      }
    } else if (props.onRedeemPasswordReset) {
      props.onRedeemPasswordReset(token, pwd);
    } else {
      props.onPolicyFeedback(
        "Password reset redeemed & prior sessions revoked"
      );
    }
    setRedeemPassword("");
  };

  return (
    <div
      style={{
        display: "flex",
        "flex-direction": "column",
        gap: "var(--ds-space-4)",
      }}
    >
      {/* Single-Use Invite / Password-Reset Token Redemption Card */}
      <div class="ds-card">
        <div class="ds-card-header">
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "var(--ds-space-1-5)",
            }}
          >
            <Icon name="key" size={13} />
            <span>Redeem Single-Use Invite or Password-Reset Link</span>
          </span>
          <div style={{ display: "flex", gap: "var(--ds-space-1)" }}>
            <Button
              size="xs"
              variant={props.redeemMode === "invite" ? "primary" : "secondary"}
              onClick={() => props.onChangeRedeemMode("invite")}
            >
              Accept Invite
            </Button>
            <Button
              size="xs"
              variant={props.redeemMode === "reset" ? "primary" : "secondary"}
              onClick={() => props.onChangeRedeemMode("reset")}
            >
              Reset Password
            </Button>
          </div>
        </div>
        <div class="ds-card-body">
          <Input
            icon={<Icon name="key" size={14} />}
            value={props.redeemToken}
            onInput={(e) => props.onRedeemTokenInput(e.currentTarget.value)}
            placeholder={
              props.redeemMode === "invite"
                ? "Single-use invite token (gitust_inv_...)"
                : "Single-use reset token (gitust_rst_...)"
            }
          />
          <Show when={props.redeemMode === "invite"}>
            <Input
              icon={<Icon name="users" size={14} />}
              value={redeemUsername()}
              onInput={(e) => setRedeemUsername(e.currentTarget.value)}
              placeholder="Choose username (e.g. alice_dev)"
            />
          </Show>
          <Input
            type="password"
            icon={<Icon name="lock" size={14} />}
            value={redeemPassword()}
            onInput={(e) => setRedeemPassword(e.currentTarget.value)}
            placeholder="New password (min 10 characters, Argon2id)"
          />
          <Button
            variant="primary"
            icon={<Icon name="check" size={14} />}
            onClick={handleRedeemSubmit}
          >
            {props.redeemMode === "invite"
              ? "Redeem Invite & Activate Account"
              : "Redeem Reset Token & Rotate Password"}
          </Button>
        </div>
      </div>

      {/* Site Admin Single-Use Invite-Link Generator */}
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
            <span>Site Admin Invite-Link Issuer (72h Single-Use)</span>
          </span>
          <Badge variant="accent">No Public Signup</Badge>
        </div>
        <div class="ds-card-body">
          <div
            style={{
              display: "flex",
              gap: "var(--ds-space-2)",
              "align-items": "center",
              "flex-wrap": "wrap",
            }}
          >
            <div style={{ flex: "1" }}>
              <Input
                icon={<Icon name="users" size={14} />}
                value={inviteEmail()}
                onInput={(e) => setInviteEmail(e.currentTarget.value)}
                placeholder={`engineer@${props.allowedDomain}`}
              />
            </div>
            <Button
              size="xs"
              variant="secondary"
              onClick={() =>
                setInviteRole((r) =>
                  r === "Developer"
                    ? "Maintainer"
                    : r === "Maintainer"
                      ? "SiteAdmin"
                      : "Developer"
                )
              }
            >
              Role: {inviteRole()}
            </Button>
            <Button
              size="sm"
              variant="primary"
              icon={<Icon name="plus" size={13} />}
              onClick={() => {
                if (props.onIssueInvite) {
                  props.onIssueInvite(inviteEmail().trim(), inviteRole());
                  setInviteEmail("");
                } else {
                  props.onPolicyFeedback(
                    `Issued 72h single-use invite link for ${inviteEmail() || `dev@${props.allowedDomain}`}`
                  );
                }
              }}
            >
              Issue Invite Link
            </Button>
          </div>

          <Show when={props.lastIssuedInviteUrl}>
            {(url) => (
              <div
                class="ds-mono"
                style={{
                  padding: "var(--ds-space-2)",
                  background: "var(--ds-bg-subtle)",
                  border: "1px solid var(--ds-border-subtle)",
                  "border-radius": "var(--ds-radius-sm)",
                  "font-size": "var(--ds-font-size-2xs)",
                  "word-break": "break-all",
                }}
              >
                One-time link (share via messenger; SHA-256 stored in DB):{" "}
                <strong>{url()}</strong>
              </div>
            )}
          </Show>

          <Show when={(props.liveInvites?.length ?? 0) > 0}>
            <div
              style={{
                display: "flex",
                "flex-direction": "column",
                gap: "var(--ds-space-1-5)",
              }}
            >
              <For each={props.liveInvites}>
                {(inv) => (
                  <div
                    style={{
                      display: "flex",
                      "align-items": "center",
                      "justify-content": "space-between",
                      "font-size": "var(--ds-font-size-2xs)",
                      padding: "var(--ds-space-1-5) var(--ds-space-2)",
                      background: "var(--ds-bg-subtle)",
                      "border-radius": "var(--ds-radius-sm)",
                    }}
                  >
                    <span class="ds-mono">
                      {inv.target_email} · {inv.purpose} ({inv.assigned_role})
                    </span>
                    <div
                      style={{
                        display: "flex",
                        gap: "var(--ds-space-1-5)",
                        "align-items": "center",
                      }}
                    >
                      <Badge
                        variant={
                          inv.revoked_at
                            ? "danger"
                            : inv.used_at
                              ? "neutral"
                              : isExpired(inv.expires_at)
                                ? "warning"
                                : "success"
                        }
                        mono
                      >
                        {inv.revoked_at
                          ? "Revoked"
                          : inv.used_at
                            ? "Used"
                            : isExpired(inv.expires_at)
                              ? "Expired"
                              : "Active"}
                      </Badge>
                      <Show
                        when={
                          !inv.revoked_at &&
                          !inv.used_at &&
                          !isExpired(inv.expires_at) &&
                          props.onRevokeInvite
                        }
                      >
                        <Button
                          size="xs"
                          variant="danger"
                          onClick={() => props.onRevokeInvite?.(inv.id)}
                        >
                          Revoke
                        </Button>
                      </Show>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </div>
      </div>

      {/* Git Credentials: PAT (HTTPS) & SSH Public Keys (russh) */}
      <div class="ds-card">
        <div class="ds-card-header">
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "var(--ds-space-1-5)",
            }}
          >
            <Icon name="terminal" size={13} />
            <span>Git Access: Personal Access Tokens (HTTPS) &amp; SSH Keys</span>
          </span>
          <Badge variant="info" mono>
            HTTPS + russh
          </Badge>
        </div>
        <div class="ds-card-body">
          <div style={{ display: "flex", gap: "var(--ds-space-2)" }}>
            <div style={{ flex: "1" }}>
              <Input
                icon={<Icon name="key" size={14} />}
                value={patName()}
                onInput={(e) => setPatName(e.currentTarget.value)}
                placeholder="PAT description (e.g. macbook-cli)"
              />
            </div>
            <Button
              size="sm"
              variant="secondary"
              icon={<Icon name="plus" size={13} />}
              onClick={() => {
                if (props.onCreatePat) {
                  props.onCreatePat(patName().trim());
                  setPatName("");
                } else {
                  props.onPolicyFeedback(
                    "Created Personal Access Token (gitust_pat_...)"
                  );
                }
              }}
            >
              Generate PAT
            </Button>
          </div>

          <Show when={props.lastIssuedPatToken}>
            {(tok) => (
              <div
                class="ds-mono"
                style={{
                  padding: "var(--ds-space-2)",
                  background: "var(--ds-bg-subtle)",
                  border: "1px solid var(--ds-border-subtle)",
                  "border-radius": "var(--ds-radius-sm)",
                  "font-size": "var(--ds-font-size-2xs)",
                  "word-break": "break-all",
                }}
              >
                New PAT (copy now; only SHA-256 stored): <strong>{tok()}</strong>
              </div>
            )}
          </Show>

          <Show when={(props.livePats?.length ?? 0) > 0}>
            <For each={props.livePats}>
              {(pat) => (
                <div
                  style={{
                    display: "flex",
                    "align-items": "center",
                    "justify-content": "space-between",
                    "font-size": "var(--ds-font-size-2xs)",
                  }}
                >
                  <span class="ds-mono">
                    {pat.name} ({pat.token_prefix}...)
                  </span>
                  <Show
                    when={!pat.revoked && !isExpired(pat.expires_at)}
                    fallback={
                      <Badge variant={pat.revoked ? "danger" : "warning"}>
                        {pat.revoked ? "Revoked" : "Expired"}
                      </Badge>
                    }
                  >
                    <Button
                      size="xs"
                      variant="ghost"
                      onClick={() => props.onRevokePat?.(pat.id)}
                    >
                      Revoke PAT
                    </Button>
                  </Show>
                </div>
              )}
            </For>
          </Show>

          <div style={{ display: "flex", gap: "var(--ds-space-2)" }}>
            <div style={{ flex: "1" }}>
              <Input
                value={sshTitle()}
                onInput={(e) => setSshTitle(e.currentTarget.value)}
                placeholder="SSH key title"
              />
            </div>
            <div style={{ flex: "2" }}>
              <Input
                value={sshPubkey()}
                onInput={(e) => setSshPubkey(e.currentTarget.value)}
                placeholder="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."
              />
            </div>
            <Button
              size="sm"
              variant="secondary"
              icon={<Icon name="plus" size={13} />}
              onClick={() => {
                if (props.onRegisterSshKey) {
                  props.onRegisterSshKey(sshTitle().trim(), sshPubkey().trim());
                  setSshTitle("");
                  setSshPubkey("");
                } else {
                  props.onPolicyFeedback(
                    "Registered OpenSSH public key with SHA-256 fingerprint"
                  );
                }
              }}
            >
              Add SSH Key
            </Button>
          </div>

          <Show when={(props.liveSshKeys?.length ?? 0) > 0}>
            <For each={props.liveSshKeys}>
              {(key) => (
                <div
                  style={{
                    display: "flex",
                    "align-items": "center",
                    "justify-content": "space-between",
                    "font-size": "var(--ds-font-size-2xs)",
                  }}
                >
                  <span class="ds-mono">
                    {key.title} · {key.fingerprint_sha256}
                  </span>
                  <Show
                    when={!key.revoked}
                    fallback={<Badge variant="danger">Revoked</Badge>}
                  >
                    <Button
                      size="xs"
                      variant="ghost"
                      onClick={() => props.onRevokeSshKey?.(key.id)}
                    >
                      Revoke Key
                    </Button>
                  </Show>
                </div>
              )}
            </For>
          </Show>
        </div>
      </div>
    </div>
  );
};
