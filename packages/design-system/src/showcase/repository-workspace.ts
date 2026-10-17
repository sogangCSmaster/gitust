import { createEffect, createMemo, createSignal, untrack } from "solid-js";
import type { WorkbenchTab } from "./WorkbenchApp";
import {
  createConnectedRepositoryClient,
  canCloneRepository,
  type ConnectedRepository,
  type ConnectedRepositoryState,
} from "./connected-repository-client";
import { DEMO_REPOSITORY } from "./repository-demo";
import { getLiveToken } from "./live-repo-client";
import type { RepositoryRenameActions } from "./repository-rename-types";
import type {
  VisibilityActions,
  VisibilityIntent,
} from "./repository-visibility";
import type {
  DefaultBranchActions,
  DefaultBranchIntent,
} from "./repository-default-branch";

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
  const [selectionEpoch, setSelectionEpoch] = createSignal(0);
  let renameKey = 0;
  let defaultKey = 0;
  let activeDefaultKey: number | undefined;
  let visibilityKey = 0;
  let activeVisibilityKey: number | undefined;
  const invalidateRename = () => {
    setSelectionEpoch((value) => value + 1);
    client.cancelRename();
    activeDefaultKey = undefined;
    activeVisibilityKey = undefined;
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
  const canNavigate = (tab: WorkbenchTab) =>
    !["swarm", "actions"].includes(tab) || !browseOnly();
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
  const clone = () => {
    const repository = selected();
    const epoch = selectionEpoch();
    const token = getLiveToken();
    if (
      !options.liveMode() ||
      !repository ||
      !canCloneRepository(repository) ||
      !token ||
      !client.isCurrentSession()
    )
      return null;
    const repoId = repository.id;
    return {
      repoId,
      isCurrent: () => {
        const current = selected();
        const currentEpoch = selectionEpoch();
        return (
          options.liveMode() &&
          current?.id === repoId &&
          canCloneRepository(current) &&
          epoch === currentEpoch &&
          token === getLiveToken() &&
          client.isCurrentSession()
        );
      },
    };
  };
  const rename: RepositoryRenameActions = {
    begin: () => {
      const repository = selected();
      const token = getLiveToken();
      if (
        !options.liveMode() ||
        !repository?.canRename ||
        !token ||
        !client.isCurrentSession()
      )
        return null;
      return {
        repoId: repository.id,
        name: repository.name,
        token,
        epoch: selectionEpoch(),
        key: ++renameKey,
      };
    },
    isCurrent: (intent) => {
      const repository = selected();
      const epoch = selectionEpoch();
      return (
        options.liveMode() &&
        repository?.id === intent.repoId &&
        repository.canRename === true &&
        epoch === intent.epoch &&
        intent.token === getLiveToken() &&
        client.isCurrentSession()
      );
    },
    save: async (intent, name) => {
      const result = await client.rename(intent, name, () =>
        rename.isCurrent(intent),
      );
      if (result.kind === "saved" && rename.isCurrent(intent))
        void client.refresh();
      return result;
    },
    cancel: (intent) => client.cancelRename(intent.key),
  };
  const defaultCurrent = (intent: DefaultBranchIntent, original = true) => {
    const repository = selected();
    const epoch = selectionEpoch();
    return (
      options.liveMode() &&
      repository?.id === intent.repoId &&
      repository.canChangeDefaultBranch === true &&
      repository.capability === "workbench" &&
      repository.isDefault &&
      (!original ||
        (activeDefaultKey === intent.key &&
          `refs/heads/${repository.branch}` === intent.currentRef)) &&
      epoch === intent.epoch &&
      intent.token === getLiveToken() &&
      client.isCurrentSession()
    );
  };
  const defaultBranch: DefaultBranchActions = {
    begin: () => {
      const repository = selected();
      const token = getLiveToken();
      if (
        !repository?.canChangeDefaultBranch ||
        !token ||
        !options.liveMode() ||
        !repository.isDefault ||
        repository.capability !== "workbench" ||
        !client.isCurrentSession()
      )
        return null;
      activeDefaultKey = ++defaultKey;
      return {
        repoId: repository.id,
        token,
        epoch: selectionEpoch(),
        key: activeDefaultKey,
        currentRef: `refs/heads/${repository.branch}`,
      };
    },
    isCurrent: defaultCurrent,
    invalidateReads: client.invalidate,
    confirmed: (intent, branch, head) => {
      if (!defaultCurrent(intent)) return;
      client.confirmDefaultBranch(intent.repoId, branch, head);
      void client.refreshIfCurrent(() => defaultCurrent(intent, false));
    },
    cancel: (intent) => {
      if (activeDefaultKey === intent.key) activeDefaultKey = undefined;
    },
    refresh: (intent) => client.refreshIfCurrent(() => defaultCurrent(intent)),
    unavailable: (intent) => {
      if (defaultCurrent(intent)) unavailable(intent.repoId);
    },
  };
  const visibilityCurrent = (intent: VisibilityIntent, original = true) => {
    const repository = selected();
    const epoch = selectionEpoch();
    return (
      options.liveMode() &&
      repository?.id === intent.repoId &&
      repository.canChangeVisibility === true &&
      typeof repository.isPrivate === "boolean" &&
      (!original ||
        (activeVisibilityKey === intent.key &&
          repository.isPrivate === intent.expected)) &&
      epoch === intent.epoch &&
      intent.token === getLiveToken() &&
      client.isCurrentSession()
    );
  };
  const visibility: VisibilityActions = {
    begin: () => {
      const repository = selected();
      const token = getLiveToken();
      if (
        !options.liveMode() ||
        !repository?.canChangeVisibility ||
        typeof repository.isPrivate !== "boolean" ||
        !token ||
        !client.isCurrentSession()
      )
        return null;
      activeVisibilityKey = ++visibilityKey;
      return {
        repoId: repository.id,
        name: repository.name,
        token,
        epoch: selectionEpoch(),
        key: activeVisibilityKey,
        expected: repository.isPrivate,
      };
    },
    isCurrent: visibilityCurrent,
    invalidateReads: client.invalidate,
    confirmed: (intent, value) => {
      if (!visibilityCurrent(intent)) return;
      client.confirmVisibility(intent.repoId, value);
      void client.refreshIfCurrent(() => visibilityCurrent(intent, false));
    },
    cancel: (intent) => {
      if (activeVisibilityKey === intent.key) activeVisibilityKey = undefined;
    },
    refresh: (intent) =>
      client.refreshIfCurrent(() => visibilityCurrent(intent)),
    unavailable: (intent) => {
      if (visibilityCurrent(intent)) unavailable(intent.repoId);
    },
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
    rename,
    clone,
    refresh: client.refresh,
    refreshViews,
    defaultBranch,
    visibility,
  };
};
