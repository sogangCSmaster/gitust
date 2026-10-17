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

const first = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "primary",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
  is_private: true,
  can_change_visibility: true,
};
const second = {
  ...first,
  id: "22222222-2222-4222-8222-222222222222",
  name: "secondary",
  capability: "browse",
  is_default: false,
};
const response = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
beforeEach(() => {
  localStorage.setItem("gitust_token", "remembered-session");
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const setup = async (
  patch: (init: RequestInit) => Promise<Response>,
  oldServer = false,
) => {
  let rows: Array<Record<string, unknown>> = [first, second].map((row) => {
    if (!oldServer) return { ...row };
    const { is_private, can_change_visibility, ...old } = row;
    return old;
  });
  const fetcher = vi.fn(async (_url: string, init?: RequestInit) =>
    init?.method === "PATCH" ? patch(init) : response({ repositories: rows }),
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
        {(repo) => (
          <RepositoryOverview
            repository={repo()}
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
    setRow: (value: Record<string, unknown>) => {
      rows[0] = { ...rows[0], ...value };
    },
  };
};
const open = async () => {
  const trigger = screen.getByRole("button", {
    name: "Change repository access",
  });
  trigger.focus();
  await fireEvent.click(trigger);
  expect(
    screen.getByRole("dialog", { name: "Repository access" }),
  ).toBeTruthy();
  await vi.dynamicImportSettled();
  const select = await screen.findByLabelText("Who can access this repository");
  await waitFor(() => expect(document.activeElement).toBe(select));
  return { trigger, select };
};
const chooseSignedIn = async () => {
  await fireEvent.keyDown(
    screen.getByLabelText("Who can access this repository"),
    { key: "ArrowDown" },
  );
  await fireEvent.click(
    await screen.findByRole("option", { name: "Signed-in users" }),
  );
};
const submit = () =>
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!);

it("confirms access only for the exact selected repository and preserves its viewer state", async () => {
  const pending = deferred<Response>();
  const state = await setup(() => pending.promise);
  const { trigger, select } = await open();
  await fireEvent.keyDown(select, { key: "Escape", code: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  await open();
  await chooseSignedIn();
  await submit();
  expect(state.workspace.selected()).toMatchObject({
    id: first.id,
    isPrivate: true,
  });
  expect(
    screen.getByText(/Existing role permissions still apply/),
  ).toBeTruthy();
  expect(
    (screen.getByRole("button", { name: "Save access" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  const call = state.fetcher.mock.calls.find(
    ([, init]) => init?.method === "PATCH",
  )!;
  expect(call[0]).toBe(`/api/v1/repos/${first.id}/visibility`);
  expect(call[1]).toMatchObject({
    headers: { Authorization: "Bearer remembered-session" },
    body: JSON.stringify({ expected_is_private: true, is_private: false }),
  });
  state.setRow({ is_private: false });
  pending.resolve(response({ id: first.id, is_private: false }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() =>
    expect(state.workspace.selected()).toMatchObject({ isPrivate: false }),
  );
  expect(
    screen.getByRole("button", { name: "Change repository access" })
      .textContent,
  ).toContain("Signed-in users");
  expect(state.clearView).toHaveBeenCalledTimes(1);
  expect(state.hydrate).toHaveBeenCalledTimes(1);
  state.workspace.open(state.workspace.state().repositories[1]!);
  expect(
    screen.getByRole("button", { name: "Change repository access" }),
  ).toBeTruthy();
});

it("retains the choice after conflicts and uncertain results without displaying server details or granting access", async () => {
  let mode = 409;
  const state = await setup(async () =>
    mode === 200
      ? response({ id: second.id, is_private: false })
      : response("private server detail", mode),
  );
  await open();
  await chooseSignedIn();
  for (mode of [409, 503, 200]) {
    await submit();
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toMatch(/Refresh/),
    );
    expect(screen.queryByText("private server detail")).toBeNull();
    expect(
      screen.getByLabelText("Who can access this repository").textContent,
    ).toContain("Signed-in users");
    expect(state.workspace.selected()).toMatchObject({ isPrivate: true });
  }
  expect(
    state.fetcher.mock.calls.filter(([, init]) => init?.method === "PATCH"),
  ).toHaveLength(3);
  state.setRow({ is_private: false });
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(
    screen.getByRole("button", { name: "Change repository access" })
      .textContent,
  ).toContain("Signed-in users");
});

it("withdraws the management control when current permission is denied", async () => {
  const state = await setup(async () => response("private denial detail", 403));
  await open();
  await chooseSignedIn();
  state.setRow({ can_change_visibility: false });
  await submit();
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(screen.queryByText("private denial detail")).toBeNull();
  await waitFor(() =>
    expect(state.workspace.state().repositories[0]).toMatchObject({
      isPrivate: true,
      canChangeVisibility: false,
    }),
  );
});

it("does not infer or offer repository visibility for older catalogs", async () => {
  const state = await setup(async () => response({}), true);
  expect(
    screen.queryByRole("button", { name: "Change repository access" }),
  ).toBeNull();
  expect(screen.queryByText("Team only")).toBeNull();
  expect(screen.queryByText("Signed-in users")).toBeNull();
  expect(
    state.fetcher.mock.calls.filter(([, init]) => init?.method === "PATCH"),
  ).toHaveLength(0);
});

it.each(["ABA", "logout", "replacement"])(
  "ignores a late access response after %s",
  async (mode) => {
    const body = deferred<string>(),
      started = deferred<void>();
    const state = await setup(
      async () =>
        ({
          status: 200,
          text: () => {
            started.resolve();
            return body.promise;
          },
        }) as Response,
    );
    await open();
    await chooseSignedIn();
    await submit();
    await started.promise;
    if (mode === "ABA") {
      state.workspace.open(state.workspace.state().repositories[1]!);
      state.workspace.open(state.workspace.state().repositories[0]!);
    } else if (mode === "logout") localStorage.removeItem("gitust_token");
    else localStorage.setItem("gitust_token", "new-session");
    const count = state.fetcher.mock.calls.length;
    body.resolve(JSON.stringify({ id: first.id, is_private: false }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(state.workspace.state().repositories[0]).toMatchObject({
      isPrivate: true,
    });
    expect(state.fetcher.mock.calls).toHaveLength(count);
    expect(
      state.fetcher.mock.calls.filter(([, init]) => init?.method === "PATCH"),
    ).toHaveLength(1);
  },
);
