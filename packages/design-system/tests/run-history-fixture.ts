export const worktreeId = "11111111-1111-4111-8111-111111111111";
export const historyRecord = (
  runId = "22222222-2222-4222-8222-222222222222",
) => ({
  completed_at: "2026-10-17T01:00:00Z",
  user_id: "33333333-3333-4333-8333-333333333333",
  verification_requested: true,
  checked_commit: "a".repeat(40),
  checks_blob_oid: "b".repeat(40),
  history_truncated: false,
  result: {
    run_id: runId,
    worktree_id: worktreeId,
    command: "printf checked",
    exit_code: 0,
    duration_ms: 12,
    stdout: "checked",
    stderr: "",
    output_truncated: false,
    steps: [
      {
        command: "printf checked",
        required: false,
        exit_code: 0,
        duration_ms: 12,
      },
    ],
  },
});
