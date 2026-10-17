// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { VerificationResultDetails } from "../src/patterns/VerificationResultDetails";
import { parseVerificationResult } from "../src/showcase/live-verification-result";
import { parseRunHistory } from "../src/showcase/live-run-history-client";
import { historyRecord, worktreeId } from "./run-history-fixture";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
const output = {
  stdout: "step-only-out",
  stderr: "<img src=x onerror=bad()>",
  truncated: true,
};
const raw = (value: unknown = output) => {
  const entry = historyRecord();
  return {
    ...entry,
    result: {
      ...entry.result,
      steps: [{ ...entry.result.steps[0], output: value }],
    },
  };
};

it("retains command-specific output in current and historical results", () => {
  const current = parseVerificationResult(
    raw().result,
    worktreeId,
    "printf checked",
  );
  const historical = parseRunHistory([raw()], worktreeId)[0].result;
  expect(current.steps?.[0]).toHaveProperty("output", output);
  expect(historical.steps?.[0]).toHaveProperty("output", output);
});

it("rejects malformed, oversized or skipped-step output while accepting legacy omissions", () => {
  for (const value of [
    null,
    {},
    { ...output, truncated: "true" },
    { ...output, stderr: "한".repeat(700) },
  ]) {
    expect(() => parseRunHistory([raw(value)], worktreeId)).toThrow();
  }
  expect(() =>
    parseVerificationResult(
      raw({ ...output, stdout: "x".repeat(3 * 1024 * 1024 + 1) }).result,
      worktreeId,
      "printf checked",
    ),
  ).toThrow();
  const skipped = raw();
  skipped.result.exit_code = 7;
  skipped.result.steps = [
    { ...skipped.result.steps[0], exit_code: 7 },
    {
      ...skipped.result.steps[0],
      required: true,
      exit_code: null,
      duration_ms: null,
    },
  ] as typeof skipped.result.steps;
  expect(() => parseRunHistory([skipped], worktreeId)).toThrow();
  expect(
    parseRunHistory([historyRecord()], worktreeId)[0].result.steps?.[0],
  ).not.toHaveProperty("output");
});

it("expands only the selected command's plain-text stdout and stderr", async () => {
  const result = parseRunHistory([raw()], worktreeId)[0].result;
  render(() => <VerificationResultDetails result={result} />);
  expect(screen.queryByText("step-only-out", { exact: true })).toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "Command output" }));
  expect(
    await screen.findByText("step-only-out", { exact: true }),
  ).toBeTruthy();
  expect(screen.getByText(output.stderr, { exact: true })).toBeTruthy();
  expect(screen.getByText("Command output was shortened")).toBeTruthy();
  expect(document.querySelector("img")).toBeNull();
  expect(screen.getByText("checked", { exact: true })).toBeTruthy();
});
