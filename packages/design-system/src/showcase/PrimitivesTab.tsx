import { createSignal, For, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Badge } from "../components/ui/badge";
import {
  Switch,
  SwitchControl,
  SwitchLabel,
  SwitchThumb,
} from "../components/ui/switch";
import { Checkbox } from "../components/ui/checkbox";
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
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogHeader,
  DialogTrigger,
} from "../components/ui/dialog";
import {
  Field,
  KeyHint,
  PageHeading,
  PersonAvatar,
  Section,
  StatusBadge,
} from "../components/workspace/presentation";
import { Icon, ICON_DEFS, type IconName } from "../components/workspace/icon";
export const PrimitivesTab: Component = () => {
  const [notifications, setNotifications] = createSignal(false);
  const [checked, setChecked] = createSignal(false);
  return (
    <div class="space-y-6">
      <PageHeading
        title="Components"
        description="The solid-ui foundation for every workspace view. These controls are interactive design examples."
      />
      <div class="grid gap-6 lg:grid-cols-2">
        <Section
          title="Actions"
          description="One clear primary action, with quieter alternatives."
        >
          <div class="flex flex-wrap gap-2">
            <Button>
              <Icon name="plus" />
              New worktree
            </Button>
            <Button variant="outline">View changes</Button>
            <Button variant="ghost">Cancel</Button>
          </div>
        </Section>
        <Section
          title="Status"
          description="A consistent visual language across reviews and runs."
        >
          <div class="flex flex-wrap gap-2">
            <StatusBadge tone="success">Ready</StatusBadge>
            <StatusBadge tone="warning">Running</StatusBadge>
            <StatusBadge tone="merged">Merged</StatusBadge>
            <Badge variant="secondary">Pending</Badge>
          </div>
        </Section>
        <Section
          title="Inputs"
          description="Labelled controls with keyboard focus and accessible state."
        >
          <Field
            label="Search your workspace"
            hideLabel
            placeholder="Search your workspace..."
          />
          <Switch
            class="flex items-center justify-between"
            checked={notifications()}
            onChange={setNotifications}
          >
            <SwitchLabel>Sample notifications</SwitchLabel>
            <SwitchControl>
              <SwitchThumb />
            </SwitchControl>
          </Switch>
          <div class="flex items-center gap-2">
            <Checkbox
              checked={checked()}
              onChange={setChecked}
              aria-label="Select sample item"
            />
            <span class="text-sm text-muted-foreground">
              Select a sample item
            </span>
          </div>
        </Section>
        <Section title="People & navigation">
          <div class="flex items-center gap-3">
            <PersonAvatar name="Yu Seungjae" />
            <PersonAvatar name="Gitoxide Core" />
            <KeyHint keys="mod+k" />
          </div>
          <Tabs defaultValue="overview">
            <TabsList>
              <TabsTrigger value="overview">Overview</TabsTrigger>
              <TabsTrigger value="activity">Activity</TabsTrigger>
            </TabsList>
            <TabsContent
              value="overview"
              class="pt-3 text-sm text-muted-foreground"
            >
              Your workspace at a glance.
            </TabsContent>
            <TabsContent
              value="activity"
              class="pt-3 text-sm text-muted-foreground"
            >
              Recent changes and reviews.
            </TabsContent>
          </Tabs>
          <Dialog>
            <DialogTrigger as={Button} variant="outline">
              Open sample dialog
            </DialogTrigger>
            <DialogContent class="w-[calc(100%-2rem)] rounded-lg">
              <DialogHeader>
                <DialogTitle>A moment of focus</DialogTitle>
                <DialogDescription>
                  Escape closes this dialog and returns focus to its trigger.
                </DialogDescription>
              </DialogHeader>
            </DialogContent>
          </Dialog>
        </Section>
        <Section title="Icon library" class="lg:col-span-2">
          <Collapsible>
            <CollapsibleTrigger as={Button} variant="ghost" class="-ml-3">
              <Icon name="chevron-right" />
              Browse {Object.keys(ICON_DEFS).length} workspace icons
            </CollapsibleTrigger>
            <CollapsibleContent>
              <div class="grid grid-cols-2 gap-5 pt-5 sm:grid-cols-3 lg:grid-cols-5">
                <For each={Object.keys(ICON_DEFS) as IconName[]}>
                  {(name) => (
                    <div class="flex items-center gap-2 text-xs text-muted-foreground">
                      <Icon name={name} size={18} />
                      <span>{name}</span>
                    </div>
                  )}
                </For>
              </div>
            </CollapsibleContent>
          </Collapsible>
        </Section>
      </div>
    </div>
  );
};
