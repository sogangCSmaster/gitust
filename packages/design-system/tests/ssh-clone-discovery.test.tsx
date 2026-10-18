// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { EnterpriseCredentialsPanel } from "../src/patterns/EnterpriseCredentialsPanel";
import {
  createConnectedRepositoryClient,
  type ConnectedRepository,
  type ConnectedRepositoryState,
} from "../src/showcase/connected-repository-client";
import { RepositoryClone } from "../src/showcase/RepositoryClone";

const repoId = "11111111-1111-4111-8111-111111111111";
const secondaryId = "22222222-2222-4222-8222-222222222222";
const rawRepo = {
  id: repoId,
  name: "real-repo",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
};
const baseRepository: ConnectedRepository = {
  id: repoId,
  name: "real-repo",
  branch: "main",
  headCommit: "a".repeat(40),
  capability: "workbench",
  isDefault: true,
  canClone: true,
};
const response = (repositories: unknown[]) =>
  new Response(JSON.stringify({ repositories }));

beforeEach(() => {
  localStorage.setItem("gitust_token", "session-a");
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("accepts valid ssh_port metadata on /api/v1/repos and rejects malformed port hints", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(response([{ ...rawRepo, ssh_port: 2222 }]))
      .mockResolvedValueOnce(response([rawRepo]))
      .mockResolvedValueOnce(response([{ ...rawRepo, ssh_port: 0 }]))
      .mockResolvedValueOnce(response([{ ...rawRepo, ssh_port: 65536 }]))
      .mockResolvedValueOnce(response([{ ...rawRepo, ssh_port: "2222" }]))
      .mockResolvedValueOnce(response([{ ...rawRepo, ssh_port: null }])),
  );
  const states: ConnectedRepositoryState[] = [];
  const client = createConnectedRepositoryClient((state) => states.push(state));

  await client.refresh();
  expect(states.at(-1)?.repositories[0]).toMatchObject({
    id: repoId,
    sshPort: 2222,
  });

  await client.refresh();
  expect(states.at(-1)?.repositories[0]?.sshPort).toBeUndefined();

  for (let i = 0; i < 4; i++) {
    await client.refresh();
    expect(states.at(-1)?.repositories).toEqual([]);
    expect(states.at(-1)?.error).toContain("invalid repository metadata");
  }
});

it("shows only the HTTP clone URL and no protocol toggle when sshPort is omitted", async () => {
  render(() => (
    <RepositoryClone
      repository={baseRepository}
      liveMode
      begin={() => ({ repoId, isCurrent: () => true })}
    />
  ));
  await fireEvent.click(
    screen.getByRole("button", { name: "Clone repository" }),
  );
  const input = (await screen.findByRole("textbox", {
    name: "Clone URL",
  })) as HTMLInputElement;
  expect(input.value).toBe(`${window.location.origin}/git/${repoId}.git`);
  expect(screen.queryByRole("group", { name: "Clone protocol" })).toBeNull();
  expect(screen.queryByRole("button", { name: "SSH" })).toBeNull();
  expect(
    screen.getByText(
      "Use your Gitust username and a personal access token as the Git password.",
    ),
  ).toBeTruthy();
});

it("switches between HTTP and SSH clone URLs, copies the SSH URL, and resets on repository or session change", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  const [repository, setRepository] = createSignal<ConnectedRepository>({
    ...baseRepository,
    sshPort: 2222,
  });
  const [isCurrent, setIsCurrent] = createSignal(true);
  render(() => (
    <RepositoryClone
      repository={repository()}
      liveMode
      begin={() => ({
        repoId: repository().id,
        isCurrent,
      })}
    />
  ));

  await fireEvent.click(
    screen.getByRole("button", { name: "Clone repository" }),
  );
  const input = (await screen.findByRole("textbox", {
    name: "Clone URL",
  })) as HTMLInputElement;
  const httpButton = screen.getByRole("button", { name: "HTTP" });
  const sshButton = screen.getByRole("button", { name: "SSH" });
  expect(httpButton.getAttribute("aria-pressed")).toBe("true");
  expect(sshButton.getAttribute("aria-pressed")).toBe("false");
  expect(input.value).toBe(`${window.location.origin}/git/${repoId}.git`);

  await fireEvent.click(sshButton);
  expect(httpButton.getAttribute("aria-pressed")).toBe("false");
  expect(sshButton.getAttribute("aria-pressed")).toBe("true");
  const expectedSshUrl = `ssh://git@${window.location.hostname}:2222/${repoId}.git`;
  expect(input.value).toBe(expectedSshUrl);
  expect(
    screen.getByText(
      "Authenticate as git with a registered Ed25519 or ECDSA SSH key.",
    ),
  ).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Copy clone URL" }));
  await screen.findByRole("status");
  expect(screen.getByRole("status").textContent).toBe("Copied clone URL.");
  expect(writeText).toHaveBeenCalledExactlyOnceWith(expectedSshUrl);

  // Switching back to HTTP clears the copied notice and restores the HTTP URL
  await fireEvent.click(httpButton);
  expect(screen.queryByRole("status")).toBeNull();
  expect(input.value).toBe(`${window.location.origin}/git/${repoId}.git`);

  // Switching to SSH again and then removing sshPort falls back to HTTP
  await fireEvent.click(sshButton);
  expect(input.value).toBe(expectedSshUrl);
  setRepository({ ...baseRepository, sshPort: undefined });
  await waitFor(() =>
    expect(input.value).toBe(`${window.location.origin}/git/${repoId}.git`),
  );
  expect(screen.queryByRole("group", { name: "Clone protocol" })).toBeNull();

  // Changing repository or session closes the dialog cleanly
  setRepository({
    ...baseRepository,
    id: secondaryId,
    sshPort: 2222,
  });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

  // Reopen on secondary repository and verify session change closes dialog
  await fireEvent.click(
    screen.getByRole("button", { name: "Clone repository" }),
  );
  await screen.findByRole("dialog");
  setIsCurrent(false);
  window.dispatchEvent(new StorageEvent("storage", { key: "gitust_token" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

it("describes Ed25519 and ECDSA (nistp256/384/521) SSH key support in EnterpriseCredentialsPanel", () => {
  render(() => (
    <EnterpriseCredentialsPanel
      view="git"
      allowedDomain="example.test"
      redeemMode="invite"
      onChangeRedeemMode={vi.fn()}
      redeemToken=""
      onRedeemTokenInput={vi.fn()}
      onPolicyFeedback={vi.fn()}
    />
  ));
  expect(
    screen.getByText(/Ed25519 and ECDSA \(nistp256\/384\/521\) keys allow SSH/i),
  ).toBeTruthy();
  const keyInput = screen.getByRole("textbox", {
    name: "OpenSSH public key",
  }) as HTMLInputElement;
  expect(keyInput.placeholder).toContain("ecdsa-sha2-nistp256/384/521");
});
