import type { GroupInviteDraft } from "./repository-group-invite";
import { createEffect, createSignal } from "solid-js";
import type { WorkbenchTab } from "./WorkbenchApp";
import {
  canCloneRepository,
  executeConnectedRepositoryRename,
  type ConnectedRepository,
} from "./connected-repository-client";
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
import {
  createRepositoryWorkspaceCore,
  isRepositoryTab,
  type RepositoryActionsContext,
  type RepositoryWorkspaceCore,
} from "./repository-workspace-core";

export {
  createRepositoryWorkspaceCore,
  isRepositoryTab,
  type RepositoryActionsContext,
  type RepositoryWorkspaceCore,
};

const attachedWorkspaces = new WeakMap<
  RepositoryWorkspaceCore,
  ReturnType<typeof buildRepositoryOverviewActions>
>();

const buildRepositoryOverviewActions = (core: RepositoryWorkspaceCore) => {
  const { selected, selectionEpoch, unavailable, client, liveMode } = core;
  let renameKey = 0;
  let defaultKey = 0;
  let activeDefaultKey: number | undefined;
  const [inviteDraft, setInviteDraft] = createSignal<{
    origin: VisibilityIntent;
    value: GroupInviteDraft;
  } | null>(null);
  let visibilityKey = 0;
  let activeVisibilityKey: number | undefined;

  core.onInvalidate(() => {
    activeDefaultKey = undefined;
    activeVisibilityKey = undefined;
    setInviteDraft(null);
  });

  const clone = () => {
    const repository = selected();
    const epoch = selectionEpoch();
    const token = getLiveToken();
    if (
      !liveMode() ||
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
          liveMode() &&
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
        !liveMode() ||
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
        liveMode() &&
        repository?.id === intent.repoId &&
        repository.canRename === true &&
        epoch === intent.epoch &&
        intent.token === getLiveToken() &&
        client.isCurrentSession()
      );
    },
    save: async (intent, name) => {
      const result = await executeConnectedRepositoryRename(
        client,
        intent,
        name,
        () => rename.isCurrent(intent),
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
      liveMode() &&
      repository?.id === intent.repoId &&
      repository.canChangeDefaultBranch === true &&
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
        !liveMode() ||
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
      liveMode() &&
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
  createEffect(() => {
    const saved = inviteDraft();
    const repository = selected();
    if (
      saved &&
      (!visibilityCurrent(saved.origin, false) || !repository?.canInviteToGroup)
    )
      setInviteDraft(null);
  });
  const visibility: VisibilityActions = {
    canManageMembers: (intent) => {
      const repository = selected();
      return (
        visibilityCurrent(intent) && repository?.canManageGroupMembers === true
      );
    },
    inviteDraft: (intent) => {
      const saved = inviteDraft();
      return saved &&
        saved.origin.repoId === intent.repoId &&
        saved.origin.token === intent.token &&
        saved.origin.epoch === intent.epoch &&
        visibilityCurrent(intent, false)
        ? saved.value
        : undefined;
    },
    setInviteDraft: (intent, value) => {
      if (visibilityCurrent(intent) && selected()?.canInviteToGroup)
        setInviteDraft({ origin: intent, value });
    },
    canInvite: (intent, original = true) => {
      const repository = selected();
      return (
        visibilityCurrent(intent, original) &&
        repository?.canInviteToGroup === true
      );
    },
    begin: () => {
      const repository = selected();
      const token = getLiveToken();
      if (
        !liveMode() ||
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
    ...core,
    rename,
    clone,
    defaultBranch,
    visibility,
  };
};

export const attachRepositoryOverviewActions = (
  core: RepositoryWorkspaceCore,
) => {
  const existing = attachedWorkspaces.get(core);
  if (existing) return existing;
  const attached = buildRepositoryOverviewActions(core);
  attachedWorkspaces.set(core, attached);
  return attached;
};

export const createRepositoryWorkspace = (options: {
  liveMode: () => boolean;
  setActiveTab: (tab: WorkbenchTab) => void;
  clearView: () => void;
  hydrate: (repository: ConnectedRepository) => void;
}) => attachRepositoryOverviewActions(createRepositoryWorkspaceCore(options));
