// @vitest-environment jsdom
import { createSignal, Show } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@solidjs/testing-library";
import {
  RepositoryLanding,
  RepositoryOverview,
} from "../src/showcase/RepositoryViews";

const download = vi.hoisted(() => {
  let resolve!: () => void;
  return {
    started: vi.fn(),
    ready: new Promise<void>((done) => {
      resolve = done;
    }),
    resolve: () => resolve(),
  };
});
vi.mock("../src/showcase/RepositoryOverview", async () => {
  download.started();
  await download.ready;
  return vi.importActual("../src/showcase/RepositoryOverview");
});
afterEach(() => cleanup());

it("keeps the catalog available without downloading overview tools until a repository is opened", async () => {
  const [opened, setOpened] = createSignal(false);
  const repository = {
    id: "11111111-1111-4111-8111-111111111111",
    name: "fixture",
    branch: "main",
    headCommit: "a".repeat(40),
    capability: "workbench" as const,
    isDefault: true,
  };
  const [selected, setSelected] = createSignal(repository);
  render(() => (
    <>
      <p>Workspace navigation remains available</p>
      <button onClick={() => setOpened(false)}>Back to repositories</button>
      <button
        onClick={() =>
          setSelected({
            ...repository,
            name: "second",
            id: "22222222-2222-4222-8222-222222222222",
          })
        }
      >
        Open second repository
      </button>
      <Show when={!opened()}>
        <RepositoryLanding
          state={{ repositories: [repository], loading: false, error: null }}
          liveMode={true}
          onOpen={() => setOpened(true)}
          onRefresh={() => {}}
          onSignIn={() => {}}
        />
      </Show>
      <Show when={opened()}>
        <RepositoryOverview
          repository={selected()}
          liveMode={true}
          worktrees={0}
          onNavigate={() => {}}
        />
      </Show>
    </>
  ));
  expect(download.started).not.toHaveBeenCalled();
  screen.getByRole("button", { name: "Open repository fixture" }).click();
  expect(screen.getByRole("status").textContent).toContain(
    "Opening Repository overview",
  );
  expect(
    screen.getByText("Workspace navigation remains available"),
  ).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Clone repository" })).toBeNull();
  screen.getByRole("button", { name: "Back to repositories" }).click();
  expect(
    screen.getByRole("button", { name: "Open repository fixture" }),
  ).toBeTruthy();
  expect(screen.queryByRole("status")).toBeNull();
  screen.getByRole("button", { name: "Open repository fixture" }).click();
  screen.getByRole("button", { name: "Open second repository" }).click();
  download.resolve();
  await vi.dynamicImportSettled();
  expect(await screen.findByRole("heading", { name: "second" })).toBeTruthy();
  expect(screen.queryByRole("heading", { name: "fixture" })).toBeNull();
  expect(download.started).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: /Files/ })).toBeTruthy();
  expect(screen.getByRole("button", { name: /Actions/ })).toBeTruthy();
});
