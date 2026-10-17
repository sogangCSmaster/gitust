// @vitest-environment jsdom
import { createMemo, createRoot, createSignal } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import { createRepositoryWorkspace } from "../src/showcase/repository-workspace";

const primary = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "primary",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
};
const secondary = {
  ...primary,
  id: "22222222-2222-4222-8222-222222222222",
  name: "secondary",
  capability: "browse",
  is_default: false,
};
afterEach(() => {
  localStorage.clear();
  vi.unstubAllGlobals();
});

it("updates primary capability and Activity targets after remembered-session catalog initialization", async () => {
  localStorage.setItem("gitust_token", "remembered-session");
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async () =>
        new Response(JSON.stringify({ repositories: [primary, secondary] })),
    ),
  );
  const task = { liveId: "retained-history-task" };
  let dispose!: () => void;
  const state = createRoot((stop) => {
    dispose = stop;
    const [liveMode] = createSignal(true);
    const [role, setRole] = createSignal<string | null>(null);
    const [historyTask, setHistoryTask] = createSignal<typeof task | null>(
      null,
    );
    const workspace = createRepositoryWorkspace({
      liveMode,
      setActiveTab: vi.fn(),
      clearView: vi.fn(),
      hydrate: vi.fn(),
    });
    const capability = createMemo(() => workspace.canWorkbench());
    const historyTarget = createMemo(() =>
      workspace.canWorkbench() && role() === "SiteAdmin" ? historyTask() : null,
    );
    return { workspace, capability, historyTarget, setRole, setHistoryTask };
  });
  try {
    expect(state.capability()).toBe(false);
    await state.workspace.refresh();
    state.workspace.open(state.workspace.state().repositories[0]!);
    state.setRole("SiteAdmin");
    state.setHistoryTask(task);
    expect({
      direct: state.workspace.canWorkbench(),
      memo: state.capability(),
      activity: state.historyTarget(),
    }).toEqual({ direct: true, memo: true, activity: task });
    state.workspace.open(state.workspace.state().repositories[1]!);
    expect(state.capability()).toBe(false);
    expect(state.historyTarget()).toBeNull();
    state.workspace.reset();
    expect(state.historyTarget()).toBeNull();
  } finally {
    dispose();
  }
});
it("clears a removed or denied selection and never promotes a browse-only catalog to a workbench", async () => {
  localStorage.setItem("gitust_token", "session-a");
  let rows = [primary, secondary];
  let denied = false;
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      denied
        ? new Response("Denied", { status: 403 })
        : new Response(JSON.stringify({ repositories: rows })),
    ),
  );
  let dispose!: () => void;
  let tab = "repositories";
  const clear = vi.fn();
  const hydrate = vi.fn();
  const workspace = createRoot((stop) => {
    dispose = stop;
    return createRepositoryWorkspace({
      liveMode: () => true,
      setActiveTab: (value) => {
        tab = value;
      },
      clearView: clear,
      hydrate,
    });
  });
  try {
    await workspace.refresh();
    workspace.open(workspace.state().repositories[0]!);
    expect(workspace.canWorkbench()).toBe(true);
    rows = [secondary];
    await workspace.refresh();
    expect(workspace.selected()).toBeNull();
    expect(tab).toBe("repositories");
    workspace.navigate("swarm");
    expect(tab).toBe("repositories");
    workspace.open(workspace.state().repositories[0]!);
    expect(workspace.browseOnly()).toBe(true);
    workspace.navigate("actions");
    expect(tab).toBe("overview");
    denied = true;
    await workspace.refresh();
    expect(workspace.selected()).toBeNull();
    expect(tab).toBe("repositories");
    expect(workspace.state().repositories).toEqual([]);
    expect(clear).toHaveBeenCalled();
  } finally {
    dispose();
  }
});

it("rejects old-row and eventless replacement-session intents before making repository requests", async () => {
  localStorage.setItem("gitust_token", "session-a");
  const fetcher = vi.fn(
    async () =>
      new Response(JSON.stringify({ repositories: [primary, secondary] })),
  );
  vi.stubGlobal("fetch", fetcher);
  let dispose!: () => void;
  const hydrate = vi.fn();
  const workspace = createRoot((stop) => {
    dispose = stop;
    return createRepositoryWorkspace({
      liveMode: () => true,
      setActiveTab: vi.fn(),
      clearView: vi.fn(),
      hydrate,
    });
  });
  try {
    await workspace.refresh();
    const old = workspace.state().repositories[0]!;
    workspace.open(old);
    localStorage.setItem("gitust_token", "session-b");
    expect(workspace.selected()).toBeNull();
    expect(workspace.canWorkbench()).toBe(false);
    const count = hydrate.mock.calls.length;
    workspace.open(old);
    expect(hydrate.mock.calls.length).toBe(count);
    workspace.reset();
    workspace.open(old);
    expect(hydrate.mock.calls.length).toBe(count);
  } finally {
    dispose();
  }
});

