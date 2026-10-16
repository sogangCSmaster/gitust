import { createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "../components/ui/table";
import {
  Field,
  PersonAvatar,
  Section,
  StatusBadge,
} from "../components/workspace/presentation";
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
  const [identifier, setIdentifier] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [redeemMode, setRedeemMode] = createSignal<"invite" | "reset">(
    "invite",
  );
  const [token, setToken] = createSignal("");
  const signIn = () => {
    if (props.onSignIn) props.onSignIn(identifier().trim(), password());
    else props.onPolicyFeedback("Sign in requires a connected Gitust server.");
    setPassword("");
  };
  return (
    <div class="space-y-6">
      <Section
        title={props.liveSession ? "Your account" : "Sign in to Gitust"}
        description={
          props.liveSession
            ? "Connected to your local Gitust server."
            : "Invite-only access. Your administrator creates the first account with the CLI."
        }
      >
        <Show
          when={props.liveSession}
          fallback={
            <form
              class="grid items-end gap-4 sm:grid-cols-2 xl:grid-cols-[1fr_1fr_auto]"
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
            <div class="flex flex-wrap items-center justify-between gap-4">
              <div class="flex min-w-0 items-center gap-3">
                <PersonAvatar name={session().username} class="size-10" />
                <div class="min-w-0">
                  <p class="break-words font-medium">@{session().username}</p>
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
      <EnterpriseCredentialsPanel
        allowedDomain="your-company.example"
        redeemMode={props.redeemMode ?? redeemMode()}
        onChangeRedeemMode={(mode) =>
          props.onChangeRedeemMode
            ? props.onChangeRedeemMode(mode)
            : setRedeemMode(mode)
        }
        redeemToken={props.redeemToken ?? token()}
        onRedeemTokenInput={(value) =>
          props.onRedeemTokenInput
            ? props.onRedeemTokenInput(value)
            : setToken(value)
        }
        onRedeemInvite={props.onRedeemInvite}
        onRedeemPasswordReset={props.onRedeemPasswordReset}
        liveInvites={props.liveInvites}
        lastIssuedInviteUrl={props.lastIssuedInviteUrl}
        onIssueInvite={props.onIssueInvite}
        onRevokeInvite={props.onRevokeInvite}
        livePats={props.livePats}
        lastIssuedPatToken={props.lastIssuedPatToken}
        onCreatePat={props.onCreatePat}
        onRevokePat={props.onRevokePat}
        liveSshKeys={props.liveSshKeys}
        onRegisterSshKey={props.onRegisterSshKey}
        onRevokeSshKey={props.onRevokeSshKey}
        onPolicyFeedback={props.onPolicyFeedback}
      />
      <Section
        title="Members"
        description="Account deactivation revokes sessions, personal tokens and registered SSH credentials."
      >
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Account</TableHead>
              <TableHead>Role</TableHead>
              <TableHead>Status</TableHead>
              <TableHead class="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <For each={props.users}>
              {(user) => (
                <TableRow>
                  <TableCell>
                    <div class="flex items-center gap-3">
                      <PersonAvatar name={user.username} />
                      <div>
                        <p class="font-medium">{user.username}</p>
                        <p class="text-xs text-muted-foreground">
                          {user.email}
                        </p>
                        <p class="text-xs text-muted-foreground">
                          {user.liveId
                            ? "Live account · 2FA not available"
                            : "Sample account"}
                        </p>
                      </div>
                    </div>
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        user.liveId
                          ? props.onPolicyFeedback(
                              `Role '${user.role}' for @${user.username} is bound at invite creation`,
                            )
                          : props.onToggleRole(user.id)
                      }
                    >
                      {user.role}
                    </Button>
                  </TableCell>
                  <TableCell>
                    <StatusBadge
                      tone={user.status === "active" ? "success" : "danger"}
                    >
                      {user.status}
                    </StatusBadge>
                  </TableCell>
                  <TableCell>
                    <div class="flex justify-end gap-2">
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
                          if (user.liveId) {
                            props.onDeactivateLiveUser?.(user.liveId);
                          } else props.onToggleStatus(user.id);
                        }}
                      >
                        {user.status === "active" ? "Deactivate" : "Reactivate"}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              )}
            </For>
          </TableBody>
        </Table>
      </Section>
      <EnterprisePolicyPreview
        liveMode={props.liveMode}
        onPolicyFeedback={props.onPolicyFeedback}
      />
    </div>
  );
};
