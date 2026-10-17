import {
  createEffect,
  createMemo,
  createSignal,
  lazy,
  Show,
  type Component,
} from "solid-js";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../components/ui/tabs";
import { DeferredView } from "../components/workspace/deferred-view";
import { LiveActionsView } from "./LiveActionsView";

const LiveLocalChecksView = lazy(() =>
  import("./LiveLocalChecksView").then((module) => ({
    default: module.LiveLocalChecksView,
  })),
);

export interface RepositoryActionsProps {
  repositoryId?: string;
  role?: string;
  isPrimary: boolean;
  selectionEpoch: number;
}

export const LiveRepositoryActionsView: Component<RepositoryActionsProps> = (
  props,
) => {
  const [mode, setMode] = createSignal("automatic");
  // Unrelated catalog/account refreshes must not restart diagnostic requests or change the selected tab.
  const repositoryId = createMemo(() => props.repositoryId);
  const role = createMemo(() => props.role);
  const primary = createMemo(() => props.isPrimary);
  const epoch = createMemo(() => props.selectionEpoch);
  const canRead = () => primary() && role() === "SiteAdmin";
  createEffect(() => {
    repositoryId();
    epoch();
    role();
    primary();
    setMode("automatic");
  });
  return (
    <Tabs
      value={mode()}
      onChange={(value) => setMode(canRead() ? value : "automatic")}
    >
      <Show when={canRead()}>
        <TabsList class="w-full sm:w-auto" aria-label="Checks diagnostics">
          <TabsTrigger value="automatic" class="flex-1">
            Automatic
          </TabsTrigger>
          <TabsTrigger value="local" class="flex-1">
            Local Checks
          </TabsTrigger>
        </TabsList>
      </Show>
      <TabsContent value="automatic">
        <Show when={primary()}>
          <LiveActionsView repositoryId={repositoryId()} role={role()} />
        </Show>
      </TabsContent>
      <TabsContent value="local">
        <Show when={mode() === "local" && canRead()}>
          <DeferredView label="Local Checks">
            <LiveLocalChecksView
              repositoryId={repositoryId()}
              role={role()}
              isPrimary={primary()}
              selectionEpoch={epoch()}
            />
          </DeferredView>
        </Show>
      </TabsContent>
    </Tabs>
  );
};
