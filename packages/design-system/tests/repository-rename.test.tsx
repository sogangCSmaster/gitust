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
import { createSignal, Show } from "solid-js";
import { createRepositoryWorkspace } from "../src/showcase/repository-workspace";
import { RepositoryLanding } from "../src/showcase/RepositoryViews";
import { RepositoryOverview } from "../src/showcase/RepositoryOverview";

const primary = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "primary",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
  can_rename: true,
};
const secondary = {
  ...primary,
  id: "22222222-2222-4222-8222-222222222222",
  name: "secondary",
  capability: "browse",
  is_default: false,
};
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
  Reflect.deleteProperty(navigator, "clipboard");
});
const setup = async (
  patch: (url: string, init: RequestInit) => Promise<Response>,
  initialHead: string | null = primary.head_commit,
  canRename = true,
) => {
  let serverName = primary.name;
  let serverHead = initialHead;
  const fetcher = vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "PATCH") return patch(url, init);
    return new Response(
      JSON.stringify({
        repositories: [
          {
            ...primary,
            name: serverName,
            head_commit: serverHead,
            can_rename: canRename,
          },
          secondary,
        ],
      }),
    );
  });
  vi.stubGlobal("fetch", fetcher);
  const hydrate = vi.fn(),
    clearView = vi.fn();
  let workspace!: ReturnType<typeof createRepositoryWorkspace>;
  render(() => {
    const [tab, setTab] = createSignal("repositories");
    workspace = createRepositoryWorkspace({
      liveMode: () => true,
      setActiveTab: setTab,
      hydrate,
      clearView,
    });
    return (
      <>
        <Show when={tab() === "repositories"}>
          <RepositoryLanding
            state={workspace.state()}
            liveMode
            onOpen={workspace.open}
            onRefresh={() => void workspace.refresh()}
            onSignIn={vi.fn()}
          />
        </Show>
        <Show when={tab() === "overview" && workspace.selected()}>
          {(repository) => (
            <RepositoryOverview
              repository={repository()}
              liveMode
              worktrees={0}
              onNavigate={workspace.navigate}
              actions={workspace}
            />
          )}
        </Show>
      </>
    );
  });
  await workspace.refresh();
  await fireEvent.click(
    screen.getByRole("button", { name: "Open repository primary" }),
  );
  return {
    workspace,
    fetcher,
    hydrate,
    clearView,
    setServerHead: (value: string | null) => {
      serverHead = value;
    },
    setServerName: (value: string) => {
      serverName = value;
    },
  };
};
const openEditor = async (value: string) => {
  const trigger = screen.getByRole("button", { name: "Rename repository" });
  trigger.focus();
  await fireEvent.click(trigger);
  const input = await screen.findByRole("textbox", { name: "Repository name" });
  await waitFor(() => expect(document.activeElement).toBe(input));
  await fireEvent.input(input, { target: { value } });
  return { input: input as HTMLInputElement, trigger };
};
const save = () =>
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!);

