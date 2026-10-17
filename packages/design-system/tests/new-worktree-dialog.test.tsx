// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { NewWorktreeDialog } from "../src/showcase/NewWorktreeDialog";
import { createNewWorktreeController } from "../src/showcase/new-worktree-controller";
const repo = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "real-repo",
  branch: "trunk",
  headCommit: "a".repeat(40),
  capability: "workbench" as const,
  isDefault: true,
};
beforeAll(async () => {
  // Prepare the real lazy module before form interactions. Pending and rejected
  // module states are covered by the dedicated deferred-feature tests.
  await import("../src/showcase/NewWorktreeForm");
});
beforeEach(() => {
  localStorage.setItem("gitust_token", "session");
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
const setup = (create: ReturnType<typeof vi.fn> = vi.fn()) => {
  const [repository, setRepository] = createSignal<typeof repo | null>(repo);
  const onCreated = vi.fn();
  let controller!: ReturnType<typeof createNewWorktreeController>;
  render(() => {
    controller = createNewWorktreeController({
      getRepository: repository,
      liveMode: () => true,
      sessionReady: () => true,
      create,
      onCreated,
    });
    return <button onClick={controller.open}>Open form</button>;
  });
  render(() => (
    <>
      {controller.target() && (
        <NewWorktreeDialog
          repository={controller.target()!}
          submitting={controller.submitting()}
          onSubmit={controller.submit}
          onClose={controller.close}
          onRefresh={vi.fn()}
        />
      )}
    </>
  ));
  return { controller, setRepository, onCreated };
};
it("opens a compact creation form without requesting creation and preserves its branch and inputs after rejection", async () => {
  const create = vi
    .fn()
    .mockResolvedValue({ kind: "error", error: "HTTP 409: branch exists" });
  const { onCreated } = setup(create);
  await fireEvent.click(screen.getByRole("button", { name: "Open form" }));
  const name = await screen.findByRole("textbox", { name: "Task name" });
  expect(create).not.toHaveBeenCalled();
  expect(
    (screen.getByRole("textbox", { name: "Base branch" }) as HTMLInputElement)
      .value,
  ).toBe("trunk");
  await fireEvent.input(name, { target: { value: "Implement review" } });
  const preview = screen.getByTestId("new-worktree-branch").textContent;
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  await screen.findByText("HTTP 409: branch exists");
  expect((name as HTMLInputElement).value).toBe("Implement review");
  expect(screen.getByTestId("new-worktree-branch").textContent).toBe(preview);
  expect(create.mock.calls[0][0]).toMatchObject({
    repoId: repo.id,
    promptSummary: "Implement review",
    baseBranch: "trunk",
    branch: preview,
  });
  expect(onCreated).not.toHaveBeenCalled();
  await fireEvent.click(
    screen.getByRole("button", { name: "Cancel", exact: true }),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
});
it("prevents duplicate submission and closes only after confirmed creation", async () => {
  let resolve!: (value: unknown) => void;
  const create = vi.fn(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { onCreated } = setup(create);
  await fireEvent.click(screen.getByRole("button", { name: "Open form" }));
  await fireEvent.input(
    await screen.findByRole("textbox", { name: "Task name" }),
    { target: { value: "New work" } },
  );
  const submit = screen.getByRole("button", {
    name: "Create worktree",
    exact: true,
  });
  await fireEvent.click(submit);
  await fireEvent.click(submit);
  expect(create).toHaveBeenCalledOnce();
  expect(screen.getByRole("dialog")).toBeTruthy();
  resolve({ kind: "created", taskId: 7, worktreeId: "created-id" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(onCreated).toHaveBeenCalledWith(7, "created-id");
});
it("closes and ignores an in-flight result when its repository is left", async () => {
  let resolve!: (value: unknown) => void;
  const create = vi.fn(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { controller, setRepository, onCreated } = setup(create);
  controller.open();
  await fireEvent.input(
    await screen.findByRole("textbox", { name: "Task name" }),
    { target: { value: "Old work" } },
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  setRepository(null);
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  resolve({ kind: "created", taskId: 7, worktreeId: "old-id" });
  await Promise.resolve();
  expect(onCreated).not.toHaveBeenCalled();
});
it("rejects an empty name without submitting and permits cancellation before a request", async () => {
  const create = vi.fn();
  setup(create);
  await fireEvent.click(screen.getByRole("button", { name: "Open form" }));
  await screen.findByRole("textbox", { name: "Task name" });
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  await screen.findByText("Enter a task name.");
  expect(create).not.toHaveBeenCalled();
  await fireEvent.click(
    screen.getByRole("button", { name: "Cancel", exact: true }),
  );
  expect(create).not.toHaveBeenCalled();
});

it("keeps advanced branch validation and agent selection connected to the submitted request", async () => {
  const create = vi
    .fn()
    .mockResolvedValue({ kind: "error", error: "Keep this form open" });
  setup(create);
  await fireEvent.click(screen.getByRole("button", { name: "Open form" }));
  await fireEvent.input(
    await screen.findByRole("textbox", { name: "Task name" }),
    { target: { value: "Advanced task" } },
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Advanced", exact: true }),
  );
  const branch = screen.getByRole("textbox", { name: "Branch", exact: true });
  await fireEvent.input(branch, { target: { value: "main" } });
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  await screen.findByText(/Use a valid branch inside agent/);
  expect(create).not.toHaveBeenCalled();
  await fireEvent.input(branch, { target: { value: "agent/custom-review" } });
  await fireEvent.keyDown(
    screen.getByRole("button", { name: "Agent Codex Swarm" }),
    { key: "ArrowDown" },
  );
  await fireEvent.click(
    await screen.findByRole("option", { name: "Cursor Agent" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  await screen.findByText("Keep this form open");
  expect(create.mock.calls[0][0]).toMatchObject({
    branch: "agent/custom-review",
    agent: "CursorAgent",
  });
});
it("dismisses a pending form after its token changes even before a storage event arrives", async () => {
  let resolve!: (value: unknown) => void;
  const create = vi.fn(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { onCreated } = setup(create);
  await fireEvent.click(screen.getByRole("button", { name: "Open form" }));
  await fireEvent.input(
    await screen.findByRole("textbox", { name: "Task name" }),
    { target: { value: "Previous session work" } },
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  localStorage.setItem("gitust_token", "replacement-session");
  resolve({ kind: "created", taskId: 7, worktreeId: "old-id" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(onCreated).not.toHaveBeenCalled();
});

it("opens on HTTP browsers that do not expose crypto.randomUUID", async () => {
  const source = globalThis.crypto;
  vi.stubGlobal("crypto", {
    getRandomValues: source.getRandomValues.bind(source),
  });
  setup();
  await fireEvent.click(screen.getByRole("button", { name: "Open form" }));
  await screen.findByRole("textbox", { name: "Task name" });
  expect(screen.getByTestId("new-worktree-branch").textContent).toMatch(
    /^agent\/task-[0-9a-f]+$/,
  );
});
