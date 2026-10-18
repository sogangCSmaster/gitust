import { createEffect, createMemo, createSignal, untrack } from "solid-js";
import type { WorkbenchTab } from "./WorkbenchApp";
import {
  createConnectedRepositorySession,
  type ConnectedRepository,
  type ConnectedRepositoryState,
} from "./connected-repository-session";
import { DEMO_REPOSITORY } from "./repository-demo";
import { getLiveToken } from "./live-repo-context";

export interface RepositoryActionsContext {
  selectionEpoch: () => number;
  refresh: (isCurrent: () => boolean) => Promise<boolean>;
}

export const isRepositoryTab = (tab: WorkbenchTab, liveMode: boolean) =>
  ["overview", "swarm", "tree", "diff"].includes(tab) ||
  (liveMode && tab === "actions");

export const createRepositoryWorkspaceCore = (options: {
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
  const client = createConnectedRepositorySession(setState);
  const [selectionEpoch, setSelectionEpoch] = createSignal(0);
  const invalidationListeners: Array<() => void> = [];
  const invalidateRename = () => {
    setSelectionEpoch((value) => value + 1);
    client.cancelRename();
    for (const listener of invalidationListeners) listener();
  };
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
  let observedHead: { id: string; head: string | null } | undefined;
  createEffect(() => {
    const repository = selected();
    const previous = observedHead;
    observedHead = repository
      ? { id: repository.id, head: repository.headCommit }
      : undefined;
    if (
      repository &&
      previous?.id === repository.id &&
      previous.head !== repository.headCommit
    )
      untrack(() => options.hydrate(repository));
  });
  const canWorkbench = () => {
    const repository = selected();
    return repository?.capability === "workbench" && repository.isDefault;
  };
  const browseOnly = () => selected()?.capability === "browse";
  const canViewActions = () => {
    const repository = selected();
    return (
      !browseOnly() ||
      Boolean(
        repository?.canViewLocalChecksAutomatic ||
          repository?.canRunLocalChecks,
      )
    );
  };
  const canNavigate = (tab: WorkbenchTab) =>
    tab === "swarm"
      ? !browseOnly()
      : tab === "actions"
        ? canViewActions()
        : true;
  const clearSelection = () => {
    invalidateRename();
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
    invalidateRename();
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
  const refreshViews = async () => {
    const origin = selected();
    const epoch = selectionEpoch();
    const token = getLiveToken();
    if (!origin || !options.liveMode() || !token) return;
    const current = () =>
      epoch === selectionEpoch() &&
      token === getLiveToken() &&
      selected()?.id === origin.id;
    await client.refreshIfCurrent(current);
    const repository = selected();
    if (current() && repository && repository.headCommit === origin.headCommit)
      options.hydrate(repository);
  };
  const actions: RepositoryActionsContext = {
    selectionEpoch,
    refresh: async (viewCurrent) => {
      const origin = selected();
      const epoch = selectionEpoch();
      const token = getLiveToken();
      if (!origin || !token || !options.liveMode()) return false;
      const current = () => {
        const repository = selected();
        return (
          viewCurrent() &&
          epoch === selectionEpoch() &&
          token === getLiveToken() &&
          repository?.id === origin.id &&
          client.isCurrentSession()
        );
      };
      await client.refreshIfCurrent(current);
      return current() && state().error === null;
    },
  };
  return {
    state,
    selected,
    selectionEpoch,
    actions,
    open,
    navigate,
    reset,
    canWorkbench,
    canViewActions,
    browseOnly,
    unavailable,
    refresh: client.refresh,
    refreshViews,
    liveMode: options.liveMode,
    client,
    onInvalidate: (listener: () => void) => {
      invalidationListeners.push(listener);
    },
  };
};

export type RepositoryWorkspaceCore = ReturnType<
  typeof createRepositoryWorkspaceCore
>;
