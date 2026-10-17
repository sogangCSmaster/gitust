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

const InviteForm = lazy(() =>
  import("./RepositoryGroupInviteForm").then((module) => ({
    default: module.RepositoryGroupInviteForm,
  })),
);

const MemberForm = lazy(() =>
  import("./RepositoryGroupMemberForm").then((module) => ({
    default: module.RepositoryGroupMemberForm,
  })),
);

export const RepositoryVisibility: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  actions?: VisibilityActions;
}> = (props) => {
  const [intent, setIntent] = createSignal<VisibilityIntent | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  const [page, setPage] = createSignal<"access" | "invite" | "member">(
    "access",
  );
  const inviting = () => page() === "invite";
  const canManageMembers = () => {
    const own = intent();
    return !!own && props.actions?.canManageMembers?.(own) === true;
  };
  const draft = () => {
    const own = intent();
    return (
      (own && props.actions?.inviteDraft?.(own)) || {
        email: "",
        role: "Developer" as const,
      }
    );
  };
  const canInvite = () => {
    const own = intent();
    return !!own && props.actions?.canInvite?.(own) === true;
  };
  const clear = () => {
    const own = intent();
    if (own) props.actions?.cancel(own);
    setPage("access");
    setIntent(null);
    setSubmitting(false);
  };
  const close = () => {
    if (!submitting() || page() !== "access") clear();
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
    if (
      (inviting() && !canInvite()) ||
      (page() === "member" && !canManageMembers())
    ) {
      setPage("access");
      setSubmitting(false);
    }
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
                <DialogTitle>
                  {page() === "member"
                    ? "Manage existing member"
                    : inviting()
                      ? "Invite to owning group"
                      : "Repository access"}
                </DialogTitle>
                <DialogDescription>
                  {page() === "member"
                    ? "Look up an existing account to manage owning-group membership."
                    : inviting()
                      ? "Create a 72-hour, one-use account invitation for the owning group."
                      : "Choose who can access this repository. Anonymous access stays disabled."}
                </DialogDescription>
              </DialogHeader>
              <p
                class="truncate text-xs text-muted-foreground"
                title={own().name}
              >
                {own().name}
              </p>
              <DeferredView label="Repository access" onCancel={close}>
                <Show
                  when={page() === "member"}
                  fallback={
                    <Show
                      when={inviting()}
                      fallback={
                        <Form
                          intent={own()}
                          actions={props.actions!}
                          submitting={submitting()}
                          onSubmitting={setSubmitting}
                          onClose={close}
                          onManageMembers={
                            canManageMembers()
                              ? () => setPage("member")
                              : undefined
                          }
                          onInvite={
                            canInvite() ? () => setPage("invite") : undefined
                          }
                        />
                      }
                    >
                      <InviteForm
                        draft={draft()}
                        onDraft={(value) => {
                          if (current() && canInvite())
                            props.actions?.setInviteDraft?.(own(), value);
                        }}
                        intent={own()}
                        actions={props.actions!}
                        submitting={submitting()}
                        onSubmitting={setSubmitting}
                        onClose={close}
                      />
                    </Show>
                  }
                >
                  <MemberForm
                    intent={own()}
                    actions={props.actions!}
                    isPrivate={props.repository.isPrivate === true}
                    onSubmitting={setSubmitting}
                    onClose={close}
                  />
                </Show>
              </DeferredView>
            </DialogContent>
          </Dialog>
        )}
      </Show>
    </>
  );
};
