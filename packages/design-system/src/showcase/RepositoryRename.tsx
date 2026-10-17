import {
  createEffect,
  createSignal,
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
import {
  TextField,
  TextFieldInput,
  TextFieldLabel,
} from "../components/ui/text-field";
import { Icon } from "../components/workspace/icon";
import type { ConnectedRepository } from "./connected-repository-client";
import {
  validRepositoryName,
  type RepositoryRenameActions,
  type RepositoryRenameIntent,
} from "./repository-rename-types";

export const RepositoryRename: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  actions?: RepositoryRenameActions;
}> = (props) => {
  const [intent, setIntent] = createSignal<RepositoryRenameIntent | null>(null);
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  let generation = 0;
  const close = (force = false) => {
    if (submitting() && !force) return;
    const own = intent();
    if (own) props.actions?.cancel(own);
    generation++;
    setIntent(null);
    setSubmitting(false);
    setName("");
    setError(null);
  };
  const current = () => {
    const own = intent();
    return (
      own &&
      props.liveMode &&
      own.repoId === props.repository.id &&
      props.actions?.isCurrent(own)
    );
  };
  createEffect(() => {
    if (intent() && !current()) close(true);
  });
  onMount(() => {
    const changed = (event: StorageEvent) => {
      if (
        (event.key === null || event.key === "gitust_token") &&
        intent() &&
        !current()
      )
        close(true);
    };
    window.addEventListener("storage", changed);
    onCleanup(() => window.removeEventListener("storage", changed));
  });
  onCleanup(() => close(true));
  const open = () => {
    if (!props.liveMode || props.repository.canRename !== true || intent())
      return;
    const own = props.actions?.begin();
    if (!own || own.repoId !== props.repository.id) return;
    generation++;
    setName(own.name);
    setError(null);
    setIntent(own);
  };
  const submit = async (event: Event) => {
    event.preventDefault();
    const own = intent();
    const actions = props.actions;
    if (!own || !actions || !current()) {
      close(true);
      return;
    }
    if (submitting()) return;
    const value = name().trim();
    if (!validRepositoryName(value)) {
      setError(
        "Use 1–100 letters, numbers, dots, underscores or hyphens; avoid edge or repeated dots.",
      );
      return;
    }
    const ownGeneration = generation;
    setError(null);
    setSubmitting(true);
    try {
      const result = await actions.save(own, value);
      if (ownGeneration !== generation || intent() !== own) return;
      setSubmitting(false);
      if (result.kind === "error") setError(result.error);
      else close(true);
    } catch {
      if (ownGeneration === generation && intent() === own) {
        setSubmitting(false);
        setError("Could not confirm the name change. Try again.");
      }
    }
  };
  return (
    <>
      <Show
        when={
          props.liveMode && props.repository.canRename === true && props.actions
        }
      >
        <Button
          size="icon"
          variant="ghost"
          class="shrink-0"
          aria-label="Rename repository"
          onClick={open}
        >
          <Icon name="pencil" />
        </Button>
      </Show>
      <Show when={intent()}>
        <Dialog
          open
          onOpenChange={(value) => {
            if (!value) close();
          }}
        >
          <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-sm rounded-lg">
            <DialogHeader class="pr-8 text-left">
              <DialogTitle>Rename repository</DialogTitle>
              <DialogDescription>Change its display name.</DialogDescription>
            </DialogHeader>
            <form class="space-y-4" onSubmit={submit} aria-busy={submitting()}>
              <TextField
                value={name()}
                onChange={setName}
                disabled={submitting()}
              >
                <TextFieldLabel>Repository name</TextFieldLabel>
                <TextFieldInput
                  maxLength={100}
                  autocomplete="off"
                  spellcheck={false}
                  aria-describedby={
                    error() ? "repository-name-error" : undefined
                  }
                />
              </TextField>
              <Show when={error()}>
                <p
                  id="repository-name-error"
                  role="alert"
                  class="text-xs text-error-foreground"
                >
                  {error()}
                </p>
              </Show>
              <div class="flex justify-end gap-2">
                <Button
                  type="button"
                  variant="outline"
                  disabled={submitting()}
                  onClick={() => close()}
                >
                  Cancel
                </Button>
                <Button type="submit" disabled={submitting()}>
                  {submitting() ? "Saving…" : "Save name"}
                </Button>
              </div>
            </form>
          </DialogContent>
        </Dialog>
      </Show>
    </>
  );
};
