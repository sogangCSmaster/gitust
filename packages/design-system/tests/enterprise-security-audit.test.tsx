// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IconSpriteSheet } from "../src/components/workspace/icon";
import { EnterpriseAdminView } from "../src/patterns/EnterpriseAdminView";
import { EnterpriseSecurityAndAuditPanel } from "../src/patterns/EnterpriseSecurityAndAuditPanel";
import {
  type LiveAuditEventRecord,
  type LiveAuditLogResponse,
  type LiveSsoProviderRecord,
  type LiveTotpEnrollmentChallenge,
  type LiveTotpStatus,
} from "../src/showcase/live-enterprise-security-client";

const makeTotpStatus = (overrides: Partial<LiveTotpStatus> = {}): LiveTotpStatus => ({
  enrolled: false,
  verified: false,
  enrolled_at: null,
  last_used_step: null,
  ...overrides,
});

const makeTotpChallenge = (
  overrides: Partial<LiveTotpEnrollmentChallenge> = {},
): LiveTotpEnrollmentChallenge => ({
  secret_base32: "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP",
  otpauth_uri:
    "otpauth://totp/Gitust:alice%40corp.internal?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=Gitust&digits=6&period=30",
  digits: 6,
  period_secs: 30,
  ...overrides,
});

const makeSsoProvider = (
  overrides: Partial<LiveSsoProviderRecord> = {},
): LiveSsoProviderRecord => ({
  id: "sso-1",
  name: "Okta Workforce OIDC",
  protocol: "oidc",
  issuer_or_url: "https://login.corp.internal/oauth2/default",
  client_id: "gitust-enterprise-web",
  allowed_domains: ["corp.internal", "eng.corp.internal"],
  enabled: true,
  require_invite_binding: true,
  created_at: "1760000000",
  ...overrides,
});

const makeAuditEntry = (
  overrides: Partial<LiveAuditEventRecord> = {},
): LiveAuditEventRecord => ({
  seq: 1,
  timestamp: "1760000100",
  actor_user_id: "u-admin",
  actor_username: "alice",
  action: "auth.totp.verified",
  target: "user:alice",
  prev_hash: "0000000000000000000000000000000000000000000000000000000000000000",
  entry_hash: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
  ...overrides,
});

