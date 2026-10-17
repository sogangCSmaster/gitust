import { createEffect, createSignal } from "solid-js";
import type { ConnectedRepository } from "./connected-repository-client";
import { getLiveToken } from "./live-repo-client";
import type { NewWorktreeInput, NewWorktreeResult } from "./new-worktree-input";

export const createNewWorktreeController = (options: {
  getRepository: () => ConnectedRepository | null | undefined;
  liveMode: () => boolean;
  sessionReady: () => boolean;
  create: (input: NewWorktreeInput) => Promise<NewWorktreeResult>;
  onCreated: (taskId: number, worktreeId: string) => void;
}) => {
  const [target, setTarget] = createSignal<ConnectedRepository | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  let epoch = 0;
  let tokenAtOpen: string | null = null;
  const reset = () => {
    epoch++;
    tokenAtOpen = null;
    setSubmitting(false);
    setTarget(null);
  };
  const current = () =>
    Boolean(
      target() &&
      options.liveMode() &&
      options.sessionReady() &&
      target()?.id === options.getRepository()?.id &&
      tokenAtOpen &&
      tokenAtOpen === getLiveToken(),
    );
  createEffect(() => {
    if (target() && !current()) reset();
  });
  const open = () => {
    const repository = options.getRepository();
    if (
      target() ||
      !repository ||
      !options.liveMode() ||
      !options.sessionReady() ||
      !getLiveToken()
    )
      return;
    epoch++;
    tokenAtOpen = getLiveToken();
    setTarget({ ...repository });
  };
  const close = () => {
    if (!submitting()) reset();
  };
  const submit = async (
    input: NewWorktreeInput,
  ): Promise<NewWorktreeResult> => {
    if (!current() || input.repoId !== target()?.id) {
      reset();
      return { kind: "stale" };
    }
    if (submitting())
      return { kind: "error", error: "Creation is already in progress." };
    const ownEpoch = epoch;
    setSubmitting(true);
    try {
      const result = await options.create(input);
      if (ownEpoch !== epoch) return { kind: "stale" };
      if (!current() || result.kind === "stale") {
        reset();
        return { kind: "stale" };
      }
      if (result.kind === "created") {
        reset();
        options.onCreated(result.taskId, result.worktreeId);
      }
      return result;
    } finally {
      if (ownEpoch === epoch) setSubmitting(false);
    }
  };
  return { target, submitting, open, close, reset, submit };
};
