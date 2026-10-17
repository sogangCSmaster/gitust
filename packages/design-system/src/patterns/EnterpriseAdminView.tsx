import {
  createEffect,
  createSignal,
  For,
  mergeProps,
  on,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../components/ui/tabs";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import {
  Field,
  PersonAvatar,
  Section,
  StatusBadge,
} from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import { EnterprisePolicyPreview } from "./EnterprisePolicyPreview";
import {
  EnterpriseCredentialsPanel,
  type LiveInviteItem,
  type LivePatItem,
  type LiveSshKeyItem,
} from "./EnterpriseCredentialsPanel";

export interface EnterpriseUser {
  id: number;
  liveId?: string;
  username: string;
  email: string;
  department: string;
  role: "Site Admin" | "Maintainer" | "Developer";
  twoFactorEnabled: boolean;
  status: "active" | "suspended";
}

export interface LiveAuthSessionInfo {
  user_id: string;
  username: string;
  email: string;
  role: "SiteAdmin" | "Maintainer" | "Developer";
}

export interface EnterpriseAdminViewProps {
  users: EnterpriseUser[];
  onToggleRole: (userId: number) => void;
  onToggleStatus: (userId: number) => void;
  onPolicyFeedback: (msg: string) => void;
  liveMode?: boolean;
  liveSession?: LiveAuthSessionInfo | null;
  onSignIn?: (identifier: string, password: string) => void;
  onSignOut?: () => void;
  redeemMode?: "invite" | "reset";
  onChangeRedeemMode?: (mode: "invite" | "reset") => void;
  redeemToken?: string;
  onRedeemTokenInput?: (val: string) => void;
  onRedeemInvite?: (token: string, username: string, password: string) => void;
  onRedeemPasswordReset?: (token: string, newPassword: string) => void;
  liveInvites?: LiveInviteItem[];
  lastIssuedInviteUrl?: string | null;
  onIssueInvite?: (
    targetEmail: string,
    role: "SiteAdmin" | "Maintainer" | "Developer",
  ) => void;
  onRevokeInvite?: (inviteId: string) => void;
  onIssuePasswordReset?: (userId: string) => void;
  onDeactivateLiveUser?: (userId: string) => void;
  livePats?: LivePatItem[];
  lastIssuedPatToken?: string | null;
  onCreatePat?: (name: string) => void;
  onRevokePat?: (patId: string) => void;
  liveSshKeys?: LiveSshKeyItem[];
  onRegisterSshKey?: (title: string, publicKeyOpenssh: string) => void;
  onRevokeSshKey?: (keyId: string) => void;
}

