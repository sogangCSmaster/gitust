import {
  createEffect,
  createSignal,
  lazy,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "../components/ui/dialog";
import { DeferredView } from "../components/workspace/deferred-view";
import { Icon } from "../components/workspace/icon";
import type { ConnectedRepository } from "./connected-repository-client";
import type {
  DefaultBranchActions,
  DefaultBranchIntent,
} from "./repository-default-branch";

const Form = lazy(() =>
  import("./RepositoryDefaultBranchForm").then((module) => ({
    default: module.RepositoryDefaultBranchForm,
  })),
);

export const RepositoryDefaultBranch: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  actions?: DefaultBranchActions;
}> = (props) => {
  const [intent, setIntent] = createSignal<DefaultBranchIntent | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  const clear = () => {
    const own = intent();
    if (own) props.actions?.cancel(own);
    setIntent(null);
    setSubmitting(false);
  };
  const close = () => {
    if (!submitting()) clear();
  };
  const current = () => {
    const repository = props.repository;
    const own = intent();
    return (
      own &&
      props.liveMode &&
      own.repoId === repository.id &&
      props.actions?.isCurrent(own)
    );
  };
  createEffect(() => {
    if (intent() && !current()) clear();
  });
  onMount(() => {
    const changed = (event: StorageEvent) => {
      if (
        (event.key === null || event.key === "gitust_token") &&
        intent() &&
        !current()
      )
        clear();
    };
    window.addEventListener("storage", changed);
    onCleanup(() => window.removeEventListener("storage", changed));
  });
  onCleanup(clear);
  const supported = () =>
    props.repository.canChangeDefaultBranch === true &&
    props.repository.capability === "workbench" &&
    props.repository.isDefault &&
    props.liveMode &&
    props.actions;
  return (
    <>
      <Show
        when={supported()}
        fallback={
          <span class="inline-flex min-w-0 max-w-full items-center gap-1.5">
            <Icon name="git-branch" size={12} class="shrink-0" />
            <span class="truncate" title={props.repository.branch}>
              {props.repository.branch}
            </span>
          </span>
        }
      >
        <Button
          variant="ghost"
          size="sm"
          class="max-w-full min-w-0 justify-start gap-1.5 px-0"
          aria-label="Change default branch"
          title={`Default branch: ${props.repository.branch}`}
          onClick={() => {
            if (intent()) return;
            const own = props.actions?.begin();
            if (own && props.actions?.isCurrent(own)) setIntent(own);
          }}
        >
          <Icon name="git-branch" size={12} class="shrink-0" />
          <span class="shrink-0">Default</span>
          <span class="truncate">{props.repository.branch}</span>
          <Icon name="chevron-right" size={12} class="shrink-0" />
        </Button>
      </Show>
      <Show when={intent()}>
        {(own) => (
          <Dialog
            open
            onOpenChange={(value) => {
              if (!value) close();
            }}
          >
            <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-sm overflow-y-auto rounded-lg">
              <DialogHeader class="pr-8 text-left">
                <DialogTitle>Change default branch</DialogTitle>
                <DialogDescription>
                  Used for repository browsing and new clones. Existing
                  worktrees keep their target.
                </DialogDescription>
              </DialogHeader>
              <p
                class="truncate text-xs text-muted-foreground"
                title={own().currentRef}
              >
                Current: {own().currentRef.slice(11)}
              </p>
              <DeferredView label="Default branch" onCancel={close}>
                <Form
                  intent={own()}
                  actions={props.actions!}
                  onClose={close}
                  submitting={submitting()}
                  onSubmitting={setSubmitting}
                />
              </DeferredView>
            </DialogContent>
          </Dialog>
        )}
      </Show>
    </>
  );
};