describe("Slice 9 (Phase 4.4) — SSO/OIDC/LDAP, TOTP 2FA, and Tamper-Evident Audit Trail", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "test-live-session-token");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("enrolls, verifies, and disables RFC 6238 TOTP 2FA", async () => {
    let currentTotp = makeTotpStatus({ enrolled: false, verified: false });
    const challenge = makeTotpChallenge();

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.toString();
      const method = init?.method ?? "GET";

      if (url === "/api/v1/auth/totp/status" && method === "GET") {
        return new Response(JSON.stringify(currentTotp), { status: 200 });
      }
      if (url === "/api/v1/auth/totp/enroll" && method === "POST") {
        currentTotp = makeTotpStatus({
          enrolled: true,
          verified: false,
          enrolled_at: "1760000050",
        });
        return new Response(JSON.stringify(challenge), { status: 200 });
      }
      if (url === "/api/v1/auth/totp/verify" && method === "POST") {
        const body = JSON.parse(String(init?.body ?? "{}"));
        expect(body.code).toBe("482910");
        currentTotp = makeTotpStatus({
          enrolled: true,
          verified: true,
          enrolled_at: "1760000050",
          last_used_step: 58666668,
        });
        return new Response(JSON.stringify(currentTotp), { status: 200 });
      }
      if (url === "/api/v1/auth/totp" && method === "DELETE") {
        const body = JSON.parse(String(init?.body ?? "{}"));
        expect(body.code).toBe("918273");
        currentTotp = makeTotpStatus({ enrolled: false, verified: false });
        return new Response(JSON.stringify(currentTotp), { status: 200 });
      }
      return new Response("Not Found", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const feedback = vi.fn();
    render(() => (
      <>
        <IconSpriteSheet />
        <EnterpriseSecurityAndAuditPanel role="Developer" onPolicyFeedback={feedback} />
      </>
    ));

    await waitFor(() => {
      expect(screen.getByText("Not Enrolled")).toBeTruthy();
    });

    // Click Enroll 2FA (TOTP)
    fireEvent.click(screen.getByRole("button", { name: /Enroll 2FA \(TOTP\)/i }));

    await waitFor(() => {
      expect(screen.getByText(challenge.secret_base32)).toBeTruthy();
      expect(screen.getByText(challenge.otpauth_uri)).toBeTruthy();
    });

    const codeInput = screen.getByLabelText("TOTP verification code") as HTMLInputElement;
    fireEvent.input(codeInput, { target: { value: "482910" } });
    fireEvent.click(screen.getByRole("button", { name: /Verify & Activate 2FA/i }));

    await waitFor(() => {
      expect(screen.getByText("2FA Verified")).toBeTruthy();
    });

    // Disable 2FA
    const disableInput = screen.getByLabelText("Disable TOTP code") as HTMLInputElement;
    fireEvent.input(disableInput, { target: { value: "918273" } });
    fireEvent.click(screen.getByRole("button", { name: /Disable 2FA/i }));

    await waitFor(() => {
      expect(screen.getByText("Not Enrolled")).toBeTruthy();
    });
  });

  it("registers and deletes OIDC/LDAP SSO providers and renders verified SHA-256 audit chain for SiteAdmin", async () => {
    let providers: LiveSsoProviderRecord[] = [makeSsoProvider()];
    const auditLog: LiveAuditLogResponse = {
      entries: [
        makeAuditEntry({
          seq: 1,
          actor_username: "alice",
          action: "sso.provider.registered",
          target: "sso:Okta Workforce OIDC",
          prev_hash: "0000000000000000000000000000000000000000000000000000000000000000",
          entry_hash: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
        }),
        makeAuditEntry({
          seq: 2,
          actor_username: "alice",
          action: "auth.totp.verified",
          target: "user:alice",
          prev_hash: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
          entry_hash: "f9e8d7c6b5a4938271605f4e3d2c1b0af9e8d7c6b5a4938271605f4e3d2c1b0a",
        }),
      ],
      verification: {
        verified: true,
        entries_checked: 2,
        head_hash: "f9e8d7c6b5a4938271605f4e3d2c1b0af9e8d7c6b5a4938271605f4e3d2c1b0a",
        first_invalid_seq: null,
        reason: null,
      },
    };

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.toString();
      const method = init?.method ?? "GET";

      if (url === "/api/v1/auth/totp/status" && method === "GET") {
        return new Response(
          JSON.stringify(makeTotpStatus({ enrolled: true, verified: true })),
          { status: 200 },
        );
      }
      if (url === "/api/v1/admin/sso/providers" && method === "GET") {
        return new Response(JSON.stringify({ providers }), { status: 200 });
      }
      if (url === "/api/v1/admin/sso/providers" && method === "POST") {
        const body = JSON.parse(String(init?.body ?? "{}"));
        const created = makeSsoProvider({
          id: "sso-2",
          name: body.name,
          protocol: body.protocol,
          issuer_or_url: body.issuer_or_url,
          client_id: body.client_id,
          allowed_domains: body.allowed_domains,
        });
        providers = [...providers, created];
        return new Response(JSON.stringify(created), { status: 201 });
      }
      if (url === "/api/v1/admin/sso/providers/sso-1" && method === "DELETE") {
        providers = providers.filter((p) => p.id !== "sso-1");
        return new Response(null, { status: 204 });
      }
      if (url === "/api/v1/admin/audit-log" && method === "GET") {
        return new Response(JSON.stringify(auditLog), { status: 200 });
      }
      return new Response("Not Found", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    render(() => (
      <>
        <IconSpriteSheet />
        <EnterpriseSecurityAndAuditPanel role="SiteAdmin" />
      </>
    ));

    // Existing SSO provider and verified audit chain should load
    await waitFor(() => {
      expect(screen.getByText("Okta Workforce OIDC")).toBeTruthy();
      expect(screen.getAllByText("OIDC").length).toBeGreaterThanOrEqual(2);
      expect(screen.getByText("Invite-Bound")).toBeTruthy();
      expect(screen.getByText("Chain Verified (2 entries)")).toBeTruthy();
      expect(screen.getByText(/head: f9e8d7c6b5a4/)).toBeTruthy();
      expect(screen.getByText("sso.provider.registered")).toBeTruthy();
      expect(screen.getByText(/f9e8d7c6b5a4 ← a1b2c3d4e5f6/)).toBeTruthy();
    });

    // Register a new LDAP provider
    fireEvent.input(screen.getByLabelText("SSO provider name"), {
      target: { value: "Corp OpenLDAP" },
    });
    fireEvent.change(screen.getByLabelText("SSO protocol"), {
      target: { value: "ldap" },
    });
    fireEvent.input(screen.getByLabelText("Issuer or LDAP URL"), {
      target: { value: "ldaps://ldap.corp.internal:636" },
    });
    fireEvent.input(screen.getByLabelText("SSO client ID"), {
      target: { value: "cn=gitust-bind,dc=corp,dc=internal" },
    });
    fireEvent.input(screen.getByLabelText("Allowed domains"), {
      target: { value: "corp.internal, ldap.corp.internal" },
    });
    fireEvent.input(screen.getByLabelText("SSO signing secret"), {
      target: { value: "super-secret-ldap-hmac-key" },
    });

    fireEvent.click(screen.getByRole("button", { name: /Register SSO provider/i }));

    await waitFor(() => {
      expect(screen.getByText("Corp OpenLDAP")).toBeTruthy();
      expect(screen.getAllByText("LDAP").length).toBeGreaterThanOrEqual(2);
      expect(screen.getByText("ldaps://ldap.corp.internal:636")).toBeTruthy();
    });

    // Delete initial provider
    fireEvent.click(
      screen.getByRole("button", { name: /Delete SSO provider Okta Workforce OIDC/i }),
    );

    await waitFor(() => {
      expect(screen.queryByText("Okta Workforce OIDC")).toBeNull();
    });
  });

  it("alerts SiteAdmin when the SHA-256 audit log chain is tampered and integrates into EnterpriseAdminView", async () => {
    const tamperedAuditLog: LiveAuditLogResponse = {
      entries: [
        makeAuditEntry({
          seq: 1,
          action: "user.login",
          target: "user:alice",
        }),
        makeAuditEntry({
          seq: 2,
          action: "repo.deleted",
          target: "repo:critical",
          prev_hash: "bad0000000000000000000000000000000000000000000000000000000000000",
          entry_hash: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        }),
      ],
      verification: {
        verified: false,
        entries_checked: 2,
        head_hash: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        first_invalid_seq: 2,
        reason: "prev_hash mismatch at seq #2",
      },
    };

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url === "/api/v1/auth/totp/status") {
        return new Response(JSON.stringify(makeTotpStatus()), { status: 200 });
      }
      if (url === "/api/v1/admin/sso/providers") {
        return new Response(JSON.stringify({ providers: [] }), { status: 200 });
      }
      if (url === "/api/v1/admin/audit-log") {
        return new Response(JSON.stringify(tamperedAuditLog), { status: 200 });
      }
      return new Response("Not Found", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const onSignIn = vi.fn();
    render(() => (
      <>
        <IconSpriteSheet />
        <EnterpriseAdminView
          users={[]}
          onToggleRole={() => {}}
          onToggleStatus={() => {}}
          onPolicyFeedback={() => {}}
          liveMode={true}
          liveSession={null}
          onSignIn={onSignIn}
        />
      </>
    ));

    // Verify 2FA code input in sign-in form
    fireEvent.input(screen.getByLabelText("Username or email"), {
      target: { value: "alice" },
    });
    fireEvent.input(screen.getByLabelText("Password"), {
      target: { value: "correct-horse-battery-staple" },
    });
    fireEvent.input(screen.getByLabelText("2FA code (if enabled)"), {
      target: { value: "123456" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Sign in$/i }));
    expect(onSignIn).toHaveBeenCalledWith(
      "alice",
      "correct-horse-battery-staple",
      "123456",
    );

    cleanup();

    // Now render EnterpriseAdminView with an active SiteAdmin session and switch to Security & Audit tab
    render(() => (
      <>
        <IconSpriteSheet />
        <EnterpriseAdminView
          users={[]}
          onToggleRole={() => {}}
          onToggleStatus={() => {}}
          onPolicyFeedback={() => {}}
          liveMode={true}
          liveSession={{
            user_id: "u-admin",
            username: "alice",
            email: "alice@corp.internal",
            role: "SiteAdmin",
          }}
        />
      </>
    ));

    const securityTab = screen.getByRole("tab", { name: /Security & Audit/i });
    fireEvent.click(securityTab);

    await waitFor(() => {
      expect(screen.getByText("Chain Tampered (seq #2)")).toBeTruthy();
      expect(screen.getByText(/head: deadbeefdead/)).toBeTruthy();
    });
  });
});
