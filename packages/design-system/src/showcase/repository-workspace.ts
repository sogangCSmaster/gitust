import { createEffect, createMemo, createSignal } from "solid-js";
import type { WorkbenchTab } from "./WorkbenchApp";
import {
  createConnectedRepositoryClient,
  type ConnectedRepository,
  type ConnectedRepositoryState,
} from "./connected-repository-client";
import { DEMO_REPOSITORY } from "./RepositoryViews";

export const isRepositoryTab = (tab: WorkbenchTab, liveMode: boolean) =>
  ["overview", "swarm", "tree", "diff"].includes(tab) ||
  (liveMode && tab === "actions");

export const createRepositoryWorkspace = (options: {
  liveMode: () => boolean;
  setActiveTab: (tab: WorkbenchTab) => void;
  clearView: () => void;
  hydrate: (repository: ConnectedRepository) => void;
}) => {
  const [state, setState] = createSignal<ConnectedRepositoryState>({
    repositories: [],
    loading: false,
    error: null,
  });
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const client = createConnectedRepositoryClient(setState);
  const repositories = createMemo(() =>
    options.liveMode() ? state().repositories : [DEMO_REPOSITORY],
  );
  const selected = () => {
    // Read reactive inputs before the session guard; browser storage itself is not reactive.
    const rows = repositories();
    const id = selectedId();
    const error = state().error;
    if (options.liveMode() && (!client.isCurrentSession() || error))
      return null;
    return rows.find((repo) => repo.id === id) ?? null;
  };
  const canWorkbench = () => {
    const repository = selected();
    return repository?.capability === "workbench" && repository.isDefault;
  };
  const browseOnly = () => selected()?.capability === "browse";
  const canNavigate = (tab: WorkbenchTab) =>
    !["swarm", "actions"].includes(tab) || !browseOnly();
  const clearSelection = () => {
    options.clearView();
    setSelectedId(null);
    options.setActiveTab("repositories");
  };
  const open = (repo: ConnectedRepository, tab: WorkbenchTab = "overview") => {
    if (options.liveMode() && !client.isCurrentSession()) {
      clearSelection();
      void client.refresh();
      return;
    }
    const target = repositories().find((entry) => entry.id === repo.id);
    if (!target) return;
    options.clearView();
    setSelectedId(target.id);
    options.setActiveTab(canNavigate(tab) ? tab : "overview");
    if (options.liveMode()) options.hydrate(target);
  };
  const navigate = (tab: WorkbenchTab) => {
    if (tab === "repositories") clearSelection();
    else if (!canNavigate(tab)) return;
    else if (isRepositoryTab(tab, options.liveMode()) && !selected()) {
      const repo = repositories().find((entry) => entry.isDefault);
      if (repo) open(repo, tab);
      else options.setActiveTab("repositories");
    } else options.setActiveTab(tab);
  };
  createEffect(() => {
    if (selectedId() && !state().loading && !selected()) clearSelection();
  });
  const reset = () => {
    client.reset();
    clearSelection();
  };
  const unavailable = (id: string) => {
    if (selectedId() !== id) return;
    clearSelection();
    client.reset();
    void client.refresh();
  };
  return {
    state,
    selected,
    open,
    navigate,
    reset,
    canWorkbench,
    browseOnly,
    unavailable,
    refresh: client.refresh,
  };
};