export const EnterpriseAdminView: Component<EnterpriseAdminViewProps> = (
  props,
) => {
  const [tab, setTab] = createSignal("account");
  const [identifier, setIdentifier] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [redeemMode, setRedeemMode] = createSignal<"invite" | "reset">(
    "invite",
  );
  const [token, setToken] = createSignal("");
  const [redeemOpen, setRedeemOpen] = createSignal(Boolean(props.redeemToken));
  createEffect(
    on(
      () => props.redeemToken,
      (value) => {
        if (value?.trim()) {
          setTab("account");
          setRedeemOpen(true);
        }
      },
    ),
  );
  const credentials = mergeProps(props, {
    allowedDomain: "your-company.example",
    get redeemMode() {
      return props.redeemMode ?? redeemMode();
    },
    onChangeRedeemMode: (mode: "invite" | "reset") =>
      props.onChangeRedeemMode
        ? props.onChangeRedeemMode(mode)
        : setRedeemMode(mode),
    get redeemToken() {
      return props.redeemToken ?? token();
    },
    onRedeemTokenInput: (value: string) =>
      props.onRedeemTokenInput
        ? props.onRedeemTokenInput(value)
        : setToken(value),
  });
  const signIn = () => {
    if (props.onSignIn) props.onSignIn(identifier().trim(), password());
    else props.onPolicyFeedback("Sign in requires a connected Gitust server.");
    setPassword("");
  };
  return (
    <Tabs value={tab()} onChange={setTab} class="min-w-0">
      <TabsList
        aria-label="Access sections"
        class="mb-3 w-full justify-start sm:w-auto"
      >
        <TabsTrigger value="account" class="flex-1">
          Account
        </TabsTrigger>
        <TabsTrigger value="credentials" class="flex-1">
          Credentials
        </TabsTrigger>
        <TabsTrigger value="members" class="flex-1">
          Members
        </TabsTrigger>
      </TabsList>
      <TabsContent
        value="account"
        forceMount
        hidden={tab() !== "account"}
        class="space-y-3"
      >
        <Section
          title={props.liveSession ? "Your account" : "Sign in to Gitust"}
          description={
            props.liveSession
              ? "Connected to your Gitust server."
              : "Invite-only access. Sign in with your account."
          }
        >
          <Show
            when={props.liveSession}
            fallback={
              <form
                class="grid items-end gap-3 sm:grid-cols-2 xl:grid-cols-[1fr_1fr_auto]"
                onSubmit={(event) => {
                  event.preventDefault();
                  signIn();
                }}
              >
                <Field
                  label="Username or email"
                  value={identifier()}
                  onValueChange={setIdentifier}
                  placeholder="username or email"
                  autocomplete="username"
                />
                <Field
                  label="Password"
                  type="password"
                  value={password()}
                  onValueChange={setPassword}
                  autocomplete="current-password"
                />
                <Button type="submit">Sign in</Button>
              </form>
            }
          >
            {(session) => (
              <div class="flex flex-wrap items-center justify-between gap-3">
                <div class="flex min-w-0 items-center gap-3">
                  <PersonAvatar name={session().username} />
                  <div class="min-w-0">
                    <p class="truncate font-medium" title={session().username}>
                      @{session().username}
                    </p>
                    <p class="break-all text-xs text-muted-foreground">
                      {session().email ? `${session().email} · ` : ""}
                      {session().role}
                    </p>
                  </div>
                </div>
                <Button variant="outline" onClick={() => props.onSignOut?.()}>
                  Sign out
                </Button>
              </div>
            )}
          </Show>
        </Section>
        <Collapsible open={redeemOpen()} onOpenChange={setRedeemOpen}>
          <CollapsibleTrigger
            as={Button}
            variant="ghost"
            size="sm"
            class="max-w-full"
          >
            <Icon name="key" /> Use an invite or reset link
          </CollapsibleTrigger>
          <CollapsibleContent class="pt-2">
            <EnterpriseCredentialsPanel {...credentials} view="redemption" />
          </CollapsibleContent>
        </Collapsible>
        <Collapsible>
          <CollapsibleTrigger as={Button} variant="ghost" size="sm">
            <Icon name="shield-check" /> Security policy preview
          </CollapsibleTrigger>
          <CollapsibleContent class="pt-2">
            <EnterprisePolicyPreview
              liveMode={props.liveMode}
              onPolicyFeedback={props.onPolicyFeedback}
            />
          </CollapsibleContent>
        </Collapsible>
      </TabsContent>
      <TabsContent
        value="credentials"
        forceMount
        hidden={tab() !== "credentials"}
      >
        <EnterpriseCredentialsPanel {...credentials} view="git" />
      </TabsContent>
      <TabsContent
        value="members"
        forceMount
        hidden={tab() !== "members"}
        class="space-y-3"
      >
        <Section
          title="Members"
          description="Deactivation revokes sessions, personal tokens and registered SSH credentials."
        >
          <Show
            when={props.users.length}
            fallback={
              <p class="text-xs text-muted-foreground">
                No members to display.
              </p>
            }
          >
            <ul class="divide-y">
              <For each={props.users}>
                {(user) => (
                  <li class="flex min-w-0 flex-wrap items-center gap-3 py-3">
                    <div class="flex min-w-0 flex-1 basis-48 items-center gap-2.5">
                      <PersonAvatar name={user.username} />
                      <div class="min-w-0">
                        <p class="truncate font-medium" title={user.username}>
                          {user.username}
                        </p>
                        <p
                          class="truncate text-xs text-muted-foreground"
                          title={user.email}
                        >
                          {user.email}
                        </p>
                      </div>
                    </div>
                    <div class="flex items-center gap-2 text-xs">
                      <Show
                        when={user.liveId}
                        fallback={
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => props.onToggleRole(user.id)}
                          >
                            {user.role}
                          </Button>
                        }
                      >
                        <span>{user.role}</span>
                      </Show>
                      <StatusBadge
                        tone={user.status === "active" ? "success" : "danger"}
                      >
                        {user.status}
                      </StatusBadge>
                    </div>
                    <div class="ml-auto flex flex-wrap justify-end gap-1">
                      <Show when={user.liveId && props.onIssuePasswordReset}>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() =>
                            user.liveId &&
                            props.onIssuePasswordReset?.(user.liveId)
                          }
                        >
                          Reset link
                        </Button>
                      </Show>
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={Boolean(
                          user.liveId && user.status !== "active",
                        )}
                        onClick={() => {
                          if (user.liveId)
                            props.onDeactivateLiveUser?.(user.liveId);
                          else props.onToggleStatus(user.id);
                        }}
                      >
                        {user.status === "active" ? "Deactivate" : "Reactivate"}
                      </Button>
                    </div>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Section>
        <EnterpriseCredentialsPanel {...credentials} view="invitations" />
      </TabsContent>
    </Tabs>
  );
};
