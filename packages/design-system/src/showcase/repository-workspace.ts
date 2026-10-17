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
  hydrate: () => void;
}) => {
  const [state, setState] = createSignal<ConnectedRepositoryState>({
    repository: null,
    loading: false,
    error: null,
  });
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const client = createConnectedRepositoryClient(setState);
  const repository = createMemo(() =>
    options.liveMode() ? state().repository : DEMO_REPOSITORY,
  );
  const selected = createMemo(() =>
    repository()?.id === selectedId() ? repository() : null,
  );
  const open = (repo: ConnectedRepository, tab: WorkbenchTab = "overview") => {
    if (repo.id !== repository()?.id) return;
    options.clearView();
    client.invalidate();
    setSelectedId(repo.id);
    options.setActiveTab(tab);
    if (options.liveMode()) options.hydrate();
  };
  const navigate = (tab: WorkbenchTab) => {
    if (tab === "repositories") {
      options.clearView();
      client.invalidate();
      setSelectedId(null);
      options.setActiveTab(tab);
    } else if (isRepositoryTab(tab, options.liveMode()) && !selected()) {
      const repo = repository();
      if (repo) open(repo, tab);
      else options.setActiveTab("repositories");
    } else options.setActiveTab(tab);
  };
  createEffect(() => {
    if (options.liveMode() && state().error && selectedId())
      navigate("repositories");
  });
  const reset = () => {
    client.reset();
    setSelectedId(null);
    options.setActiveTab("repositories");
  };
  return { state, selected, open, navigate, reset, refresh: client.refresh };
};
