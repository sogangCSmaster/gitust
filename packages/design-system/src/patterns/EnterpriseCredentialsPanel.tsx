import { createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription } from "../components/ui/alert";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../components/ui/tabs";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import {
  Field,
  Section,
  StatusBadge,
} from "../components/workspace/presentation";

export interface LiveInviteItem {
  id: string;
  purpose: "account_invite" | "password_reset";
  target_email: string;
  assigned_role: "SiteAdmin" | "Maintainer" | "Developer";
  expires_at: string;
  used_at: string | null;
  revoked_at: string | null;
  team_ids?: string[];
  repository_id?: string;
}

export interface LivePatItem {
  id: string;
  name: string;
  token_prefix: string;
  expires_at: string | null;
  revoked: boolean;
}

export interface LiveSshKeyItem {
  id: string;
  title: string;
  fingerprint_sha256: string;
  revoked: boolean;
}

export interface EnterpriseCredentialsPanelProps {
  allowedDomain: string;
  view?: "redemption" | "invitations" | "git";
  redeemMode: "invite" | "reset";
  onChangeRedeemMode: (mode: "invite" | "reset") => void;
  redeemToken: string;
  onRedeemTokenInput: (val: string) => void;
  onRedeemInvite?: (token: string, username: string, password: string) => void;
  onRedeemPasswordReset?: (token: string, newPassword: string) => void;
  liveInvites?: LiveInviteItem[];
  lastIssuedInviteUrl?: string | null;
  onIssueInvite?: (
    targetEmail: string,
    role: "SiteAdmin" | "Maintainer" | "Developer",
  ) => void;
  onRevokeInvite?: (inviteId: string) => void;
  livePats?: LivePatItem[];
  lastIssuedPatToken?: string | null;
  onCreatePat?: (name: string) => void;
  onRevokePat?: (patId: string) => void;
  liveSshKeys?: LiveSshKeyItem[];
  onRegisterSshKey?: (title: string, publicKeyOpenssh: string) => void;
  onRevokeSshKey?: (keyId: string) => void;
  onPolicyFeedback: (msg: string) => void;
}

export const EnterpriseCredentialsPanel: Component<
  EnterpriseCredentialsPanelProps
