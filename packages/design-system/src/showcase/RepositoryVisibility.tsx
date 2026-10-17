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
  VisibilityActions,
  VisibilityIntent,
} from "./repository-visibility";

const Form = lazy(() =>
  import("./RepositoryVisibilityForm").then((module) => ({
    default: module.RepositoryVisibilityForm,
  })),
);

export const RepositoryVisibility: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  actions?: VisibilityActions;
}> = (props) => {
  const [intent, setIntent] = createSignal<VisibilityIntent | null>(null);
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
  const label = () =>
    props.repository.isPrivate ? "Team only" : "Signed-in users";
  return (
    <>
      <Show
        when={props.liveMode && typeof props.repository.isPrivate === "boolean"}
      >
        <Show
          when={props.repository.canChangeVisibility === true && props.actions}
          fallback={
            <span class="inline-flex min-w-0 items-center gap-1.5">
              <Icon name="lock" size={12} />
              <span>{label()}</span>
            </span>
          }
        >
          <Button
            variant="ghost"
            size="sm"
            class="max-w-full min-w-0 gap-1.5 px-0"
            aria-label="Change repository access"
            onClick={() => {
              if (intent()) return;
              const own = props.actions?.begin();
              if (own && props.actions?.isCurrent(own)) setIntent(own);
            }}
          >
            <Icon name="lock" size={12} class="shrink-0" />
            <span>{label()}</span>
            <Icon name="chevron-right" size={12} class="shrink-0" />
          </Button>
        </Show>
      </Show>
      <Show when={intent()}>
        {(own) => (
          <Dialog
            open
            onOpenChange={(open) => {
              if (!open) close();
            }}
          >
            <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-sm overflow-y-auto rounded-lg">
              <DialogHeader class="pr-8 text-left">
                <DialogTitle>Repository access</DialogTitle>
                <DialogDescription>
                  Choose who can access this repository. Anonymous access stays
                  disabled.
                </DialogDescription>
              </DialogHeader>
              <p
                class="truncate text-xs text-muted-foreground"
                title={own().name}
              >
                {own().name}
              </p>
              <DeferredView label="Repository access" onCancel={close}>
                <Form
                  intent={own()}
                  actions={props.actions!}
                  submitting={submitting()}
                  onSubmitting={setSubmitting}
                  onClose={close}
                />
              </DeferredView>
            </DialogContent>
          </Dialog>
        )}
      </Show>
    </>
  );
};
