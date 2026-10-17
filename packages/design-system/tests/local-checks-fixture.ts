export const localRepoId = "99999999-9999-4999-8999-999999999999";
export const localRecord = (
  runId = "aaaa1111-1111-4111-8111-111111111111",
) => ({
  completed_at: "2026-10-18T01:00:00.123456789Z",
  source_ref: "refs/heads/topic/local-checks",
  checked_commit: "e".repeat(40),
  checks_blob_oid: "f".repeat(40),
  status: "failed",
  history_truncated: true,
  result: {
    run_id: runId,
    worktree_id: "bbbb1111-1111-4111-8111-111111111111",
    command: "printf local; exit 7 && printf skipped",
    exit_code: 7,
    duration_ms: 18,
    stdout: "local output",
    stderr: "",
    output_truncated: false,
    backend: "macos_seatbelt",
    network_isolated: true,
    steps: [
      {
        command: "printf local; exit 7",
        required: true,
        exit_code: 7,
        duration_ms: 18,
        output: { stdout: "local output", stderr: "", truncated: false },
      },
      {
        command: "printf skipped",
        required: true,
        exit_code: null,
        duration_ms: null,
      },
    ],
  },
});

export const localPage = () => ({
  version: 1,
  repo_id: localRepoId,
  records: [localRecord()],
});
