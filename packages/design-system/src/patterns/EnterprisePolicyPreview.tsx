import { Component, createSignal } from "solid-js";
import { Badge } from "../primitives/Badge";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";

export const EnterprisePolicyPreview: Component<{
  liveMode?: boolean;
  onPolicyFeedback: (message: string) => void;
}> = (props) => {
  const [inviteOnly, setInviteOnly] = createSignal(true);
  const [require2FA, setRequire2FA] = createSignal(true);
  const [ssoEnforced, setSsoEnforced] = createSignal(false);
  const [allowedDomain, setAllowedDomain] = createSignal("company.internal");

  const toggleSwitch = (
    label: string,
    getter: () => boolean,
    setter: (v: boolean) => void
  ) => {
    if (props.liveMode) return;
    const next = !getter();
    setter(next);
    props.onPolicyFeedback(
      `Demo policy preview: ${label} → ${next ? "ENABLED" : "DISABLED"} · not applied to the server`
    );
  };

  return (
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
              <span>Planned security policies · Demo</span>
            </span>
            <Badge variant="neutral">Not connected to server</Badge>
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
                disabled={props.liveMode}
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
                disabled={props.liveMode}
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
                disabled={props.liveMode}
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
                disabled={props.liveMode}
                value={allowedDomain()}
                onInput={(e) => setAllowedDomain(e.currentTarget.value)}
              />
            </div>
          </div>
        </div>
  );
};
