// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@solidjs/testing-library";
import { WorkbenchApp } from "../src/showcase/WorkbenchApp";
import { shortcutManager } from "../src/keyboard/shortcut-manager";

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
    else if (url.includes("/commits?")) body = { commits: [] };
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
  expect(screen.getByText("Browse only", { exact: true })).toBeTruthy();
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
  await screen.findByPlaceholderText("Search commands...");
  expect(screen.queryByText("Go to worktrees", { exact: true })).toBeNull();
  expect(screen.queryByText("Go to Actions", { exact: true })).toBeNull();
  expect(screen.queryByText("Create worktree", { exact: true })).toBeNull();
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
    screen.getByRole("button", { name: "Rename repository" }),
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
  expect(screen.getByRole("button", { name: "Clone repository" })).toBeTruthy();
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
