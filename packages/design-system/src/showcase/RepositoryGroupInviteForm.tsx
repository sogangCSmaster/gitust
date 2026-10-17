import {
  createSignal,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import {
  TextField,
  TextFieldInput,
  TextFieldLabel,
} from "../components/ui/text-field";
import {
  createGroupInviteMutation,
  normalizeInviteEmail,
  type GroupInviteDraft,
  type InviteRole,
} from "./repository-group-invite";
import type {
  VisibilityActions,
  VisibilityIntent,
} from "./repository-visibility";

const roles: InviteRole[] = ["Developer", "Maintainer", "SiteAdmin"];
export const RepositoryGroupInviteForm: Component<{
  intent: VisibilityIntent;
  actions: VisibilityActions;
  draft: GroupInviteDraft;
  onDraft: (draft: GroupInviteDraft) => void;
  submitting: boolean;
  onSubmitting: (value: boolean) => void;
  onClose: () => void;
}> = (props) => {
  const [link, setLink] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const mutation = createGroupInviteMutation(props.actions);
  let mounted = true;
  let email: HTMLInputElement | undefined;
  let closeButton: HTMLButtonElement | undefined;
  let linkInput: HTMLInputElement | undefined;
  const current = () =>
    mounted &&
    props.actions.isCurrent(props.intent) &&
    props.actions.canInvite?.(props.intent) === true;
  const update = (draft: GroupInviteDraft) => {
    if (current()) props.onDraft(draft);
  };
  const focusForm = () =>
    queueMicrotask(() => {
      if (current()) {
        const target =
          link() && linkInput?.isConnected
            ? linkInput
            : email?.disabled
              ? closeButton
              : email;
        target?.focus();
      }
    });
  onMount(() => {
    if (current()) focusForm();
    else props.onClose();
  });
  onCleanup(() => {
    mounted = false;
    mutation.cancel();
    setLink(null);
  });
  const submit = async (event: Event) => {
    event.preventDefault();
    if (!current()) {
      props.onClose();
      return;
    }
    if (
      props.submitting ||
      props.draft.status === "issued" ||
      props.draft.status === "conflict"
    )
      return;
    const normalized = normalizeInviteEmail(props.draft.email);
    if (
      !normalized ||
      new TextEncoder().encode(normalized).length > 254 ||
      !/^[^\s@\p{Cc}]+@[^\s@\p{Cc}]+$/u.test(normalized)
    ) {
      setError("Enter a valid email address.");
      return;
    }
    let id = props.draft.id;
    try {
      id ??= crypto.randomUUID();
    } catch {
      setError("Could not create a safe invitation ID. Try again.");
      return;
    }
    const draft: GroupInviteDraft = {
      ...props.draft,
      id,
      email: normalized,
      status: "uncertain",
    };
    update(draft);
    setLink(null);
    setError(null);
    props.onSubmitting(true);
    focusForm();
    const result = await mutation.save(props.intent, draft);
    if (!current()) return;
    props.onSubmitting(false);
    if (result.kind === "stale") {
      props.onClose();
      return;
    }
    if (result.kind === "error") {
      update({ ...draft, status: result.conflict ? "conflict" : "uncertain" });
      setError(result.error);
      focusForm();
      return;
    }
    update({ ...draft, status: "issued", expiresAt: result.expiresAt });
    if (result.path) {
      const origin = new URL(window.location.origin);
      if (origin.protocol === "http:" || origin.protocol === "https:")
        setLink(`${origin.origin}${result.path}`);
    }
    focusForm();
  };
  return (
    <form
      novalidate
      class="min-w-0 space-y-4"
      onSubmit={submit}
      aria-busy={props.submitting}
    >
      <p class="text-xs text-muted-foreground">
        Membership includes other private repositories owned by this group.
      </p>
      <TextField
        value={props.draft.email}
        onChange={(value) => update({ ...props.draft, email: value })}
        disabled={props.submitting || !!props.draft.id}
      >
        <TextFieldLabel>Email</TextFieldLabel>
        <TextFieldInput
          ref={email}
          type="email"
          maxLength={254}
          autocomplete="off"
        />
      </TextField>
      <Select<InviteRole>
        options={roles}
        value={props.draft.role}
        disabled={props.submitting || !!props.draft.id}
        onChange={(role) => {
          if (role) update({ ...props.draft, role });
        }}
        itemComponent={(item) => (
          <SelectItem item={item.item}>{item.item.rawValue}</SelectItem>
        )}
      >
        <SelectTrigger aria-label="Site-wide role" class="min-w-0">
          <SelectValue<InviteRole>>
            {(state) => state.selectedOption()}
          </SelectValue>
        </SelectTrigger>
        <SelectContent />
      </Select>
      <p class="text-xs text-muted-foreground">
        {props.draft.role === "SiteAdmin"
          ? "SiteAdmin grants administration across the entire site, not just this group."
          : "Roles apply site-wide wherever the account has access."}
      </p>
      <Show when={props.draft.status === "issued"}>
        <p role="status" class="text-xs">
          {link()
            ? "Invitation created. Share this one-use link privately; it expires in 72 hours."
            : "This invitation was already issued. Its link is no longer shown. Manage existing invitations in Access."}
        </p>
      </Show>
      <Show when={link()}>
        {(value) => (
          <TextField value={value()}>
            <TextFieldLabel>Invitation link</TextFieldLabel>
            <TextFieldInput
              ref={linkInput}
              readOnly
              onFocus={(event) => event.currentTarget.select()}
            />
          </TextField>
        )}
      </Show>
      <Show when={error()}>
        <p role="alert" class="text-xs text-error-foreground">
          {error()}
        </p>
      </Show>
      <Show when={props.draft.id}>
        <p class="break-all text-xs text-muted-foreground">
          Invitation ID: {props.draft.id}
        </p>
        <p class="text-xs text-muted-foreground">
          After changing repository or session, check existing invitations in
          Access before creating another.
        </p>
        <p class="text-xs text-muted-foreground">
          New invitation creates another request and does not revoke earlier
          links. Manage revocation in Access.
        </p>
      </Show>
      <div class="flex flex-wrap items-center justify-between gap-2">
        <Show when={props.draft.id}>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={props.submitting}
            onClick={() => {
              update({ email: props.draft.email, role: props.draft.role });
              setLink(null);
              setError(null);
              focusForm();
            }}
          >
            New invitation
          </Button>
        </Show>
        <div class="ml-auto flex justify-end gap-2">
          <Button
            ref={closeButton}
            type="button"
            variant="outline"
            onClick={props.onClose}
          >
            Close
          </Button>
          <Show
            when={
              props.draft.status !== "issued" &&
              props.draft.status !== "conflict"
            }
          >
            <Button type="submit" disabled={props.submitting}>
              {props.draft.id ? "Retry same invitation" : "Create invitation"}
            </Button>
          </Show>
        </div>
      </div>
    </form>
  );
};
