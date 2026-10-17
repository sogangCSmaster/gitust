import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createLiveAuthClient, type LiveAuthClientCallbacks } from "../src/showcase/live-auth-client";
let token: string | null;
let cb: LiveAuthClientCallbacks;
let fetchMock: ReturnType<typeof vi.fn>;
const response = (data: unknown, status = 200) => new Response(JSON.stringify(data), { status });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
beforeEach(() => {
  token = "session-a";
  vi.stubGlobal("window", { localStorage: {
    getItem: () => token, removeItem: () => { token = null; }, setItem: (_: string, value: string) => { token = value; },
  } });
  fetchMock = vi.fn().mockImplementation(async (url: string) => response(
    url.endsWith("/me") ? { user_id: "a", username: "a", email: "a@example.test", role: "Developer" } : []
  ));
  vi.stubGlobal("fetch", fetchMock);
  cb = {
    setLiveSession: vi.fn(), setEnterpriseUsers: vi.fn(), setLiveInvites: vi.fn(),
    setLastIssuedInviteUrl: vi.fn(), setLivePats: vi.fn(), setLastIssuedPatToken: vi.fn(),
    setLiveSshKeys: vi.fn(), setRedeemToken: vi.fn(), fallbackUsers: [],
    onTokenAuthenticated: vi.fn(), onTokenCleared: vi.fn(), showQuickFeedback: vi.fn(),
  };
});
afterEach(() => vi.unstubAllGlobals());
it("retains an issued PAT when a background auth refresh overlaps the mutation", async () => {
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const client = createLiveAuthClient(cb);
  const creating = client.createPat("key");
  await client.hydrateAuthData();
  pending.resolve(response({ raw_token: "new-one-time-token" }));
  await creating;
  expect(cb.setLastIssuedPatToken).toHaveBeenCalledWith("new-one-time-token");
});
it("does not claim remote session revocation after a failed logout", async () => {
  fetchMock.mockResolvedValue(response({}, 500));
  await createLiveAuthClient(cb).signOut();
  expect(token).toBeNull();
  expect(cb.showQuickFeedback).toHaveBeenLastCalledWith(expect.stringContaining("revocation could not be confirmed"));
});
it("does not restore a previous account's credentials after session replacement", async () => {
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const client = createLiveAuthClient(cb);
  const refresh = client.hydrateAuthData();
  token = "session-b";
  pending.resolve(response({ user_id: "a", username: "a", role: "Developer" }));
  await refresh;
  expect(cb.setLiveSession).not.toHaveBeenCalled();
});
it("shows active and suspended accounts using the server's lowercase status values", async () => {
  fetchMock.mockImplementation(async (url: string) => response(
    url.endsWith("/me") ? { user_id: "admin", username: "admin", email: "admin@example.test", role: "SiteAdmin" }
      : url.endsWith("/admin/users") ? [
        { id: "enabled", username: "enabled", email: "a@example.test", role: "Developer", status: "active" },
        { id: "disabled", username: "disabled", email: "b@example.test", role: "Developer", status: "suspended" },
      ] : []
  ));
  await createLiveAuthClient(cb).hydrateAuthData();
  expect(cb.setEnterpriseUsers).toHaveBeenLastCalledWith([
    expect.objectContaining({ liveId: "enabled", status: "active" }),
    expect.objectContaining({ liveId: "disabled", status: "suspended" }),
  ]);
});
it("shows a signed-in non-admin's own account as active", async () => {
  await createLiveAuthClient(cb).hydrateAuthData();
  expect(cb.setEnterpriseUsers).toHaveBeenLastCalledWith([
    expect.objectContaining({ liveId: "a", status: "active" }),
  ]);
});
it("clears the previous account's credentials and rejects its late PAT after a cross-tab replacement", async () => {
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const client = createLiveAuthClient(cb);
  const creating = client.createPat("old account key");
  token = "session-b";
  await client.syncStoredSession();
  expect(cb.setLiveSession).toHaveBeenCalledWith(null);
  expect(cb.setLastIssuedInviteUrl).toHaveBeenCalledWith(null);
  expect(cb.setLastIssuedPatToken).toHaveBeenCalledWith(null);
  expect(cb.setLiveInvites).toHaveBeenCalledWith([]);
  expect(cb.setLivePats).toHaveBeenCalledWith([]);
  expect(cb.setLiveSshKeys).toHaveBeenCalledWith([]);
  expect(cb.setEnterpriseUsers).toHaveBeenCalledWith([]);
  pending.resolve(response({ raw_token: "obsolete-synthetic-pat" }));
  await creating;
  expect(cb.setLastIssuedPatToken).not.toHaveBeenCalledWith("obsolete-synthetic-pat");
  expect(cb.onTokenAuthenticated).toHaveBeenCalledTimes(1);
});
