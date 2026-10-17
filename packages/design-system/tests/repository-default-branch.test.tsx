// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { createSignal, Show } from "solid-js";
import { createRepositoryWorkspace } from "../src/showcase/repository-workspace";
import { RepositoryOverview } from "../src/showcase/RepositoryOverview";

const primary = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "primary",
  branch: "main",
  head_commit: "a".repeat(40) as string | null,
  capability: "workbench",
  is_default: true,
  can_change_default_branch: true,
};
const secondary = {
  ...primary,
  id: "22222222-2222-4222-8222-222222222222",
  name: "secondary",
  capability: "browse",
  is_default: false,
  can_change_default_branch: false,
};
const writableSecondary = {
  ...secondary,
  can_change_default_branch: true,
  can_push: true,
};
const topic = {
  name: "topic",
  ref_name: "refs/heads/topic",
  commit_sha: "b".repeat(40),
};
const response = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status });
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
  head: string | null = primary.head_commit,
  repository = primary,
) => {
  let row = { ...repository, head_commit: head };
  const other = repository.id === primary.id ? secondary : primary;
  let page = { branches: [topic], next_cursor: null as string | null };
  const fetcher = vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "PATCH") return patch(init);
    if (url.includes("/branches?")) return response(page);
    return response({ repositories: [row, other] });
  });
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
        {(repository) => (
          <RepositoryOverview
            repository={repository()}
            liveMode={live()}
            actions={workspace}
            worktrees={2}
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
    setRow: (value: Partial<typeof row>) => {
      row = { ...row, ...value };
    },
    setPage: (value: typeof page) => {
      page = value;
    },
  };
};
const open = async () => {
  const trigger = screen.getByRole("button", { name: "Change default branch" });
  trigger.focus();
  await fireEvent.click(trigger);
  expect(screen.getByRole("dialog")).toBeTruthy();
  await vi.dynamicImportSettled();
  const select = await screen.findByLabelText("New default branch");
  await waitFor(() => expect(document.activeElement).toBe(select));
  await waitFor(() =>
    expect(screen.queryByText("Loading branches…")).toBeNull(),
  );
  return { trigger, select };
};
const choose = async (label = "topic · bbbbbbbb") => {
  await fireEvent.keyDown(screen.getByLabelText("New default branch"), {
    key: "ArrowDown",
  });
  await fireEvent.click(await screen.findByRole("option", { name: label }));
};
const submit = () =>
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!);

