import {
  createEffect,
  createSignal,
  onCleanup,
  onMount,
  type Component,
} from "solid-js";
import { RunHistoryDialog } from "../patterns/RunHistoryDialog";
import {
  createLiveRunHistoryClient,
  type RunHistoryState,
} from "./live-run-history-client";

export const LiveRunHistoryDialog: Component<{
  worktreeId: string;
  branch: string;
  initialRunId?: string;
  onClose: () => void;
}> = (props) => {
  const [state, setState] = createSignal<RunHistoryState>({
    status: "idle",
    records: [],
  });
  const client = createLiveRunHistoryClient(setState);
  createEffect(() => {
    void client.load(props.worktreeId);
  });
  const close = () => {
    client.clear();
    props.onClose();
  };
  const sessionChanged = (event: StorageEvent) => {
    if (event.key === "gitust_token" || event.key === null) {
      close();
    }
  };
  onMount(() => window.addEventListener("storage", sessionChanged));
  onCleanup(() => {
    client.clear();
    window.removeEventListener("storage", sessionChanged);
  });
  return (
    <RunHistoryDialog
      branch={props.branch}
      state={state()}
      initialRunId={props.initialRunId}
      onRefresh={() => void client.load(props.worktreeId)}
      onClose={close}
    />
  );
};