it("renames a remembered-session repository optimistically, survives background refresh and updates list/title without clearing the viewer", async () => {
  const pending = deferred<Response>();
  const { workspace, hydrate, clearView, fetcher, setServerName } = await setup(
    () => pending.promise,
  );
  const initialHydration = hydrate.mock.calls.length,
    initialClear = clearView.mock.calls.length;
  await openEditor(" primary-renamed ");
  await save();
  expect(
    screen.getByRole("heading", {
      name: "primary-renamed",
      exact: true,
      hidden: true,
    }),
  ).toBeTruthy();
  await workspace.refresh();
  expect(
    screen.getByRole("heading", {
      name: "primary-renamed",
      exact: true,
      hidden: true,
    }),
  ).toBeTruthy();
  expect(hydrate.mock.calls.length).toBe(initialHydration);
  expect(clearView.mock.calls.length).toBe(initialClear);
  setServerName("primary-renamed");
  pending.resolve(
    new Response(JSON.stringify({ id: primary.id, name: "primary-renamed" })),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await screen.findByRole("heading", { name: "primary-renamed", exact: true });
  expect(hydrate.mock.calls.length).toBe(initialHydration);
  expect(clearView.mock.calls.length).toBe(initialClear);
  const patches = fetcher.mock.calls.filter(
    ([, init]) => init?.method === "PATCH",
  );
  expect(patches).toHaveLength(1);
  expect(patches[0]).toMatchObject([
    `/api/v1/repos/${primary.id}`,
    {
      headers: { Authorization: "Bearer remembered-session" },
      body: JSON.stringify({ name: "primary-renamed" }),
    },
  ]);
  workspace.navigate("repositories");
  await screen.findByRole("button", {
    name: "Open repository primary-renamed",
  });
  expect(
    fetcher.mock.calls.every(
      ([url]) =>
        url === "/api/v1/repos" || url === `/api/v1/repos/${primary.id}`,
    ),
  ).toBe(true);
});

it("restores the latest confirmed name after failure, retains the input and closes with Escape back to its trigger", async () => {
  const pending = deferred<Response>();
  const { workspace, setServerName } = await setup(async () => pending.promise);
  const { input, trigger } = await openEditor("wanted-name");
  await save();
  setServerName("other-admin-name");
  await workspace.refresh();
  pending.resolve(
    new Response("private error is not displayed", { status: 503 }),
  );
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain(
    "Could not confirm the name change",
  );
  expect(screen.queryByText("private error is not displayed")).toBeNull();
  expect(input.value).toBe("wanted-name");
  expect(workspace.selected()?.name).toBe("other-admin-name");
  expect(
    screen.getByRole("heading", {
      name: "other-admin-name",
      exact: true,
      hidden: true,
    }),
  ).toBeTruthy();
  input.focus();
  await fireEvent.keyDown(input, { key: "Escape", code: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(trigger));
});

it("invalidates an old PATCH body across A to B to A even when UUID and token match again", async () => {
  const body = deferred<string>(),
    started = deferred<void>();
  const { workspace, fetcher } = await setup(
    async () =>
      ({
        status: 200,
        text: () => {
          started.resolve();
          return body.promise;
        },
      }) as Response,
  );
  await openEditor("late-primary-name");
  await save();
  await started.promise;
  const rows = workspace.state().repositories;
  workspace.open(rows[1]!);
  workspace.open(rows[0]!);
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(workspace.selected()?.name).toBe("primary");
  body.resolve(JSON.stringify({ id: primary.id, name: "late-primary-name" }));
  await Promise.resolve();
  await Promise.resolve();
  expect(workspace.selected()?.name).toBe("primary");
  await screen.findByRole("heading", { name: "primary", exact: true });
  expect(
    fetcher.mock.calls.filter(([, init]) => init?.method === "PATCH"),
  ).toHaveLength(1);
});

it.each(["logout", "replacement"])(
  "drops a late body after eventless %s without issuing a request under the new credential",
  async (mode) => {
    const body = deferred<string>(),
      started = deferred<void>();
    const { workspace, fetcher } = await setup(
      async () =>
        ({
          status: 200,
          text: () => {
            started.resolve();
            return body.promise;
          },
        }) as Response,
    );
    await openEditor("old-session-name");
    await save();
    await started.promise;
    const count = fetcher.mock.calls.length;
    if (mode === "logout") localStorage.removeItem("gitust_token");
    else localStorage.setItem("gitust_token", "replacement-session");
    body.resolve(JSON.stringify({ id: primary.id, name: "old-session-name" }));
    await waitFor(() => expect(workspace.selected()).toBeNull());
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(workspace.state().repositories).toEqual([]);
    expect(fetcher.mock.calls.length).toBe(count);
    expect(
      screen.queryByRole("heading", { name: "old-session-name", exact: true }),
    ).toBeNull();
  },
);

it("shows a credential-free primary Clone dialog with honest copy feedback and keyboard focus, including an unborn default", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  const { workspace, fetcher, setServerHead } = await setup(
    vi.fn(),
    primary.head_commit,
    false,
  );
  const trigger = screen.getByRole("button", { name: "Clone repository" });
  trigger.focus();
  const count = fetcher.mock.calls.length;
  await fireEvent.click(trigger);
  const input = (await screen.findByRole("textbox", {
    name: "Clone URL",
  })) as HTMLInputElement;
  await waitFor(() => expect(document.activeElement).toBe(input));
  expect(input.readOnly).toBe(true);
  expect(input.value).toBe(`${window.location.origin}/git/${primary.id}.git`);
  const url = new URL(input.value);
  expect([url.username, url.password, url.search, url.hash]).toEqual([
    "",
    "",
    "",
    "",
  ]);
  expect(input.value).not.toContain("remembered-session");
  expect(screen.getByText(/personal access token.*Git password/i)).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Copy clone URL" }));
  await screen.findByRole("status");
  expect(screen.getByRole("status").textContent).toBe("Copied clone URL.");
  expect(writeText).toHaveBeenCalledExactlyOnceWith(input.value);
  writeText.mockRejectedValueOnce(new Error("private clipboard diagnostic"));
  await fireEvent.click(screen.getByRole("button", { name: "Copy clone URL" }));
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain("Select the URL");
  expect(screen.queryByText("private clipboard diagnostic")).toBeNull();
  expect(input.selectionStart).toBe(0);
  expect(input.selectionEnd).toBe(input.value.length);
  Reflect.deleteProperty(navigator, "clipboard");
  await fireEvent.click(screen.getByRole("button", { name: "Copy clone URL" }));
  expect(writeText).toHaveBeenCalledTimes(2);
  expect(fetcher.mock.calls.length).toBe(count);
  await fireEvent.keyDown(input, { key: "Escape", code: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  setServerHead(null);
  await workspace.refresh();
  expect(screen.getByRole("button", { name: "Clone repository" })).toBeTruthy();
  workspace.open(workspace.state().repositories[1]!);
  expect(screen.queryByRole("button", { name: "Clone repository" })).toBeNull();
});

it("expires Clone intents across selection ABA, eventless token replacement and logout without stale clipboard feedback", async () => {
  const first = deferred<void>(),
    second = deferred<void>();
  const writeText = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  const { workspace, fetcher } = await setup(vi.fn());
  const openClone = async () => {
    await fireEvent.click(
      screen.getByRole("button", { name: "Clone repository" }),
    );
    return await screen.findByRole("button", { name: "Copy clone URL" });
  };
  await fireEvent.click(await openClone());
  expect(writeText).toHaveBeenCalledTimes(1);
  const rows = workspace.state().repositories;
  workspace.open(rows[1]!);
  workspace.open(rows[0]!);
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  first.resolve();
  await Promise.resolve();
  expect(screen.queryByText("Copied clone URL.")).toBeNull();
  const oldCopy = await openClone();
  localStorage.setItem("gitust_token", "replacement-session");
  await fireEvent.click(oldCopy);
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(writeText).toHaveBeenCalledTimes(1);
  localStorage.setItem("gitust_token", "remembered-session");
  await workspace.refresh();
  workspace.open(workspace.state().repositories[0]!);
  await fireEvent.click(await openClone());
  expect(writeText).toHaveBeenCalledTimes(2);
  const requests = fetcher.mock.calls.length;
  localStorage.removeItem("gitust_token");
  window.dispatchEvent(new StorageEvent("storage", { key: "gitust_token" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  second.resolve();
  await Promise.resolve();
  expect(screen.queryByText("Copied clone URL.")).toBeNull();
  expect(fetcher.mock.calls.length).toBe(requests);
});
