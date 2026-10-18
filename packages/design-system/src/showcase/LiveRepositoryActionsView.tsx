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

const LiveWorkflowsView = lazy(() =>
  import("./LiveWorkflowsView").then((module) => ({
    default: module.LiveWorkflowsView,
  })),
);

export interface RepositoryActionsProps {
  repositoryId?: string;
  role?: string;
  isPrimary: boolean;
  selectionEpoch: number;
  canRunChecks?: boolean;
  canViewLocalChecksAutomatic?: boolean;
  selectedBranch?: string;
  defaultRef?: string;
  defaultCommit?: string | null;
  refreshRepository?: (isCurrent: () => boolean) => Promise<boolean>;
}

export const LiveRepositoryActionsView: Component<RepositoryActionsProps> = (
  props,
) => {
  const defaultMode = () => (props.isPrimary ? "automatic" : "local");
  const [mode, setMode] = createSignal(defaultMode());
  // Unrelated catalog/account refreshes must not restart diagnostic requests or change the selected tab.
  const repositoryId = createMemo(() => props.repositoryId);
  const role = createMemo(() => props.role);
  const primary = createMemo(() => props.isPrimary);
  const epoch = createMemo(() => props.selectionEpoch);
  const canRun = createMemo(() => props.canRunChecks === true);
  const canViewAutomatic = createMemo(
    () => props.canViewLocalChecksAutomatic === true,
  );
  const selectedBranch = createMemo(() => props.selectedBranch);
  const defaultRef = createMemo(() => props.defaultRef);
  const defaultCommit = createMemo(() => props.defaultCommit);
  const canRead = () =>
    role() === "SiteAdmin" &&
    (primary() || canRun() || canViewAutomatic());
  createEffect(() => {
    repositoryId();
    epoch();
    role();
    setMode(primary() ? "automatic" : "local");
  });
  return (
    <Tabs
      value={mode()}
      onChange={(value) => setMode(canRead() ? value : defaultMode())}
    >
      <Show when={canRead()}>
        <TabsList class="w-full sm:w-auto" aria-label="Checks diagnostics">
          <Show when={primary()}>
            <TabsTrigger value="automatic" class="flex-1">
              Automatic
            </TabsTrigger>
          </Show>
          <TabsTrigger value="local" class="flex-1">
            Local Checks
          </TabsTrigger>
          <TabsTrigger value="workflows" class="flex-1">
            Workflows
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
              canRunChecks={canRun()}
              canViewLocalChecksAutomatic={canViewAutomatic()}
              selectedBranch={selectedBranch()}
              defaultRef={defaultRef()}
              defaultCommit={defaultCommit()}
              refreshRepository={props.refreshRepository}
            />
          </DeferredView>
        </Show>
      </TabsContent>
      <TabsContent value="workflows">
        <Show when={mode() === "workflows" && canRead()}>
          <DeferredView label="Workflows">
            <LiveWorkflowsView
              repositoryId={repositoryId()}
              role={role()}
              defaultRef={defaultRef()}
              defaultCommit={defaultCommit()}
            />
          </DeferredView>
        </Show>
      </TabsContent>
    </Tabs>
  );
};
