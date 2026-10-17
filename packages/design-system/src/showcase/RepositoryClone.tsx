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

export interface RepositoryCloneTarget {
  repoId: string;
  isCurrent: () => boolean;
}
const cloneUrl = (id: string): string | null => {
  if (
    !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(id) ||
    /^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(id)
  )
    return null;
  try {
    const origin = new URL(window.location.origin);
    if (origin.protocol !== "http:" && origin.protocol !== "https:")
      return null;
    return new URL(`/git/${encodeURIComponent(id.toLowerCase())}.git`, origin)
      .href;
  } catch {
    return null;
  }
};

export const RepositoryClone: Component<{
  repository: ConnectedRepository;
  liveMode: boolean;
  begin?: () => RepositoryCloneTarget | null;
}> = (props) => {
  const [target, setTarget] = createSignal<
    (RepositoryCloneTarget & { url: string }) | null
  >(null);
  const [copying, setCopying] = createSignal(false);
  const [notice, setNotice] = createSignal<"copied" | "manual" | null>(null);
  let input: HTMLInputElement | undefined;
  let generation = 0;
  const close = () => {
    generation++;
    setTarget(null);
    setCopying(false);
    setNotice(null);
  };
  const current = () => {
    const own = target();
    return (
      own &&
      props.liveMode &&
      props.repository.capability === "workbench" &&
      props.repository.isDefault &&
      own.repoId === props.repository.id &&
      own.isCurrent()
    );
  };
  createEffect(() => {
    if (target() && !current()) close();
  });
  onMount(() => {
    const changed = (event: StorageEvent) => {
      if (
        (event.key === null || event.key === "gitust_token") &&
        target() &&
        !current()
      )
        close();
    };
    window.addEventListener("storage", changed);
    onCleanup(() => window.removeEventListener("storage", changed));
  });
  onCleanup(close);
  const open = () => {
    if (
      target() ||
      !props.liveMode ||
      props.repository.capability !== "workbench" ||
      !props.repository.isDefault
    )
      return;
    const own = props.begin?.();
    if (!own || own.repoId !== props.repository.id || !own.isCurrent()) return;
    const url = cloneUrl(own.repoId);
    if (!url) return;
    generation++;
    setNotice(null);
    setTarget({ ...own, url });
  };
  const copy = async () => {
    const own = target();
    if (!own || !current()) {
      close();
      return;
    }
    if (copying()) return;
    const ownGeneration = generation;
    const active = () => {
      if (ownGeneration !== generation || target() !== own) return false;
      if (!current()) {
        close();
        return false;
      }
      return true;
    };
    setNotice(null);
    setCopying(true);
    try {
      const clipboard = navigator.clipboard;
      if (typeof clipboard?.writeText !== "function")
        throw new Error("Clipboard unavailable");
      await clipboard.writeText(own.url);
      if (active()) setNotice("copied");
    } catch {
      if (active()) {
        setNotice("manual");
        input?.focus();
        input?.select();
      }
    } finally {
      if (active()) setCopying(false);
    }
  };
  return (
    <>
      <Show
        when={
          props.liveMode &&
          props.repository.capability === "workbench" &&
          props.repository.isDefault &&
          props.begin &&
          cloneUrl(props.repository.id)
        }
      >
        <Button
          class="shrink-0"
          variant="outline"
          size="sm"
          aria-label="Clone repository"
          onClick={open}
        >
          Clone
        </Button>
      </Show>
      <Show when={target()}>
        {(own) => (
          <Dialog
            open
            onOpenChange={(value) => {
              if (!value) close();
            }}
          >
            <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-md rounded-lg">
              <DialogHeader class="pr-8 text-left">
                <DialogTitle>Clone repository</DialogTitle>
                <DialogDescription>
                  Use your Gitust username and a personal access token as the
                  Git password.
                </DialogDescription>
              </DialogHeader>
              <TextField value={own().url}>
                <TextFieldLabel>Clone URL</TextFieldLabel>
                <TextFieldInput
                  ref={input}
                  readOnly
                  autocomplete="off"
                  spellcheck={false}
                  class="min-w-0 font-mono text-xs"
                  onFocus={(event) => event.currentTarget.select()}
                />
              </TextField>
              <Show when={notice()}>
                {(message) => (
                  <p
                    role={message() === "copied" ? "status" : "alert"}
                    class="text-xs text-muted-foreground"
                  >
                    {message() === "copied"
                      ? "Copied clone URL."
                      : "Copy unavailable. Select the URL and copy it manually."}
                  </p>
                )}
              </Show>
              <div class="flex justify-end gap-2">
                <Button variant="outline" onClick={close}>
                  Close
                </Button>
                <Button
                  aria-label="Copy clone URL"
                  disabled={copying()}
                  onClick={() => void copy()}
                >
                  <Icon name="copy" /> {copying() ? "Copying…" : "Copy URL"}
                </Button>
              </div>
            </DialogContent>
          </Dialog>
        )}
      </Show>
    </>
  );
};