it("refreshes HEAD views across unborn transitions while preserving selection and name-only views", async () => {
  localStorage.setItem("gitust_token", "remembered-session");
  let row = {
    ...primary,
    head_commit: null as string | null,
    can_rename: true,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify({ repositories: [row] }))),
  );
  const hydrate = vi.fn();
  const clear = vi.fn();
  let dispose!: () => void;
  const workspace = createRoot((stop) => {
    dispose = stop;
    return createRepositoryWorkspace({
      liveMode: () => true,
      setActiveTab: vi.fn(),
      clearView: clear,
      hydrate,
    });
  });
  try {
    await workspace.refresh();
    workspace.open(workspace.state().repositories[0]!);
    expect(workspace.canWorkbench()).toBe(true);
    expect(hydrate).toHaveBeenCalledTimes(1);
    row = { ...row, name: "new-name" };
    await workspace.refresh();
    expect(workspace.selected()?.name).toBe("new-name");
    expect(hydrate).toHaveBeenCalledTimes(1);
    row = { ...row, head_commit: "b".repeat(40) };
    await workspace.refresh();
    expect(workspace.selected()?.headCommit).toBe(row.head_commit);
    expect(hydrate).toHaveBeenCalledTimes(2);
    row = { ...row, head_commit: "c".repeat(40) };
    await workspace.refresh();
    expect(hydrate).toHaveBeenCalledTimes(3);
    row = { ...row, head_commit: null };
    await workspace.refresh();
    expect(workspace.selected()?.headCommit).toBeNull();
    expect(hydrate).toHaveBeenCalledTimes(4);
    expect(clear).toHaveBeenCalledTimes(1);
    expect(workspace.canWorkbench()).toBe(true);
  } finally {
    dispose();
  }
});

it("does not hydrate a late explicit refresh after leaving and reopening a UUID or replacing its token", async () => {
  localStorage.setItem("gitust_token", "session-a");
  const payload = JSON.stringify({ repositories: [primary, secondary] });
  const fetcher = vi.fn(
    async (_url: string, _options?: RequestInit) => new Response(payload),
  );
  vi.stubGlobal("fetch", fetcher);
  const hydrate = vi.fn();
  let dispose!: () => void;
  const workspace = createRoot((stop) => {
    dispose = stop;
    return createRepositoryWorkspace({
      liveMode: () => true,
      setActiveTab: vi.fn(),
      clearView: vi.fn(),
      hydrate,
    });
  });
  try {
    await workspace.refresh();
    const [a, b] = workspace.state().repositories;
    workspace.open(a!);
    let finish!: (text: string) => void;
    fetcher.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: () =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    } as Response);
    const pending = workspace.refreshViews();
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
    workspace.open(b!);
    workspace.open(a!);
    const calls = hydrate.mock.calls.length;
    finish(
      JSON.stringify({
        repositories: [{ ...primary, head_commit: "c".repeat(40) }, secondary],
      }),
    );
    await pending;
    expect(hydrate).toHaveBeenCalledTimes(calls);
    expect(workspace.selected()?.headCommit).toBe(primary.head_commit);
    expect(workspace.state().loading).toBe(false);
    let finishReplaced!: (text: string) => void;
    fetcher.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: () =>
        new Promise<string>((resolve) => {
          finishReplaced = resolve;
        }),
    } as Response);
    const replaced = workspace.refreshViews();
    await vi.waitFor(() => expect(finishReplaced).toBeTypeOf("function"));
    localStorage.setItem("gitust_token", "session-b");
    finishReplaced(payload);
    await replaced;
    expect(hydrate).toHaveBeenCalledTimes(calls);
    expect(fetcher.mock.calls.at(-1)?.[1]).toMatchObject({
      headers: { Authorization: "Bearer session-a" },
    });
  } finally {
    dispose();
  }
});
