// @vitest-environment jsdom
import { commitHistory } from "./fixtures/commit-history";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@solidjs/testing-library";
import { WorkbenchApp } from "../src/showcase/WorkbenchApp";
import { shortcutManager } from "../src/keyboard/shortcut-manager";

configure({ asyncUtilTimeout: 5000 });

const primary = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "agent-project",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
};
const secondary = {
  id: "22222222-2222-4222-8222-222222222222",
  name: "reference-library",
  branch: "trunk",
  head_commit: "b".repeat(40),
  capability: "browse",
  is_default: false,
};
let fetcher: ReturnType<typeof vi.fn>;
let catalog: unknown[];
beforeEach(() => {
  localStorage.setItem("gitust_token", "multi-session");
  catalog = [primary, secondary];
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn(() => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
    })),
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
  fetcher = vi.fn(async (url: string) => {
    let body: unknown = [];
    if (url.endsWith("/auth/me"))
      body = { user_id: "admin", username: "Admin", role: "SiteAdmin" };
    else if (url === "/api/v1/repos") body = { repositories: catalog };
    else if (url === "/api/v1/repo") body = primary;
    else if (url.includes("/branches?"))
      body = { branches: [], next_cursor: null };
    else if (url.includes("/commits?"))
      body = commitHistory(
        new URL(url, "https://example.invalid").searchParams.get("rev")!,
      );
    else if (url.includes("/tree?"))
      body = {
        commit_sha: url.includes(secondary.id)
          ? secondary.head_commit
          : primary.head_commit,
        entries: [{ path: "README.md", is_dir: false }],
      };
    else if (url.includes("/diff?"))
      body = {
        head_sha: url.includes(secondary.id)
          ? secondary.head_commit
          : primary.head_commit,
        base_sha: "c".repeat(40),
        files: [],
        lines: [],
      };
    return new Response(JSON.stringify(body));
  });
  vi.stubGlobal("fetch", fetcher);
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("opens a real browse-only repository without exposing primary worktree actions or commands", async () => {
  render(() => <WorkbenchApp />);
  await screen.findByRole("button", { name: "Open repository agent-project" });
  await fireEvent.input(
    screen.getByRole("textbox", { name: "Find a repository" }),
    { target: { value: "reference" } },
  );
  expect(
    screen.queryByRole("button", { name: "Open repository agent-project" }),
  ).toBeNull();
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository reference-library",
    }),
  );
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  expect(within(nav).queryByRole("button", { name: "Worktrees" })).toBeNull();
  expect(
    within(nav).queryByRole("button", { name: "Actions", exact: true }),
  ).toBeNull();
  expect(await screen.findByText("Read only", { exact: true })).toBeTruthy();
  expect(
    screen.queryByRole("button", { name: "Rename repository" }),
  ).toBeNull();
  expect(screen.queryByRole("button", { name: "Clone repository" })).toBeNull();
  const before = fetcher.mock.calls.length;
  expect(shortcutManager.executeById("nav.swarm")).toBe(false);
  expect(shortcutManager.executeById("nav.actions")).toBe(false);
  expect(shortcutManager.executeById("swarm.spawn")).toBe(false);
  expect(fetcher.mock.calls.length).toBe(before);
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Files", exact: true }),
  );
  await vi.dynamicImportSettled();
  await screen.findByLabelText("Select branch");
  await screen.findByText("README.md");
  expect(
    fetcher.mock.calls.some(([url]) =>
      url.startsWith(`/api/v1/repos/${secondary.id}/tree?`),
    ),
  ).toBe(true);
  expect(
    fetcher.mock.calls.some(
      ([url]) =>
        url.includes("/swarm/") ||
        url.includes("/repo/automatic-runs") ||
        url.includes("/sync/"),
    ),
  ).toBe(false);
  await fireEvent.click(
    screen.getByRole("button", { name: "Search commands", exact: true }),
  );
  await vi.dynamicImportSettled();
  await screen.findByRole("dialog", { name: "Command palette" });
  await vi.dynamicImportSettled();
  await screen.findByPlaceholderText("Search commands...");
  expect(screen.queryByText("Go to worktrees", { exact: true })).toBeNull();
  expect(screen.queryByText("Go to Actions", { exact: true })).toBeNull();
  expect(screen.queryByText("Create worktree", { exact: true })).toBeNull();
});