it("changes an unborn default only after an exact confirmed branch+SHA, without clearing the selected workspace", async () => {
  const pending = deferred<Response>();
  const state = await setup(() => pending.promise, null);
  expect(
    state.fetcher.mock.calls.some(([url]) => url.includes("/branches?")),
  ).toBe(false);
  const { trigger, select } = await open();
  await fireEvent.keyDown(select, { key: "Escape", code: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  await open();
  await choose();
  await submit();
  expect(
    (
      screen.getByRole("button", {
        name: "Cancel",
        exact: true,
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  await fireEvent.keyDown(screen.getByRole("dialog"), {
    key: "Escape",
    code: "Escape",
  });
  expect(screen.getByRole("dialog")).toBeTruthy();
  expect(state.workspace.selected()).toMatchObject({
    branch: "main",
    headCommit: null,
  });
  expect(state.hydrate).toHaveBeenCalledTimes(1);
  const patch = state.fetcher.mock.calls.find(
    ([, init]) => init?.method === "PATCH",
  )!;
  expect(patch[0]).toBe(`/api/v1/repos/${primary.id}/default-branch`);
  expect(patch[1]).toMatchObject({
    headers: { Authorization: "Bearer remembered-session" },
    body: JSON.stringify({
      target_ref: topic.ref_name,
      expected_current_ref: "refs/heads/main",
      expected_target_commit: topic.commit_sha,
    }),
  });
  state.setRow({ branch: "topic", head_commit: topic.commit_sha });
  pending.resolve(
    response({
      id: primary.id,
      branch: "topic",
      head_commit: topic.commit_sha,
    }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() =>
    expect(state.workspace.selected()).toMatchObject({
      branch: "topic",
      headCommit: topic.commit_sha,
    }),
  );
  expect(
    screen.getByRole("button", { name: "Change default branch" }).textContent,
  ).toContain("topic");
  expect(state.clearView).toHaveBeenCalledTimes(1);
  expect(state.hydrate).toHaveBeenCalledTimes(2);
  state.workspace.open(state.workspace.state().repositories[1]!);
  expect(
    screen.queryByRole("button", { name: "Change default branch" }),
  ).toBeNull();
  state.workspace.open(state.workspace.state().repositories[0]!);
  state.setRow({ can_change_default_branch: false });
  await state.workspace.refresh();
  expect(
    screen.queryByRole("button", { name: "Change default branch" }),
  ).toBeNull();
});

it("keeps Next available for a page with no eligible candidates and retains selection after conflict or unconfirmed response", async () => {
  let mode = 409;
  const state = await setup(async () =>
    mode === 409
      ? response("private conflict detail", 409)
      : response({
          id: primary.id,
          branch: "topic",
          head_commit: "c".repeat(40),
        }),
  );
  const agent = {
    name: "agent/private",
    ref_name: "refs/heads/agent/private",
    commit_sha: topic.commit_sha,
  };
  const cursor = "v1." + btoa(agent.ref_name).replace(/=+$/, "");
  state.setPage({
    branches: [
      {
        name: "HEAD",
        ref_name: "refs/heads/HEAD",
        commit_sha: topic.commit_sha,
      },
      agent,
    ],
    next_cursor: cursor,
  });
  await open();
  expect(screen.getByRole("status").textContent).toContain(
    "No eligible branches on this page",
  );
  state.setPage({ branches: [topic], next_cursor: null });
  await fireEvent.click(
    screen.getByRole("button", { name: "Next", exact: true }),
  );
  await screen.findByText("Choose an existing committed branch.");
  await choose();
  await submit();
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain(
    "Refresh before retrying",
  );
  expect(screen.queryByText("private conflict detail")).toBeNull();
  expect(screen.getByLabelText("New default branch").textContent).toContain(
    "topic · bbbbbbbb",
  );
  expect(state.workspace.selected()?.branch).toBe("main");
  mode = 200;
  await submit();
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain(
      "Could not confirm",
    ),
  );
  expect(state.workspace.selected()?.headCommit).toBe(primary.head_commit);
  expect(state.hydrate).toHaveBeenCalledTimes(1);
  state.setRow({ can_change_default_branch: false });
  await state.workspace.refresh();
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(
    screen.queryByRole("button", { name: "Change default branch" }),
  ).toBeNull();
  expect(
    state.fetcher.mock.calls.some(([url]) => url.includes("cursor=")),
  ).toBe(true);
});

it("changes a supported browse repository by its UUID and hides management without a positive hint", async () => {
  const pending = deferred<Response>();
  const state = await setup(
    () => pending.promise,
    secondary.head_commit,
    writableSecondary,
  );
  expect(screen.getByText("Git hosting")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Worktrees/ })).toBeNull();
  await open();
  await choose();
  await submit();
  const patch = state.fetcher.mock.calls.find(
    ([, init]) => init?.method === "PATCH",
  )!;
  expect(patch[0]).toBe(`/api/v1/repos/${secondary.id}/default-branch`);
  expect(JSON.parse(patch[1]!.body as string)).toEqual({
    target_ref: topic.ref_name,
    expected_current_ref: "refs/heads/main",
    expected_target_commit: topic.commit_sha,
  });
  expect(state.workspace.selected()?.branch).toBe("main");
  state.setRow({ branch: topic.name, head_commit: topic.commit_sha });
  pending.resolve(
    response({
      id: secondary.id,
      branch: topic.name,
      head_commit: topic.commit_sha,
    }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(
    screen.getByRole("button", { name: "Change default branch" }).textContent,
  ).toContain(topic.name);
  expect(state.clearView).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: /Files/ })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Worktrees/ })).toBeNull();
  for (const hint of [false, undefined]) {
    state.setRow({ can_change_default_branch: hint });
    await state.workspace.refresh();
    expect(
      screen.queryByRole("button", { name: "Change default branch" }),
    ).toBeNull();
    expect(screen.getByText("Git hosting")).toBeTruthy();
  }
});

it.each(["ABA", "logout", "replacement"])(
  "ignores a late successful PATCH body after %s without changing a new selection or credential",
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
      secondary.head_commit,
      writableSecondary,
    );
    await open();
    await choose();
    await submit();
    await started.promise;
    if (mode === "ABA") {
      const [a, b] = state.workspace.state().repositories;
      state.workspace.open(b!);
      state.workspace.open(a!);
    } else {
      if (mode === "logout") localStorage.removeItem("gitust_token");
      else localStorage.setItem("gitust_token", "new-session");
    }
    const count = state.fetcher.mock.calls.length;
    body.resolve(
      JSON.stringify({
        id: secondary.id,
        branch: "topic",
        head_commit: topic.commit_sha,
      }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(state.workspace.state().repositories[0]?.branch).toBe("main");
    expect(state.fetcher.mock.calls).toHaveLength(count);
    expect(
      state.fetcher.mock.calls.filter(([, init]) => init?.method === "PATCH"),
    ).toHaveLength(1);
  },
);
