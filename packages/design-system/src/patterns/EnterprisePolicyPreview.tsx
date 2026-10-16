import { createSignal, For, type Component } from "solid-js";
import {
  Switch,
  SwitchControl,
  SwitchThumb,
  SwitchLabel,
  SwitchDescription,
} from "../components/ui/switch";
import {
  Field,
  Section,
  StatusBadge,
} from "../components/workspace/presentation";
export const EnterprisePolicyPreview: Component<{
  liveMode?: boolean;
  onPolicyFeedback: (message: string) => void;
}> = (props) => {
  const [inviteOnly, setInviteOnly] = createSignal(true),
    [twoFactor, setTwoFactor] = createSignal(true),
    [sso, setSso] = createSignal(false),
    [domain, setDomain] = createSignal("company.internal");
  const policies = [
    {
      label: "Invite-only accounts",
      description: "Accounts are created through administrator-issued links.",
      get: inviteOnly,
      set: setInviteOnly,
    },
    {
      label: "Two-factor authentication",
      description:
        "Planned policy preview. Not enforced by the current server.",
      get: twoFactor,
      set: setTwoFactor,
    },
    {
      label: "Corporate SSO",
      description:
        "Planned OIDC/SAML integration. Not connected to the server.",
      get: sso,
      set: setSso,
    },
  ];
  return (
    <Section
      title="Security policy preview"
      description="Demonstration controls only. Changes here are never applied to the server."
    >
      <StatusBadge>Planned · Demo</StatusBadge>
      <div class="divide-y">
        <For each={policies}>
          {(policy) => (
            <Switch
              class="flex items-center justify-between gap-6 py-4"
              checked={policy.get()}
              disabled={props.liveMode}
              onChange={(checked) => {
                if (props.liveMode) return;
                policy.set(checked);
                props.onPolicyFeedback(
                  `Demo policy preview: ${policy.label} → ${checked ? "ENABLED" : "DISABLED"} · not applied to the server`,
                );
              }}
            >
              <div class="space-y-2">
                <SwitchLabel>{policy.label}</SwitchLabel>
                <SwitchDescription class="text-xs text-muted-foreground">
                  {policy.description}
                </SwitchDescription>
              </div>
              <SwitchControl>
                <SwitchThumb />
              </SwitchControl>
            </Switch>
          )}
        </For>
      </div>
      <Field
        label="Allowed email domain (preview)"
        disabled={props.liveMode}
        value={domain()}
        onValueChange={setDomain}
      />
    </Section>
  );
};