it("preserves an explicit branch snapshot across a confirmed default change and uses the new default only when selected", async () => {
  const pinned = "d".repeat(40),
    target = "e".repeat(40);
  const openKeyboardSelect = async (trigger: HTMLElement) => {
    trigger.focus();
    expect(document.activeElement).toBe(trigger);
    await fireEvent.keyDown(trigger, { key: "ArrowDown" });
  };
  let current = { ...primary, can_change_default_branch: true };
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string, init?: RequestInit) => {
    if (url === "/api/v1/repos")
      return new Response(
        JSON.stringify({ repositories: [current, secondary] }),
      );
    if (url.endsWith("/default-branch") && init?.method === "PATCH") {
      expect(JSON.parse(init.body as string)).toEqual({
        target_ref: "refs/heads/release",
        expected_current_ref: "refs/heads/main",
        expected_target_commit: target,
      });
      current = { ...current, branch: "release", head_commit: target };
      return new Response(
        JSON.stringify({
          id: primary.id,
          branch: "release",
          head_commit: target,
        }),
      );
    }
    if (url.includes("/branches?"))
      return new Response(
        JSON.stringify({
          branches: [
            {
              name: "release",
              ref_name: "refs/heads/release",
              commit_sha: target,
            },
            { name: "topic", ref_name: "refs/heads/topic", commit_sha: pinned },
          ],
          next_cursor: null,
        }),
      );
    if (url.includes("/tree?")) {
      const revision = new URL(url, "https://example.invalid").searchParams.get(
        "rev",
      );
      return new Response(
        JSON.stringify({
          commit_sha: revision,
          entries: [
            {
              path:
                revision === target
                  ? "RELEASE.txt"
                  : revision === pinned
                    ? "PINNED.txt"
                    : "README.md",
              is_dir: false,
            },
          ],
        }),
      );
    }
    return normalFetch(url, init);
  });
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository agent-project",
    }),
  );
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Files", exact: true }),
  );
  const branch = await screen.findByLabelText("Select branch");
  await screen.findByText("Snapshot aaaaaaaa. Select a branch to change it.");
  await openKeyboardSelect(branch);
  await fireEvent.click(
    await screen.findByRole("option", { name: "topic · dddddddd" }),
  );
  await screen.findByText("PINNED.txt");
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Overview", exact: true }),
  );
  const defaultTrigger = await screen.findByRole("button", {
    name: "Change default branch",
  });
  defaultTrigger.focus();
  expect(document.activeElement).toBe(defaultTrigger);
  await fireEvent.click(defaultTrigger);
  expect(screen.getByRole("dialog")).toBeTruthy();
  await vi.dynamicImportSettled();
  const candidate = await screen.findByLabelText("New default branch");
  await screen.findByText("Choose an existing committed branch.");
  await openKeyboardSelect(candidate);
  await fireEvent.click(
    await screen.findByRole("option", { name: "release · eeeeeeee" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Change default", exact: true }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(defaultTrigger));
  const restoredNav = await screen.findByRole("navigation", {
    name: "Repository views",
  });
  await fireEvent.click(
    within(restoredNav).getByRole("button", { name: "Files", exact: true }),
  );
  await screen.findByText("PINNED.txt");
  const restoredBranch = await screen.findByLabelText("Select branch");
  expect(restoredBranch.textContent).toContain("topic · dddddddd");
  await openKeyboardSelect(restoredBranch);
  await fireEvent.click(
    await screen.findByRole("option", { name: "Default (release)" }),
  );
  await screen.findByText("RELEASE.txt");
  expect(screen.getByLabelText("Select branch").textContent).toContain(
    "Default (release)",
  );
});

it("preserves sample worktrees while opening an explicitly labelled demo repository", async () => {
  localStorage.removeItem("gitust_token");
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Open repository gitust" }),
  );
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  expect(within(nav).getByText("6")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Clone repository" })).toBeNull();
});

it("opens unborn repositories with rename and HEAD guidance while preserving primary worktree commands", async () => {
  catalog = [
    { ...primary, head_commit: null, can_rename: true },
    { ...secondary, head_commit: null, can_rename: true },
  ];
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository reference-library",
    }),
  );
  expect(
    await screen.findByRole("button", { name: "Rename repository" }),
  ).toBeTruthy();
  expect(
    screen.getByRole("heading", { name: "No commits on the default branch" }),
  ).toBeTruthy();
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Files", exact: true }),
  );
  expect(
    screen.getByRole("button", { name: "Refresh repository" }),
  ).toBeTruthy();
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Changes", exact: true }),
  );
  expect(screen.queryByRole("combobox", { name: "Select commit" })).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Gitust repositories" }),
  );
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository agent-project",
    }),
  );
  expect(
    await screen.findByRole("button", { name: "Clone repository" }),
  ).toBeTruthy();
  const primaryNav = screen.getByRole("navigation", {
    name: "Repository views",
  });
  expect(
    within(primaryNav).getByRole("button", { name: "Actions", exact: true }),
  ).toBeTruthy();
  await fireEvent.click(
    within(primaryNav).getByRole("button", { name: "Worktrees", exact: true }),
  );
  await vi.dynamicImportSettled();
  expect(shortcutManager.executeById("swarm.spawn")).toBe(true);
  await vi.dynamicImportSettled();
  const base = (await screen.findByRole("textbox", {
    name: "Base branch",
  })) as HTMLInputElement;
  expect(base.value).toBe("");
  expect(
    fetcher.mock.calls.some(([url]) =>
      /\/(commits|tree|diff|blob)\?/.test(url),
    ),
  ).toBe(false);
  expect(
    fetcher.mock.calls.some(
      ([, options]) =>
        options?.method === "POST" && options?.body?.includes("base_branch"),
    ),
  ).toBe(false);
});

