import {
  createSignal,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import {
  TextField,
  TextFieldInput,
  TextFieldLabel,
} from "../components/ui/text-field";
import {
  createGroupMemberClient,
  normalizeMemberUsername,
  validMemberUsername,
  type GroupMember,
} from "./repository-group-member";
import type {
  VisibilityActions,
  VisibilityIntent,
} from "./repository-visibility";

export const RepositoryGroupMemberForm: Component<{
  intent: VisibilityIntent;
  actions: VisibilityActions;
  isPrivate: boolean;
  onSubmitting: (value: boolean) => void;
  onClose: () => void;
}> = (props) => {
  const [username, setUsername] = createSignal("");
  const [member, setMember] = createSignal<GroupMember | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [message, setMessage] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal<"lookup" | "save" | null>(null);
  const client = createGroupMemberClient(props.actions);
  let mounted = true,
    generation = 0;
  let input: HTMLInputElement | undefined,
    closeButton: HTMLButtonElement | undefined,
    changeButton: HTMLButtonElement | undefined;
  const current = () =>
    mounted &&
    props.actions.isCurrent(props.intent) &&
    props.actions.canManageMembers?.(props.intent) === true;
  const actionable = () => {
    const row = member();
    return row !== null && (row.isMember || row.status === "active");
  };
  const focus = (result = false) =>
    queueMicrotask(() => {
      if (current()) {
        const target = pending()
          ? closeButton
          : result && actionable() && changeButton?.isConnected
            ? changeButton
            : input;
        target?.focus();
      }
    });
  const clear = () => {
    generation++;
    client.cancel();
    setPending(null);
    setMember(null);
    setMessage(null);
    setError(null);
    props.onSubmitting(false);
  };
  onMount(() => {
    if (current()) focus();
    else props.onClose();
  });
  onCleanup(() => {
    mounted = false;
    generation++;
    client.cancel();
  });
  const lookup = async (event: Event) => {
    event.preventDefault();
    if (!current()) {
      props.onClose();
      return;
    }
    if (pending()) return;
    const normalized = normalizeMemberUsername(username());
    clear();
    if (!validMemberUsername(normalized)) {
      setError("Enter an existing username (2–39 letters, numbers, - or _).");
      focus();
      return;
    }
    const own = ++generation;
    const active = () => current() && generation === own;
    setPending("lookup");
    props.onSubmitting(true);
    focus();
    const result = await client.lookup(props.intent, normalized, active);
    if (!active()) return;
    setPending(null);
    props.onSubmitting(false);
    if (result.kind === "stale") {
      props.onClose();
      return;
    }
    if (result.kind === "member") setMember(result.member);
    else setError(result.error);
    focus(result.kind === "member");
  };
  const change = async () => {
    const target = member();
    if (!current()) {
      props.onClose();
      return;
    }
    if (pending() || !target || !actionable()) return;
    const own = ++generation;
    const active = () => current() && generation === own;
    setError(null);
    setMessage(null);
    setPending("save");
    props.onSubmitting(true);
    focus();
    const result = await client.save(
      props.intent,
      target,
      !target.isMember,
      active,
    );
    if (!active()) return;
    setPending(null);
    props.onSubmitting(false);
    if (result.kind === "stale") {
      props.onClose();
      return;
    }
    if (result.kind === "member") {
      setMember(result.member);
      setMessage(
        result.member.isMember
          ? "Added to the owning group."
          : "Removed from the owning group. The account remains unchanged.",
      );
      void props.actions.refresh(props.intent);
    } else {
      setMember(null);
      setError(result.error);
    }
    focus(result.kind === "member");
  };
  return (
    <div class="min-w-0 space-y-4" aria-busy={!!pending()}>
      <p class="text-xs text-muted-foreground">
        Membership applies to other private repositories owned by this group.
        Site-wide roles stay unchanged.
      </p>
      <form class="flex min-w-0 items-end gap-2" onSubmit={lookup}>
        <TextField
          class="min-w-0 flex-1"
          value={username()}
          disabled={pending() === "save"}
          onChange={(value) => {
            clear();
            setUsername(value);
          }}
        >
          <TextFieldLabel>Username</TextFieldLabel>
          <TextFieldInput
            ref={input}
            maxLength={39}
            autocomplete="off"
            autocapitalize="none"
            spellcheck={false}
          />
        </TextField>
        <Button type="submit" variant="outline" disabled={!!pending()}>
          {pending() === "lookup" ? "Finding…" : "Find"}
        </Button>
      </form>
      <Show when={member()}>
        {(row) => (
          <div class="min-w-0 space-y-2 rounded-md border p-3 text-xs">
            <p class="truncate font-medium" title={row().username}>
              @{row().username}
            </p>
            <p>
              {row().role} ·{" "}
              {row().status === "active" ? "Active" : "Suspended"}
            </p>
            <p>
              {row().isMember
                ? "Member of the owning group"
                : "Not a member of the owning group"}
            </p>
            <Show when={row().status === "suspended"}>
              <p class="text-muted-foreground">
                This account is inactive. Removing membership does not
                reactivate it.
              </p>
            </Show>
            <Show
              when={
                row().status === "active" &&
                (row().role === "SiteAdmin" || !props.isPrivate)
              }
            >
              <p class="text-muted-foreground">
                {row().role === "SiteAdmin"
                  ? "SiteAdmin access remains even after membership is removed."
                  : "This repository also allows signed-in users, so removing membership does not remove that access."}
              </p>
            </Show>
          </div>
        )}
      </Show>
      <Show when={message()}>
        <p role="status" class="text-xs">
          {message()}
        </p>
      </Show>
      <Show when={error()}>
        <p role="alert" class="text-xs text-error-foreground">
          {error()}
        </p>
      </Show>
      <div class="flex justify-end gap-2">
        <Button
          ref={closeButton}
          type="button"
          variant="outline"
          onClick={props.onClose}
        >
          Close
        </Button>
        <Show when={member()}>
          <Button
            ref={changeButton}
            type="button"
            disabled={!!pending() || !actionable()}
            onClick={change}
          >
            {pending() === "save"
              ? "Saving…"
              : member()?.isMember
                ? "Remove from group"
                : "Add to group"}
          </Button>
        </Show>
      </div>
    </div>
  );
};
