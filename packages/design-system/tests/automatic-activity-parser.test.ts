import { expect, it } from "vitest";
import { parseAutomaticRuns } from "../src/showcase/live-automatic-runs";
import { parseRunHistory } from "../src/showcase/live-run-history-client";
import { mergeRunActivity } from "../src/patterns/run-activity-model";
import { activityLabel } from "../src/patterns/run-activity-presentation";
import { automaticJob, historyRecord, worktreeId } from "./run-history-fixture";
const id = (n: number) =>
  `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
it("binds route identity, preserves all lifecycle states and enforces independent active/terminal caps", () => {
  const raw = [
    "prepared",
    "queued",
    "running",
    "completed",
    "failed",
    "timed_out",
    "cancelled",
    "interrupted",
  ].map((phase, i) => ({
    ...automaticJob(id(i + 1)),
    phase,
    reason: i < 4 ? null : "execution_failed",
  }));
  const parsed = parseAutomaticRuns(raw, worktreeId);
  expect(parsed).toHaveLength(8);
  expect(parsed[0]?.worktreeId).toBe(worktreeId);
  expect(() =>
    parseAutomaticRuns([...raw, automaticJob(id(9))], worktreeId),
  ).toThrow();
  expect(() =>
    parseAutomaticRuns([automaticJob(id(1)), automaticJob(id(2))], worktreeId),
  ).toThrow();
  const terminal = Array.from({ length: 6 }, (_, i) => ({
    ...automaticJob(id(i + 1)),
    phase: "completed",
  }));
  expect(() => parseAutomaticRuns(terminal, worktreeId)).toThrow();
});
it("rejects malformed enum, purpose, identity, timestamp, version drift and phase/reason metadata", () => {
  const bad = [
    { origin: "web" },
    { purpose: "login" },
    { phase: "passed" },
    { reason: "secret raw error" },
    { run_id: "00000000-0000-0000-0000-000000000000" },
    { user_id: "admin" },
    { expected: "0".repeat(40) },
    { previous: "branch/main" },
    { checks_oid: "b".repeat(39) },
    { created_at: "2026-02-30T00:00:00Z" },
    { changed_at: "2026-10-17T01:00:00Z" },
    { phase: "interrupted", reason: null },
    { phase: "completed", reason: "command_failed" },
    { command: "never accepted" },
    { version: 2 },
    { worktree_id: worktreeId },
  ];
  for (const patch of bad)
    expect(() =>
      parseAutomaticRuns([{ ...automaticJob(), ...patch }], worktreeId),
    ).toThrow();
  const mixed = "abcdefab-cdef-4abc-8def-abcdefabcdef";
  expect(() =>
    parseAutomaticRuns(
      [
        automaticJob(mixed),
        { ...automaticJob(mixed.toUpperCase()), phase: "running" },
      ],
      worktreeId,
    ),
  ).toThrow();
  expect(() =>
    parseRunHistory(
      [historyRecord(mixed), historyRecord(mixed.toUpperCase())],
      worktreeId,
    ),
  ).toThrow();
  expect(() =>
    parseAutomaticRuns(
      [
        {
          ...automaticJob(),
          created_at: "2026-10-17T02:00:00.000000002Z",
          changed_at: "2026-10-17T02:00:00.000000001Z",
        },
      ],
      worktreeId,
    ),
  ).toThrow();
});
it("merges matching results once and rejects actor, SHA, policy or manual-purpose collisions", () => {
  const wire = {
    ...historyRecord(),
    automatic: true,
    verification_requested: false,
  };
  const history = parseRunHistory([wire], worktreeId);
  const raw = {
    ...automaticJob(wire.result.run_id),
    phase: "failed",
    reason: "authority_unavailable",
  };
  const jobs = parseAutomaticRuns([raw], worktreeId);
  const combined = mergeRunActivity(history, jobs);
  expect(combined).toHaveLength(1);
  expect(combined[0]?.history?.result.exitCode).toBe(0);
  expect(activityLabel(combined[0]!)).toBe("Failed");
  for (const patch of [
    { user_id: id(33) },
    { expected: "c".repeat(40) },
    { checks_oid: "c".repeat(40) },
  ]) {
    expect(() =>
      mergeRunActivity(
        history,
        parseAutomaticRuns([{ ...raw, ...patch }], worktreeId),
      ),
    ).toThrow();
  }
  expect(() =>
    mergeRunActivity(parseRunHistory([historyRecord()], worktreeId), jobs),
  ).toThrow();
  const onlyLifecycle = mergeRunActivity(
    [],
    parseAutomaticRuns(
      [{ ...raw, phase: "completed", reason: null }],
      worktreeId,
    ),
  );
  expect(activityLabel(onlyLifecycle[0]!)).toBe("Completed");
  expect(onlyLifecycle[0]?.history).toBeUndefined();
});
it("retains at most thirteen unique activity rows without fabricating missing command results", () => {
  const history = parseRunHistory(
    Array.from({ length: 5 }, (_, i) => historyRecord(id(i + 1))),
    worktreeId,
  );
  const jobs = parseAutomaticRuns(
    [
      "prepared",
      "queued",
      "running",
      "completed",
      "completed",
      "completed",
      "completed",
      "completed",
    ].map((phase, i) => ({ ...automaticJob(id(i + 10)), phase })),
    worktreeId,
  );
  expect(mergeRunActivity(history, jobs)).toHaveLength(13);
});