it("retries a failed HEAD view on explicit Refresh when the catalog commit has not changed", async () => {
  const previous = fetcher.getMockImplementation()!;
  let fail = true;
  fetcher.mockImplementation(async (url: string) => {
    if (url.includes("/tree?") && fail) {
      fail = false;
      return new Response("Temporary repository read failure", { status: 503 });
    }
    return previous(url);
  });
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository agent-project",
    }),
  );
  await screen.findByText(/Live repository hydration failed/);
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Worktrees", exact: true }),
  );
  await vi.dynamicImportSettled();
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh worktrees" }),
  );
  await waitFor(() =>
    expect(
      fetcher.mock.calls.filter(([url]) => url.includes("/tree?")),
    ).toHaveLength(2),
  );
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Files", exact: true }),
  );
  await screen.findByText("README.md");
});

it("browses a non-default branch snapshot when the default is unborn and shares it across Files and Changes", async () => {
  catalog = [{ ...secondary, head_commit: null }];
  const topic = "d".repeat(40);
  const nextTip = "e".repeat(40);
  let tip = topic;
  const previous = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string) => {
    if (url.includes("/branches?"))
      return new Response(
        JSON.stringify({
          branches: [
            { name: "topic", ref_name: "refs/heads/topic", commit_sha: tip },
          ],
          next_cursor: null,
        }),
      );
    if (url.includes("/tree?"))
      return new Response(
        JSON.stringify({
          commit_sha: topic,
          entries: [{ path: "TOPIC.txt", is_dir: false }],
        }),
      );
    if (url.includes("/blob?"))
      return new Response(
        JSON.stringify({ commit_sha: topic, content: "TOPIC CONTENT" }),
      );
    if (url.includes("/diff?"))
      return new Response(
        JSON.stringify({
          head_sha: topic,
          base_sha: "c".repeat(40),
          files: [],
          lines: [],
        }),
      );
    return previous(url);
  });
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository reference-library",
    }),
  );
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Files", exact: true }),
  );
  const selector = await screen.findByLabelText("Select branch");
  await screen.findByText("Snapshot —. Select a branch to change it.");
  await fireEvent.keyDown(selector, { key: "ArrowDown" });
  await fireEvent.click(
    await screen.findByRole("option", { name: "topic · dddddddd" }),
  );
  await screen.findByText("TOPIC.txt");
  await fireEvent.click(screen.getByText("TOPIC.txt"));
  await screen.findByText("TOPIC CONTENT");
  const calls = fetcher.mock.calls.filter(([url]) =>
    /\/(commits|tree|diff|blob)\?/.test(url),
  );
  expect(calls.length).toBeGreaterThan(3);
  for (const [url] of calls) {
    const query = new URL(url, "https://example.invalid").searchParams;
    expect(query.get("rev") ?? query.get("head")).toBe(topic);
  }
  await fireEvent.click(
    within(nav).getByRole("button", { name: "Changes", exact: true }),
  );
  expect((await screen.findByLabelText("Select branch")).textContent).toContain(
    "topic · dddddddd",
  );
  const count = calls.length;
  tip = nextTip;
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh branch list" }),
  );
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "Refresh branch list" })
        .hasAttribute("disabled"),
    ).toBe(false),
  );
  await waitFor(() =>
    expect(screen.getByLabelText("Select branch").textContent).toContain(
      "topic · dddddddd",
    ),
  );
  expect(
    fetcher.mock.calls.filter(([url]) =>
      /\/(commits|tree|diff|blob)\?/.test(url),
    ),
  ).toHaveLength(count);
  await fireEvent.keyDown(screen.getByLabelText("Select branch"), {
    key: "ArrowDown",
  });
  expect(
    await screen.findByRole("option", { name: "topic · eeeeeeee" }),
  ).toBeTruthy();
  await fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
  expect(within(nav).queryByRole("button", { name: "Worktrees" })).toBeNull();
});
