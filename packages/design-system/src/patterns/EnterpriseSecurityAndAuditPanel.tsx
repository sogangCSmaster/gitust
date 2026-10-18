import {
  createEffect,
  createSignal,
  For,
  onCleanup,
  Show,
  type Component,
} from "solid-js";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Icon } from "../components/workspace/icon";
import {
  deleteSsoProvider,
  disableTotp,
  enrollTotp,
  fetchAuditLog,
  fetchSsoProviders,
  fetchTotpStatus,
  registerSsoProvider,
  verifyTotpEnrollment,
  type LiveAuditLogResponse,
  type LiveSsoProtocol,
  type LiveSsoProviderRecord,
  type LiveTotpEnrollmentChallenge,
  type LiveTotpStatus,
} from "../showcase/live-enterprise-security-client";

export interface EnterpriseSecurityAndAuditPanelProps {
  role?: "SiteAdmin" | "Maintainer" | "Developer";
  onPolicyFeedback?: (msg: string) => void;
}

export const EnterpriseSecurityAndAuditPanel: Component<
  EnterpriseSecurityAndAuditPanelProps
> = (props) => {
  const [totpStatus, setTotpStatus] = createSignal<LiveTotpStatus | null>(null);
  const [challenge, setChallenge] =
    createSignal<LiveTotpEnrollmentChallenge | null>(null);
  const [verifyCode, setVerifyCode] = createSignal("");
  const [disableCode, setDisableCode] = createSignal("");
  const [ssoProviders, setSsoProviders] = createSignal<LiveSsoProviderRecord[]>(
    [],
  );
  const [auditLog, setAuditLog] = createSignal<LiveAuditLogResponse | null>(
    null,
  );
  const [loading, setLoading] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const [ssoName, setSsoName] = createSignal("");
  const [ssoProtocol, setSsoProtocol] = createSignal<LiveSsoProtocol>("oidc");
  const [ssoIssuer, setSsoIssuer] = createSignal("");
  const [ssoClientId, setSsoClientId] = createSignal("");
  const [ssoDomains, setSsoDomains] = createSignal("");
  const [ssoSecret, setSsoSecret] = createSignal("");

  let requestSeq = 0;

  const loadAll = async (role?: string, signal?: AbortSignal) => {
    const seq = ++requestSeq;
    setLoading(true);
    setError(null);
    try {
      const isAdmin = role === "SiteAdmin";
      const [totp, providers, audit] = await Promise.all([
        fetchTotpStatus(signal),
        isAdmin ? fetchSsoProviders(signal) : Promise.resolve([]),
        isAdmin ? fetchAuditLog(signal) : Promise.resolve(null),
      ]);
      if (seq !== requestSeq) return;
      setTotpStatus(totp);
      setSsoProviders(providers);
      setAuditLog(audit);
    } catch (err) {
      if (seq !== requestSeq || signal?.aborted) return;
      setError(
        err instanceof Error ? err.message : "Failed to load security state",
      );
    } finally {
      if (seq === requestSeq) setLoading(false);
    }
  };

  createEffect(() => {
    const role = props.role;
    const ctrl = new AbortController();
    onCleanup(() => {
      requestSeq++;
      ctrl.abort();
    });
    void loadAll(role, ctrl.signal);
  });

  const refreshAuditIfAdmin = async (expectedSeq: number) => {
    if (props.role !== "SiteAdmin") return;
    try {
      const updated = await fetchAuditLog();
      if (expectedSeq !== requestSeq) return;
      setAuditLog(updated);
    } catch {
      // Ignore background audit refresh error
    }
  };

  const handleEnrollTotp = async () => {
    if (busy()) return;
    const seq = requestSeq;
    setBusy(true);
    setError(null);
    try {
      const ch = await enrollTotp();
      if (seq !== requestSeq) return;
      setChallenge(ch);
      setTotpStatus({
        enrolled: true,
        verified: false,
        enrolled_at: null,
        last_used_step: null,
      });
      props.onPolicyFeedback?.("Generated RFC 6238 TOTP secret challenge");
      void refreshAuditIfAdmin(seq);
    } catch (err) {
      if (seq !== requestSeq) return;
      setError(err instanceof Error ? err.message : "TOTP enrollment failed");
    } finally {
      if (seq === requestSeq) setBusy(false);
    }
  };

  const handleVerifyTotp = async (e: Event) => {
    e.preventDefault();
    const code = verifyCode().trim();
    if (!code || busy()) return;
    const seq = requestSeq;
    setBusy(true);
    setError(null);
    try {
      const status = await verifyTotpEnrollment(code);
      if (seq !== requestSeq) return;
      setTotpStatus(status);
      setChallenge(null);
      setVerifyCode("");
      props.onPolicyFeedback?.("2FA (TOTP) verified and activated");
      void refreshAuditIfAdmin(seq);
    } catch (err) {
      if (seq !== requestSeq) return;
      setError(err instanceof Error ? err.message : "TOTP verification failed");
    } finally {
      if (seq === requestSeq) setBusy(false);
    }
  };

  const handleDisableTotp = async () => {
    if (busy()) return;
    const seq = requestSeq;
    setBusy(true);
    setError(null);
    try {
      const code = disableCode().trim() || undefined;
      const status = await disableTotp(code);
      if (seq !== requestSeq) return;
      setTotpStatus(status);
      setChallenge(null);
      setDisableCode("");
      props.onPolicyFeedback?.("2FA (TOTP) disabled");
      void refreshAuditIfAdmin(seq);
    } catch (err) {
      if (seq !== requestSeq) return;
      setError(err instanceof Error ? err.message : "Unable to disable 2FA");
    } finally {
      if (seq === requestSeq) setBusy(false);
    }
  };

  const handleRegisterSso = async (e: Event) => {
    e.preventDefault();
    const name = ssoName().trim();
    const issuer_or_url = ssoIssuer().trim();
    const client_id = ssoClientId().trim();
    const signing_secret = ssoSecret().trim();
    const allowed_domains = ssoDomains()
      .split(",")
      .map((d) => d.trim())
      .filter(Boolean);
    if (
      !name ||
      !issuer_or_url ||
      !client_id ||
      !signing_secret ||
      allowed_domains.length === 0 ||
      busy()
    ) {
      return;
    }
    const seq = requestSeq;
    setBusy(true);
    setError(null);
    try {
      const created = await registerSsoProvider({
        name,
        protocol: ssoProtocol(),
        issuer_or_url,
        client_id,
        allowed_domains,
        signing_secret,
      });
      if (seq !== requestSeq) return;
      setSsoProviders((prev) => [...prev, created]);
      setSsoName("");
      setSsoIssuer("");
      setSsoClientId("");
      setSsoDomains("");
      setSsoSecret("");
      props.onPolicyFeedback?.(`Registered SSO provider '${created.name}'`);
      void refreshAuditIfAdmin(seq);
    } catch (err) {
      if (seq !== requestSeq) return;
      setError(
        err instanceof Error ? err.message : "SSO provider registration failed",
      );
    } finally {
      if (seq === requestSeq) setBusy(false);
    }
  };

  const handleDeleteSso = async (provider: LiveSsoProviderRecord) => {
    if (busy()) return;
    const seq = requestSeq;
    setBusy(true);
    setError(null);
    try {
      await deleteSsoProvider(provider.id);
      if (seq !== requestSeq) return;
      setSsoProviders((prev) => prev.filter((p) => p.id !== provider.id));
      props.onPolicyFeedback?.(`Deleted SSO provider '${provider.name}'`);
      void refreshAuditIfAdmin(seq);
    } catch (err) {
      if (seq !== requestSeq) return;
      setError(
        err instanceof Error ? err.message : "Failed to delete SSO provider",
      );
    } finally {
      if (seq === requestSeq) setBusy(false);
    }
  };

  return (
    <div
      class="space-y-4"
      data-testid="enterprise-security-audit-panel"
    >
      <Show when={error()}>
        {(msg) => (
          <div
            role="alert"
            class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {msg()}
          </div>
        )}
      </Show>

      {/* Two-Factor Authentication (TOTP / RFC 6238) */}
      <div class="rounded-lg border border-border bg-card p-3 space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="flex items-center gap-2">
            <Icon name="shield-check" size={14} class="text-primary" />
            <h4 class="text-xs font-semibold text-foreground">
              Two-Factor Authentication (TOTP / RFC 6238)
            </h4>
          </div>
          <Badge
            variant={totpStatus()?.verified ? "success" : "outline"}
            class="text-[10px]"
          >
            {totpStatus() === null && loading()
              ? "Loading 2FA..."
              : totpStatus()?.verified
                ? "2FA Verified"
                : totpStatus()?.enrolled
                  ? "Enrollment Pending"
                  : "Not Enrolled"}
          </Badge>
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy()}
            onClick={handleEnrollTotp}
          >
            <Icon name="key" size={12} />
            Enroll 2FA (TOTP)
          </Button>
          <Show when={totpStatus()?.enrolled || totpStatus()?.verified}>
            <input
              type="text"
              aria-label="Disable TOTP code"
              value={disableCode()}
              onInput={(e) => setDisableCode(e.currentTarget.value)}
              placeholder="6-digit TOTP code"
              class="h-8 w-36 rounded-md border border-input bg-background px-2 font-mono text-xs text-foreground"
            />
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy()}
              onClick={handleDisableTotp}
            >
              <Icon name="x" size={12} />
              Disable 2FA
            </Button>
          </Show>
        </div>

        <Show when={challenge()}>
          {(ch) => (
            <form
              onSubmit={handleVerifyTotp}
              class="rounded-md border border-border/70 bg-muted/20 p-2.5 space-y-2 text-xs"
            >
              <div class="space-y-1">
                <div class="text-muted-foreground">Base32 Secret:</div>
                <code class="block rounded bg-background px-2 py-1 font-mono text-[11px] text-foreground break-all">
                  {ch().secret_base32}
                </code>
                <div class="text-muted-foreground">OTPAuth URI:</div>
                <code class="block rounded bg-background px-2 py-1 font-mono text-[10px] text-muted-foreground break-all">
                  {ch().otpauth_uri}
                </code>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <input
                  type="text"
                  aria-label="TOTP verification code"
                  value={verifyCode()}
                  onInput={(e) => setVerifyCode(e.currentTarget.value)}
                  placeholder="Enter 6-digit code"
                  class="h-8 w-40 rounded-md border border-input bg-background px-2 font-mono text-xs text-foreground"
                />
                <Button type="submit" size="sm" disabled={busy()}>
                  <Icon name="check" size={12} />
                  Verify & Activate 2FA
                </Button>
              </div>
            </form>
          )}
        </Show>
      </div>

      {/* SiteAdmin: Enterprise SSO Providers (OIDC / LDAP) & Audit Trail */}
      <Show when={props.role === "SiteAdmin"}>
        <div class="rounded-lg border border-border bg-card p-3 space-y-3">
          <div class="flex items-center justify-between gap-2">
            <div class="flex items-center gap-2">
              <Icon name="lock" size={14} class="text-primary" />
              <h4 class="text-xs font-semibold text-foreground">
                Enterprise SSO Providers (OIDC / LDAP)
              </h4>
            </div>
            <Badge variant="outline" class="text-[10px] font-mono">
              {ssoProviders().length} configured
            </Badge>
          </div>

          <form
            onSubmit={handleRegisterSso}
            class="grid gap-2 rounded-md border border-border/70 bg-muted/20 p-2.5 sm:grid-cols-3"
          >
            <input
              type="text"
              aria-label="SSO provider name"
              value={ssoName()}
              onInput={(e) => setSsoName(e.currentTarget.value)}
              placeholder="Okta / Corp LDAP"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs text-foreground"
            />
            <select
              aria-label="SSO protocol"
              value={ssoProtocol()}
              onChange={(e) =>
                setSsoProtocol(e.currentTarget.value as LiveSsoProtocol)
              }
              class="h-8 rounded-md border border-input bg-background px-2 text-xs text-foreground"
            >
              <option value="oidc">OIDC</option>
              <option value="ldap">LDAP</option>
            </select>
            <input
              type="text"
              aria-label="Issuer or LDAP URL"
              value={ssoIssuer()}
              onInput={(e) => setSsoIssuer(e.currentTarget.value)}
              placeholder="https://idp.corp.internal"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs text-foreground"
            />
            <input
              type="text"
              aria-label="SSO client ID"
              value={ssoClientId()}
              onInput={(e) => setSsoClientId(e.currentTarget.value)}
              placeholder="Client ID or Bind DN"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs text-foreground"
            />
            <input
              type="text"
              aria-label="Allowed domains"
              value={ssoDomains()}
              onInput={(e) => setSsoDomains(e.currentTarget.value)}
              placeholder="corp.internal, eng.internal"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs text-foreground"
            />
            <div class="flex items-center gap-2">
              <input
                type="password"
                aria-label="SSO signing secret"
                value={ssoSecret()}
                onInput={(e) => setSsoSecret(e.currentTarget.value)}
                placeholder="Signing secret"
                class="h-8 flex-1 rounded-md border border-input bg-background px-2 text-xs text-foreground"
              />
              <Button type="submit" size="sm" disabled={busy()}>
                <Icon name="plus" size={12} />
                Register SSO provider
              </Button>
            </div>
          </form>

          <div class="space-y-1.5">
            <For each={ssoProviders()}>
              {(provider) => (
                <div class="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border/80 bg-background px-3 py-2 text-xs">
                  <div class="flex flex-wrap items-center gap-2">
                    <span class="font-medium text-foreground">
                      {provider.name}
                    </span>
                    <Badge variant="secondary" class="text-[10px] font-mono">
                      {provider.protocol.toUpperCase()}
                    </Badge>
                    <Show when={provider.require_invite_binding}>
                      <Badge variant="outline" class="text-[10px]">
                        Invite-Bound
                      </Badge>
                    </Show>
                    <span class="font-mono text-[11px] text-muted-foreground">
                      {provider.issuer_or_url}
                    </span>
                    <span class="text-[11px] text-muted-foreground">
                      ({provider.allowed_domains.join(", ")})
                    </span>
                  </div>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    aria-label={`Delete SSO provider ${provider.name}`}
                    disabled={busy()}
                    onClick={() => handleDeleteSso(provider)}
                  >
                    <Icon name="x" size={12} />
                    Delete
                  </Button>
                </div>
              )}
            </For>
          </div>
        </div>

        <div class="rounded-lg border border-border bg-card p-3 space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex items-center gap-2">
              <Icon name="layers" size={14} class="text-primary" />
              <h4 class="text-xs font-semibold text-foreground">
                Tamper-Evident Audit Trail (SHA-256 Hash Chain)
              </h4>
            </div>
            <Show when={auditLog()}>
              {(log) => (
                <div class="flex flex-wrap items-center gap-2">
                  <Badge
                    variant={
                      log().verification.verified ? "success" : "error"
                    }
                    class="text-[10px] font-mono"
                  >
                    {log().verification.verified
                      ? `Chain Verified (${log().verification.entries_checked} entries)`
                      : `Chain Tampered (seq #${log().verification.first_invalid_seq})`}
                  </Badge>
                  <span class="font-mono text-[10px] text-muted-foreground">
                    head: {log().verification.head_hash.slice(0, 12)}
                  </span>
                </div>
              )}
            </Show>
          </div>

          <Show when={auditLog()}>
            {(log) => (
              <div class="space-y-1.5">
                <For each={log().entries}>
                  {(entry) => (
                    <div class="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border/70 bg-background px-2.5 py-1.5 text-xs">
                      <div class="flex flex-wrap items-center gap-2">
                        <span class="font-mono text-[10px] text-muted-foreground">
                          #{entry.seq}
                        </span>
                        <Badge variant="outline" class="text-[10px] font-mono">
                          {entry.action}
                        </Badge>
                        <span class="font-medium text-foreground">
                          @{entry.actor_username}
                        </span>
                        <span class="text-muted-foreground">→</span>
                        <span class="font-mono text-[11px] text-muted-foreground">
                          {entry.target}
                        </span>
                      </div>
                      <span class="font-mono text-[10px] text-muted-foreground">
                        {entry.entry_hash.slice(0, 12)} ←{" "}
                        {entry.prev_hash.slice(0, 12)}
                      </span>
                    </div>
                  )}
                </For>
              </div>
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
};

export default EnterpriseSecurityAndAuditPanel;
