// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@solidjs/testing-library";
import { RepositoryOverview } from "../src/showcase/RepositoryViews";

const download = vi.hoisted(() => {
  let reject!: (error: Error) => void;
  return {
    ready: new Promise<never>((_, fail) => {
      reject = fail;
    }),
    reject: () => reject(new Error("chunk download failed")),
  };
});
vi.mock("../src/showcase/RepositoryOverview", () => download.ready);
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("shows a recoverable error when overview tools cannot download and leaves navigation available", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  render(() => (
    <>
      <button>Repositories navigation</button>
      <RepositoryOverview
        repository={{
          id: "11111111-1111-4111-8111-111111111111",
          name: "fixture",
          branch: "main",
          headCommit: "a".repeat(40),
          capability: "workbench",
          isDefault: true,
        }}
        liveMode={true}
        worktrees={0}
        onNavigate={() => {}}
      />
    </>
  ));
  expect(screen.getByRole("status").textContent).toContain(
    "Opening Repository overview",
  );
  download.reject();
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Could not load Repository overview",
  );
  expect(
    screen.getByRole("button", { name: "Reload application" }),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "Repositories navigation" }),
  ).toBeTruthy();
  expect(screen.queryByRole("heading", { name: "fixture" })).toBeNull();
});
