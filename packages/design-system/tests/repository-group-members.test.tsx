// @vitest-environment jsdom
import { createSignal, Show } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { RepositoryOverview } from "../src/showcase/RepositoryOverview";
import { createRepositoryWorkspace } from "../src/showcase/repository-workspace";
import { RepositoryVisibility } from "../src/showcase/RepositoryVisibility";
const repo = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "primary",
  branch: "main",
  headCommit: "a".repeat(40),
  capability: "workbench" as const,
  isDefault: true,
  isPrivate: true,
  canChangeVisibility: true,
  canManageGroupMembers: true,
};
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
it("opens exact account lookup from repository access without loading a global account directory", async () => {
  localStorage.setItem("gitust_token", "session-a");
  const fetcher = vi.fn();
  vi.stubGlobal("fetch", fetcher);
  const intent = {
    repoId: repo.id,
    name: repo.name,
    token: "session-a",
    epoch: 0,
    key: 1,
    expected: true,
  };
  const actions = {
    begin: () => intent,
    isCurrent: () => true,
    canManageMembers: () => true,
    invalidateReads: vi.fn(),
    confirmed: vi.fn(),
    cancel: vi.fn(),
    refresh: async () => {},
    unavailable: vi.fn(),
  };
  render(() => (
    <RepositoryVisibility repository={repo} liveMode actions={actions} />
  ));
  await fireEvent.click(
    screen.getByRole("button", { name: "Change repository access" }),
  );
  await vi.dynamicImportSettled();
  await fireEvent.click(
    screen.getByRole("button", { name: "Manage existing member" }),
  );
  await vi.dynamicImportSettled();
  expect(await screen.findByRole("textbox", { name: "Username" })).toBeTruthy();
  expect(fetcher).not.toHaveBeenCalled();
  await fireEvent.input(screen.getByRole("textbox", { name: "Username" }), {
    target: { value: "Kevin" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Find", exact: true }),
  );
  expect(screen.getByRole("alert").textContent).toContain(
    "Enter an existing username",
  );
  expect(fetcher).not.toHaveBeenCalled();
});

const wire = {
  id: repo.id,
  name: repo.name,
  branch: repo.branch,
  head_commit: repo.headCommit,
  capability: "workbench",
  is_default: true,
  is_private: true,
  can_change_visibility: true,
  can_manage_group_members: true,
};
const second = {
  ...wire,
  id: "22222222-2222-4222-8222-222222222222",
  name: "reference",
  capability: "browse",
  is_default: false,
};
const snapshot = (extra: Record<string, unknown> = {}) => ({
  repository_id: repo.id,
  owner_team_id: "33333333-3333-4333-8333-333333333333",
  user_id: "44444444-4444-4444-8444-444444444444",
  username: "Existing_User",
  role: "Developer",
  status: "active",
  is_member: false,
  membership_version: "9007199254740993",
  ...extra,
});
const reply = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
};
beforeEach(() => {
  localStorage.setItem("gitust_token", "session-a");
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
const setup = async (
  route: (url: string, init: RequestInit) => Promise<Response>,
  hint: boolean | undefined = true,
) => {
  let rows = [wire, second].map(({ can_manage_group_members, ...row }) =>
    hint === undefined ? row : { ...row, can_manage_group_members: hint },
  );
  const fetcher = vi.fn(async (url: string, init: RequestInit = {}) =>
    url === "/api/v1/repos" ? reply({ repositories: rows }) : route(url, init),
  );
  vi.stubGlobal("fetch", fetcher);
  let workspace!: ReturnType<typeof createRepositoryWorkspace>;
  const hydrate = vi.fn(),
    clearView = vi.fn();
  render(() => {
    const [live] = createSignal(true);
    workspace = createRepositoryWorkspace({
      liveMode: live,
      setActiveTab: vi.fn(),
      hydrate,
      clearView,
    });
    return (
      <Show when={workspace.selected()}>
        {(selected) => (
          <RepositoryOverview
            repository={selected()}
            liveMode={live()}
            worktrees={1}
            actions={workspace}
            onNavigate={workspace.navigate}
          />
        )}
      </Show>
    );
  });
  await workspace.refresh();
  workspace.open(workspace.state().repositories[0]!);
  return {
    workspace,
    fetcher,
    hydrate,
    clearView,
    withdraw: () => {
      rows = rows.map((row) => ({ ...row, can_manage_group_members: false }));
    },
  };
};
const open = async () => {
  const trigger = await screen.findByRole("button", {
    name: "Change repository access",
  });
  trigger.focus();
  await fireEvent.click(trigger);
  await vi.dynamicImportSettled();
  await fireEvent.click(
    screen.getByRole("button", { name: "Manage existing member" }),
  );
  await vi.dynamicImportSettled();
  return screen.findByRole("textbox", { name: "Username" });
};
const find = async (name = "Existing_User") => {
  await fireEvent.input(screen.getByRole("textbox", { name: "Username" }), {
    target: { value: name },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Find", exact: true }),
  );
};
const patches = (fetcher: ReturnType<typeof vi.fn>) =>
  fetcher.mock.calls.filter(([, init]) => init?.method === "PATCH");
it("finds one exact account, confirms large-version grant and removal, and keeps the selected viewer", async () => {
  const pending = deferred<Response>();
  let changes = 0;
  const state = await setup(async (url, init) => {
    if (init.method !== "PATCH") return reply(snapshot());
    changes++;
    if (changes === 1) return pending.promise;
    return reply(
      snapshot({ is_member: false, membership_version: "9007199254740995" }),
    );
  });
  const input = await open();
  await waitFor(() => expect(document.activeElement).toBe(input));
  expect(state.fetcher.mock.calls).toHaveLength(1);
  await find("  EXISTING_User  ");
  const add = await screen.findByRole("button", { name: "Add to group" });
  await waitFor(() => expect(document.activeElement).toBe(add));
  expect(state.fetcher.mock.calls.at(-1)![0]).toBe(
    `/api/v1/repos/${repo.id}/group-members?username=existing_user`,
  );
  expect(screen.getByText("@Existing_User")).toBeTruthy();
  await fireEvent.click(add);
  await fireEvent.click(add);
  expect(patches(state.fetcher)).toHaveLength(1);
  expect(screen.getByText("Not a member of the owning group")).toBeTruthy();
  expect(JSON.parse(patches(state.fetcher)[0]![1].body)).toEqual({
    expected_membership_version: "9007199254740993",
    is_member: true,
  });
  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Close", exact: true }),
    ),
  );
  pending.resolve(
    reply(
      snapshot({ is_member: true, membership_version: "9007199254740994" }),
    ),
  );
  const remove = await screen.findByRole("button", {
    name: "Remove from group",
  });
  await waitFor(() => expect(document.activeElement).toBe(remove));
  expect(screen.getByText("Added to the owning group.")).toBeTruthy();
  await fireEvent.click(remove);
  await screen.findByText(
    "Removed from the owning group. The account remains unchanged.",
  );
  expect(JSON.parse(patches(state.fetcher)[1]![1].body)).toEqual({
    expected_membership_version: "9007199254740994",
    is_member: false,
  });
  expect(state.hydrate).toHaveBeenCalledTimes(1);
  expect(state.clearView).toHaveBeenCalledTimes(1);
  await fireEvent.keyDown(
    screen.getByRole("button", { name: "Add to group" }),
    { key: "Escape", code: "Escape" },
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Change repository access" }),
    ),
  );
  expect(
    state.fetcher.mock.calls.some(([url]) =>
      String(url).includes("admin/users"),
    ),
  ).toBe(false);
});
it("keeps unknown accounts local to the form and allows removing a suspended SiteAdmin without claiming access loss", async () => {
  let queries = 0;
  const state = await setup(async (_url, init) => {
    if (init.method === "PATCH")
      return reply(
        snapshot({
          role: "SiteAdmin",
          status: "suspended",
          membership_version: "9007199254740994",
        }),
      );
    return ++queries === 1
      ? reply({ code: "group_member_not_found" }, 404)
      : reply(
          snapshot({ role: "SiteAdmin", status: "suspended", is_member: true }),
        );
  });
  await open();
  await find("NoSuchUser");
  await screen.findByText("Account not found. Check the exact username.");
  expect(state.workspace.selected()?.id).toBe(repo.id);
  expect(state.clearView).toHaveBeenCalledTimes(1);
  await find();
  const remove = await screen.findByRole("button", {
    name: "Remove from group",
  });
  expect((remove as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByText(/SiteAdmin access remains/)).toBeNull();
  expect(
    screen.queryByText(/removing membership does not remove that access/),
  ).toBeNull();
  await fireEvent.click(remove);
  await screen.findByText(
    "Removed from the owning group. The account remains unchanged.",
  );
  expect(
    (screen.getByRole("button", { name: "Add to group" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
});
it.each([
  [409, "membership_conflict"],
  [503, "membership_unconfirmed"],
])(
  "requires explicit relookup after %s without retrying with a new version",
  async (status, code) => {
    let queries = 0;
    const state = await setup(async (_url, init) =>
      init.method === "PATCH"
        ? reply({ code }, status)
        : reply(snapshot({ membership_version: String(++queries) })),
    );
    await open();
    await find();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add to group" }),
    );
    await screen.findByRole("alert");
    expect(screen.queryByRole("button", { name: "Add to group" })).toBeNull();
    expect(patches(state.fetcher)).toHaveLength(1);
    expect(queries).toBe(1);
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("textbox", { name: "Username" }),
      ),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Find", exact: true }),
    );
    await screen.findByRole("button", { name: "Add to group" });
    expect(patches(state.fetcher)).toHaveLength(1);
    expect(queries).toBe(2);
  },
);
it("rejects mismatched membership identity/version responses without showing a confirmed grant", async () => {
  const bad = [
    snapshot({
      user_id: "55555555-5555-4555-8555-555555555555",
      is_member: true,
      membership_version: "9007199254740994",
    }),
    snapshot({ is_member: true }),
    snapshot({ membership_version: "01" }),
  ];
  let queries = 0;
  const state = await setup(async (_url, init) =>
    init.method === "PATCH"
      ? reply(bad.shift())
      : reply(++queries === 3 ? bad.shift() : snapshot()),
  );
  await open();
  for (let i = 0; i < 2; i++) {
    await find();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add to group" }),
    );
    await screen.findByRole("alert");
    expect(screen.queryByText("Added to the owning group.")).toBeNull();
  }
  await find();
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Add to group" })).toBeNull();
  expect(patches(state.fetcher)).toHaveLength(2);
});
it.each(["username", "selection", "session"])(
  "ignores a delayed lookup body after %s changes",
  async (kind) => {
    const body = deferred<string>();
    const state = await setup(
      async () => ({ status: 200, text: () => body.promise }) as Response,
    );
    await open();
    await find();
    await waitFor(() => expect(state.fetcher.mock.calls).toHaveLength(2));
    if (kind === "username")
      await fireEvent.input(screen.getByRole("textbox", { name: "Username" }), {
        target: { value: "Other_User" },
      });
    else if (kind === "selection") {
      state.workspace.open(state.workspace.state().repositories[1]!);
      state.workspace.open(state.workspace.state().repositories[0]!);
    } else {
      localStorage.setItem("gitust_token", "session-b");
      window.dispatchEvent(
        new StorageEvent("storage", { key: "gitust_token" }),
      );
    }
    body.resolve(JSON.stringify(snapshot()));
    await vi.dynamicImportSettled();
    await waitFor(() =>
      expect(screen.queryByText("@Existing_User")).toBeNull(),
    );
    expect(patches(state.fetcher)).toHaveLength(0);
  },
);
it.each(["selection", "session"])(
  "does not publish a late mutation result after %s replacement",
  async (kind) => {
    const body = deferred<string>();
    const state = await setup(async (_url, init) =>
      init.method === "PATCH"
        ? ({ status: 200, text: () => body.promise } as Response)
        : reply(snapshot()),
    );
    await open();
    await find();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add to group" }),
    );
    await waitFor(() => expect(patches(state.fetcher)).toHaveLength(1));
    if (kind === "selection") {
      state.workspace.open(state.workspace.state().repositories[1]!);
      state.workspace.open(state.workspace.state().repositories[0]!);
    } else {
      localStorage.setItem("gitust_token", "session-b");
      window.dispatchEvent(
        new StorageEvent("storage", { key: "gitust_token" }),
      );
    }
    body.resolve(
      JSON.stringify(
        snapshot({ is_member: true, membership_version: "9007199254740994" }),
      ),
    );
    await vi.dynamicImportSettled();
    await waitFor(() =>
      expect(screen.queryByText("Added to the owning group.")).toBeNull(),
    );
    expect(
      state.fetcher.mock.calls.filter(([url]) => url === "/api/v1/repos"),
    ).toHaveLength(1);
  },
);
it("hides management for an old/negative catalog and clears an open result when the current hint is withdrawn", async () => {
  const state = await setup(async () => reply(snapshot()));
  await open();
  await find();
  await screen.findByRole("button", { name: "Add to group" });
  state.withdraw();
  await state.workspace.refresh();
  await waitFor(() =>
    expect(screen.queryByRole("textbox", { name: "Username" })).toBeNull(),
  );
  expect(
    screen.queryByRole("button", { name: "Manage existing member" }),
  ).toBeNull();
});