> = (props) => {
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [email, setEmail] = createSignal("");
  const [role, setRole] = createSignal<
    "SiteAdmin" | "Maintainer" | "Developer"
  >("Developer");
  const [patName, setPatName] = createSignal("");
  const [sshTitle, setSshTitle] = createSignal("");
  const [sshKey, setSshKey] = createSignal("");
  const expired = (date: string | null | undefined) =>
    Boolean(date && Date.parse(date) <= Date.now());
  const unavailable = () =>
    props.onPolicyFeedback(
      "This action needs a connected Gitust server and an authorized account.",
    );
  const redeem = () => {
    const token = props.redeemToken.trim();
    if (props.redeemMode === "invite") {
      props.onRedeemInvite
        ? props.onRedeemInvite(token, username().trim(), password())
        : unavailable();
    } else {
      props.onRedeemPasswordReset
        ? props.onRedeemPasswordReset(token, password())
        : unavailable();
    }
    setPassword("");
  };
  return (
    <div
      class={
        props.view === "git"
          ? "grid min-w-0 gap-3 lg:grid-cols-2"
          : "grid min-w-0 gap-3"
      }
    >
      <Show when={!props.view || props.view === "redemption"}>
        <Section
          title="Accept an invitation"
          description="Use the single-use link shared by your administrator."
        >
          <Tabs
            value={props.redeemMode}
            onChange={(value) =>
              props.onChangeRedeemMode(value as "invite" | "reset")
            }
          >
            <TabsList class="w-full">
              <TabsTrigger value="invite" class="flex-1">
                Accept invite
              </TabsTrigger>
              <TabsTrigger value="reset" class="flex-1">
                Reset password
              </TabsTrigger>
            </TabsList>
            <TabsContent value={props.redeemMode}>
              <form
                class="space-y-3"
                onSubmit={(event) => {
                  event.preventDefault();
                  redeem();
                }}
              >
                <Field
                  label="Single-use token"
                  value={props.redeemToken}
                  onValueChange={props.onRedeemTokenInput}
                  placeholder="Paste your invite or reset token"
                />
                <Show when={props.redeemMode === "invite"}>
                  <Field
                    label="Username"
                    value={username()}
                    onValueChange={setUsername}
                    placeholder="Choose a username"
                    autocomplete="username"
                  />
                </Show>
                <Field
                  label="New password"
                  type="password"
                  value={password()}
                  onValueChange={setPassword}
                  autocomplete="new-password"
                />
                <Button type="submit">
                  {props.redeemMode === "invite"
                    ? "Activate account"
                    : "Reset password"}
                </Button>
              </form>
            </TabsContent>
          </Tabs>
        </Section>
      </Show>
      <Show when={!props.view || props.view === "invitations"}>
        <Section
          title="Invite a teammate"
          description="Invites expire after 72 hours and can only be used once."
        >
          <Field
            label="Email address"
            type="email"
            value={email()}
            onValueChange={setEmail}
            placeholder={`name@${props.allowedDomain}`}
          />
          <Select
            options={["Developer", "Maintainer", "SiteAdmin"]}
            value={role()}
            onChange={(value) => {
              if (value)
                setRole(value as "SiteAdmin" | "Maintainer" | "Developer");
            }}
            itemComponent={(item) => (
              <SelectItem item={item.item}>{item.item.rawValue}</SelectItem>
            )}
          >
            <SelectLabel class="mb-2 block text-xs">Role</SelectLabel>
            <SelectTrigger aria-label="Invite role">
              <SelectValue<string>>
                {(state) => state.selectedOption()}
              </SelectValue>
            </SelectTrigger>
            <SelectContent />
          </Select>
          <Button
            variant="outline"
            onClick={() =>
              props.onIssueInvite
                ? props.onIssueInvite(email().trim(), role())
                : unavailable()
            }
          >
            Issue invite link
          </Button>
          <Show when={props.lastIssuedInviteUrl}>
            {(url) => (
              <Alert>
                <AlertDescription>
                  <p class="mb-2 font-medium">Copy this single-use link.</p>
                  <code class="break-all text-xs">{url()}</code>
                </AlertDescription>
              </Alert>
            )}
          </Show>
          <ul class="divide-y">
            <For each={props.liveInvites}>
              {(invite) => (
                <li class="flex flex-wrap items-center justify-between gap-3 py-2.5 text-sm">
                  <div class="min-w-0">
                    <p class="break-all">{invite.target_email}</p>
                    <p class="mt-1 text-xs text-muted-foreground">
                      {invite.assigned_role} · expires{" "}
                      {new Date(invite.expires_at).toLocaleString()}
                    </p>
                    <details class="mt-1 text-xs text-muted-foreground">
                      <summary class="cursor-pointer">
                        {invite.purpose === "password_reset"
                          ? "Password reset"
                          : invite.repository_id
                            ? "Owning group"
                            : Array.isArray(invite.team_ids)
                              ? invite.team_ids.length
                                ? "Group membership"
                                : "Account only"
                              : "Scope unavailable"}
                      </summary>
                      <p class="break-all">Invitation ID: {invite.id}</p>
                      <Show when={invite.purpose === "password_reset"}>
                        <p>Existing account</p>
                      </Show>
                      <Show when={invite.purpose !== "password_reset"}>
                        <Show when={invite.repository_id}>
                          <p class="break-all">
                            Repository: {invite.repository_id}
                          </p>
                        </Show>
                        <For
                          each={
                            Array.isArray(invite.team_ids)
                              ? invite.team_ids
                              : []
                          }
                        >
                          {(id) => <p class="break-all">Group: {id}</p>}
                        </For>
                      </Show>
                    </details>
                  </div>
                  <Show
                    when={
                      !invite.used_at &&
                      !invite.revoked_at &&
                      !expired(invite.expires_at)
                    }
                    fallback={
                      <StatusBadge>
                        {invite.used_at
                          ? "Used"
                          : invite.revoked_at
                            ? "Revoked"
                            : "Expired"}
                      </StatusBadge>
                    }
                  >
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => props.onRevokeInvite?.(invite.id)}
                    >
                      Revoke invite
                    </Button>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Section>
      </Show>
      <Show when={!props.view || props.view === "git"}>
        <Section
          title="Personal access tokens"
          description="Use a token to authenticate Git over HTTPS."
        >
          <form
            class="flex flex-wrap items-end gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              props.onCreatePat
                ? props.onCreatePat(patName().trim())
                : unavailable();
              setPatName("");
            }}
          >
            <Field
              class="min-w-0 flex-1 basis-40"
              label="Token name"
              value={patName()}
              onValueChange={setPatName}
              placeholder="MacBook CLI"
            />
            <Button type="submit" variant="outline">
              Generate PAT
            </Button>
          </form>
          <Show when={props.lastIssuedPatToken}>
            {(token) => (
              <Alert>
                <AlertDescription>
                  <p class="mb-2 font-medium">
                    Copy your token now. It will not be shown again.
                  </p>
                  <code class="break-all text-xs">{token()}</code>
                </AlertDescription>
              </Alert>
            )}
          </Show>
          <ul class="divide-y">
            <For each={props.livePats}>
              {(pat) => (
                <li class="flex flex-wrap items-center justify-between gap-3 py-2.5">
                  <div class="min-w-0">
                    <p class="break-words text-sm">{pat.name}</p>
                    <code class="text-xs text-muted-foreground">
                      {pat.token_prefix}…
                    </code>
                  </div>
                  <Show
                    when={!pat.revoked && !expired(pat.expires_at)}
                    fallback={
                      <StatusBadge tone={pat.revoked ? "danger" : "warning"}>
                        {pat.revoked ? "Revoked" : "Expired"}
                      </StatusBadge>
                    }
                  >
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => props.onRevokePat?.(pat.id)}
                    >
                      Revoke PAT
                    </Button>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Section>
        <Section
          title="SSH public keys"
          description="Store a public key. Git SSH serving is not yet available. Private keys stay on your machine."
        >
          <form
            class="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              props.onRegisterSshKey
                ? props.onRegisterSshKey(sshTitle().trim(), sshKey().trim())
                : unavailable();
              setSshTitle("");
              setSshKey("");
            }}
          >
            <Field
              label="Key title"
              value={sshTitle()}
              onValueChange={setSshTitle}
              placeholder="Work laptop"
            />
            <Field
              label="OpenSSH public key"
              value={sshKey()}
              onValueChange={setSshKey}
              placeholder="ssh-ed25519 AAAA..."
            />
            <Button type="submit" variant="outline">
              Add SSH key
            </Button>
          </form>
          <ul class="divide-y">
            <For each={props.liveSshKeys}>
              {(key) => (
                <li class="flex flex-wrap items-center justify-between gap-3 py-2.5">
                  <div class="min-w-0">
                    <p class="truncate text-sm" title={key.title}>
                      {key.title}
                    </p>
                    <code class="break-all text-xs text-muted-foreground">
                      {key.fingerprint_sha256}
                    </code>
                  </div>
                  <Show
                    when={!key.revoked}
                    fallback={<StatusBadge tone="danger">Revoked</StatusBadge>}
                  >
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => props.onRevokeSshKey?.(key.id)}
                    >
                      Revoke key
                    </Button>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Section>
      </Show>
    </div>
  );
};
