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
