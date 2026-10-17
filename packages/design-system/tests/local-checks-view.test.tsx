// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { LiveRepositoryActionsView } from "../src/showcase/LiveRepositoryActionsView";
import { localPage, localRepoId } from "./local-checks-fixture";
import { actionsPage } from "./actions-page-fixture";

beforeEach(() => localStorage.setItem("gitust_token", "local-session"));
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.unstubAllGlobals();
});

it("exposes Local Checks only for the primary SiteAdmin and clears it when that context ends", async () => {
  const [role, setRole] = createSignal("Developer");
  const [primary, setPrimary] = createSignal(true);
  const [epoch, setEpoch] = createSignal(1);
  const fetcher = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.endsWith("/local-check-runs")
            ? localPage()
            : { ...actionsPage(), repo_id: localRepoId },
        ),
      ),
  );
  vi.stubGlobal("fetch", fetcher);
  render(() => (
    <LiveRepositoryActionsView
      repositoryId={localRepoId}
      role={role()}
      isPrimary={primary()}
      selectionEpoch={epoch()}
    />
  ));
  expect(screen.queryByRole("tab", { name: "Local Checks" })).toBeNull();
  expect(fetcher).not.toHaveBeenCalled();
  setRole("SiteAdmin");
  await fireEvent.click(
    await screen.findByRole("tab", { name: "Local Checks" }),
  );
  await screen.findByRole("list", { name: "Local Checks commands" });
  const requests = fetcher.mock.calls.length;
  localStorage.setItem("gitust_token", "replacement-session");
  window.dispatchEvent(new StorageEvent("storage", { key: "gitust_token" }));
  expect(
    screen.queryByRole("list", { name: "Local Checks commands" }),
  ).toBeNull();
  expect(screen.getByRole("status").textContent).toBe(
    "Refresh to load Local Checks.",
  );
  expect(fetcher).toHaveBeenCalledTimes(requests);
  setEpoch(3);
  expect(
    screen.queryByRole("list", { name: "Local Checks commands" }),
  ).toBeNull();
  expect(
    screen
      .getByRole("tab", { name: "Automatic", exact: true })
      .getAttribute("aria-selected"),
  ).toBe("true");
  setPrimary(false);
  expect(screen.queryByRole("tab", { name: "Local Checks" })).toBeNull();
});
