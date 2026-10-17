// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { RepositoryFilesView } from "../src/showcase/RepositoryFilesView";
import { RepositoryChangesView } from "../src/showcase/RepositoryChangesView";
import { EnterpriseAdminView } from "../src/patterns/EnterpriseAdminView";

beforeEach(() => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("opens a selected file and returns to the preserved file search", async () => {
  const selected = vi.fn();
  const [path, setPath] = createSignal("Repository preview");
  render(() => (
    <RepositoryFilesView
      files={[
        { path: "src/one.rs", kind: "file" },
        { path: "src/two.rs", kind: "file" },
      ]}
      selectedFilePath={path()}
      lines={[]}
      onSelect={(file) => {
        selected(file);
        setPath(file.path);
      }}
    />
  ));
  await fireEvent.input(screen.getByLabelText("Search repository paths"), {
    target: { value: "two" },
  });
  const back = screen.getByRole("button", { name: "Back to files" });
  vi.spyOn(back, "getClientRects").mockReturnValue([
    {},
  ] as unknown as DOMRectList);
  await fireEvent.click(screen.getByRole("button", { name: "src/two.rs" }));
  expect(document.activeElement).toBe(back);
  expect(selected).toHaveBeenCalledWith({ path: "src/two.rs", kind: "file" });
  await fireEvent.click(screen.getByRole("button", { name: "Back to files" }));
  expect(
    (screen.getByLabelText("Search repository paths") as HTMLInputElement)
      .value,
  ).toBe("two");
  expect(screen.queryByRole("button", { name: "src/one.rs" })).toBeNull();
});

it("keeps the full comparison identity available without a row of commit buttons", async () => {
  const head = "a".repeat(40),
    base = "b".repeat(40);
  const selectCommit = vi.fn();
  render(() => (
    <RepositoryChangesView
      liveMode
      commits={[
        {
          sha: head,
          short_sha: "aaaaaaa",
          summary: "Selected change",
          author_name: "Developer",
        },
        {
          sha: base,
          short_sha: "bbbbbbb",
          summary: "Next change",
          author_name: "Developer",
        },
      ]}
      activeCommitRev={head}
      onSelectCommit={selectCommit}
      lookupMicros={null}
      review={{
        worktreeId: "task",
        branch: "agent/change",
        baseBranch: "main",
        headSha: head,
        baseSha: base,
        source: "verified",
        state: "ready",
      }}
      selectedFilePath="src/one.rs"
      lines={[]}
      files={[]}
      truncated={false}
      onSelectFile={vi.fn()}
    />
  ));
  expect(screen.getByLabelText("Select commit")).toBeTruthy();
  expect(
    screen.queryByRole("button", { name: /aaaaaaa Selected change/ }),
  ).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Comparison details" }),
  );
  expect(screen.getByText(head, { selector: "code" })).toBeTruthy();
  expect(screen.getByText(base, { selector: "code" })).toBeTruthy();
  expect(screen.getByText(/Uncommitted files are not included/)).toBeTruthy();
  await fireEvent.keyDown(screen.getByLabelText("Select commit"), {
    key: "ArrowDown",
  });
  await fireEvent.keyDown(screen.getByRole("option", { name: /Next change/ }), {
    key: "Enter",
  });
  expect(selectCommit).toHaveBeenCalledWith(base);
});

it("opens incoming recovery links on Account and clears one-time output even while its tab is hidden", async () => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const [redeemToken, setRedeemToken] = createSignal("");
  const [issuedToken, setIssuedToken] = createSignal<string | null>(
    "test-once-only",
  );
  const reset = vi.fn();
  render(() => (
    <EnterpriseAdminView
      users={[]}
      onToggleRole={vi.fn()}
      onToggleStatus={vi.fn()}
      onPolicyFeedback={vi.fn()}
      redeemMode="reset"
      redeemToken={redeemToken()}
      lastIssuedPatToken={issuedToken()}
      onRedeemPasswordReset={reset}
    />
  ));
  await fireEvent.click(
    screen.getByRole("tab", { name: "Credentials", exact: true }),
  );
  expect(screen.getByText("test-once-only")).toBeTruthy();
  setRedeemToken("local-recovery-fixture");
  expect(
    screen
      .getByRole("tab", { name: "Account", exact: true })
      .getAttribute("aria-selected"),
  ).toBe("true");
  await fireEvent.input(screen.getByLabelText("New password"), {
    target: { value: "new-test-password" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Reset password", exact: true }),
  );
  expect(reset).toHaveBeenCalledWith(
    "local-recovery-fixture",
    "new-test-password",
  );
  expect(
    (screen.getByLabelText("New password") as HTMLInputElement).value,
  ).toBe("");
  setIssuedToken(null);
  await fireEvent.click(
    screen.getByRole("tab", { name: "Credentials", exact: true }),
  );
  expect(screen.queryByText("test-once-only")).toBeNull();
  vi.restoreAllMocks();
});
