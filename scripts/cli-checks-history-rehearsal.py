#!/usr/bin/env python3
"""Verify opt-in local Checks history using disposable repositories and relocated backup."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import uuid


def load_script(name):
    path = Path(__file__).with_name(name)
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Rehearsal:
    def __init__(self, fixture, root):
        self.f, self.h, self.root = fixture, fixture.h, root
        self.repo = fixture.repo
        self.ids = []

    def query(self, repo, run_id=None, *, expected=0, status="ok"):
        args = ["sandbox", "checks-history", "--repo", repo]
        if run_id is not None:
            args += ["--run-id", run_id]
        report = json.loads(self.f.cli(*args, expected=expected))
        self.h.require(set(report) == {"version", "status", "reason", "repo_id", "records"}
                       and report["version"] == 1 and report["status"] == status,
                       "history query has an unexpected result contract")
        if status == "ok":
            self.h.require(report["reason"] is None and isinstance(report["records"], list)
                           and len(report["records"]) <= 5,
                           "history query hid an error or exceeded retention")
        elif status == "not_found":
            self.h.require(report["reason"] is None and report["records"] == [],
                           "missing history result has an unexpected error or retained record")
        else:
            self.h.require(isinstance(report["reason"], str) and bool(report["reason"])
                           and report["records"] == [], "history query hid a missing or unavailable result")
        return report

    def record(self, repo, *, exit_code=0):
        before = self.f.state(repo)
        sha = self.f.git(repo, "rev-parse", "HEAD")
        report = json.loads(self.f.cli("sandbox", "checks", "--repo", repo,
                                      "--expected-commit", sha, "--timeout", "30", "--record",
                                      expected=exit_code))
        self.h.require(set(report) == {"version", "status", "reason", "source_ref", "checked_commit",
                                       "checks_blob_oid", "result", "cleanup", "history"}
                       and report["version"] == 2 and report["reason"] is None
                       and report["status"] == ("passed" if exit_code == 0 else "failed")
                       and report["cleanup"] == "confirmed" and report["checked_commit"] == sha
                       and report["source_ref"] == before[0]
                       and report["checks_blob_oid"] == self.f.git(repo, "rev-parse", "HEAD:.gitust-checks.json"),
                       "recorded Checks do not describe the executed committed snapshot")
        history, result = report["history"], report["result"]
        self.h.require(history["status"] == "saved" and history["reason"] is None
                       and history["run_id"] == result["run_id"] and result["exit_code"] == exit_code
                       and result["network_isolated"] is True
                       and all(step["required"] is True for step in result["steps"]),
                       "recorded result lacks actual all-required execution and durable-save confirmation")
        identity = json.loads((self.f.common(repo) / "gitust-repository/identity.json").read_text())
        self.h.require(history["repo_id"] == identity["repo_id"], "history is bound to a different repository identity")
        self.h.require(self.f.state(repo) == before, "recording changed original Git, tasks, approval or scratch state")
        common = self.f.common(repo)
        self.h.require(not list((common / "gitust-active-executions").glob("*.json")),
                       "confirmed recorded run left an execution fence")
        directory = common / "gitust-local-check-runs"
        metadata = (directory / "history.json").stat()
        self.h.require(directory.stat().st_mode & 0o7777 == 0o700
                       and metadata.st_mode & 0o7777 == 0o600 and metadata.st_size <= 3 * 1024 * 1024,
                       "history is not private and bounded on disk")
        stored = self.query(repo, result["run_id"])
        self.h.require(stored["repo_id"] == identity["repo_id"] and len(stored["records"]) == 1,
                       "a fresh process could not retrieve the recorded run")
        row = stored["records"][0]
        self.h.require(row["result"]["run_id"] == result["run_id"]
                       and row["source_ref"] == report["source_ref"]
                       and row["checked_commit"] == report["checked_commit"]
                       and row["checks_blob_oid"] == report["checks_blob_oid"]
                       and row["status"] == report["status"], "stored result changed the run identity or outcome")
        self.h.require(row["result"]["backend"] == result["backend"]
                       and row["result"]["duration_ms"] == result["duration_ms"]
                       and [s["exit_code"] for s in row["result"]["steps"]]
                       == [s["exit_code"] for s in result["steps"]], "history fabricated execution measurements")
        self.f.backends.add(result["backend"])
        return report, row

    def execute(self):
        self.f.setup()
        identity_path = self.f.common(self.repo) / "gitust-repository"
        self.h.require(not identity_path.exists(), "plain CLI fixture unexpectedly initialized repository identity")
        self.query(self.repo, expected=2, status="rejected")
        self.h.require(not identity_path.exists(), "read-only history query created repository identity")
        long_commands = ["awk 'BEGIN { for (i=0; i<10000; i++) printf \"한\" }'", "printf second-step"]
        self.f.commit_config(json.dumps({"version": 1, "commands": long_commands}), "test: bounded history output")
        actual, stored = self.record(self.repo)
        first = actual["result"]["run_id"]
        self.ids.append(first)
        self.h.require(len(actual["result"]["stdout"].encode()) > 16384
                       and stored["history_truncated"] is True
                       and len(stored["result"]["stdout"].encode()) <= 16384
                       and len(stored["result"]["steps"][0]["output"]["stdout"].encode()) <= 1024,
                       "history did not bound UTF-8 output independently of the live result")
        failed = ["printf expected-failure; exit 7", "printf skipped"]
        self.f.commit_config(json.dumps({"version": 1, "commands": failed}), "test: retained failed run")
        actual, stored = self.record(self.repo, exit_code=7)
        self.ids.append(actual["result"]["run_id"])
        self.h.require([s["exit_code"] for s in stored["result"]["steps"]] == [7, None],
                       "retained failure lost its skipped step")
        self.f.commit_config(self.f.good_config, "test: successful retained runs")
        for _ in range(4):
            actual, _stored = self.record(self.repo)
            self.ids.append(actual["result"]["run_id"])
        history = self.query(self.repo)
        self.h.require([r["result"]["run_id"] for r in history["records"]] == list(reversed(self.ids[-5:])),
                       "history is not the newest five distinct runs in append order")
        self.query(self.repo, first, expected=2, status="not_found")
        self.query(self.repo, str(uuid.uuid4()), expected=2, status="not_found")
        current = self.f.git(self.repo, "rev-parse", "HEAD")
        self.f.run(self.repo, current, status="passed", exit_code=0,
                   commands=self.f.good_commands, step_exits=[0, 0])
        self.h.require(self.query(self.repo) == history, "plain v1 execution unexpectedly recorded or changed history")
        bare = self.root / "separate.git"
        self.f.git(self.root, "clone", "--bare", "--no-local", str(self.repo), str(bare))
        actual, _stored = self.record(bare)
        self.h.require(actual["history"]["repo_id"] != history["repo_id"]
                       and len(self.query(bare)["records"]) == 1,
                       "independent bare repository inherited another repository's local history")
        self.f.git(bare, "fsck", "--full", "--strict")
        self.restore(history)
        return {"version": 1, "passed": True, "actual_repositories": 2, "restored_repositories": 1,
                "query_does_not_initialize_identity": True, "recorded_success_and_failure_reopened": True,
                "exact_ref_commit_blob_and_run_binding": True, "actual_measurements_and_skipped_steps_preserved": True,
                "bounded_utf8_output_preserves_live_result": True, "newest_five_retained_and_evicted_ids_missing": True,
                "plain_v1_execution_does_not_record": True, "separate_bare_repository_identity": True,
                "history_storage_private_and_bounded": True,
                "real_backup_and_relocated_history": True, "original_approval_and_origins_preserved": True,
                "temporary_checkouts_and_scratch_removed": True, "git_fsck": True,
                "platform": os.uname().sysname, "sandbox_backends": sorted(self.f.backends),
                "external_github_publication": False}

    def restore(self, history):
        auth = self.root / "auth/auth-db.json"
        self.h.command([str(self.f.binary), "admin", "create", "--username", "history-admin",
                        "--email", "history-admin@company.internal", "--auth-db", str(auth)],
                       env={"GITUST_ADMIN_PASSWORD": secrets.token_urlsafe(24)})
        script = Path(__file__).with_name("backup.py").resolve()
        backup, restored = self.root / "backup", self.root / "restored"
        self.h.command([sys.executable, str(script), "create", "--repo", str(self.repo),
                        "--auth-db", str(auth), "--output", str(backup)])
        before = self.f.authority(self.repo)
        tag = self.f.git(self.repo, "rev-parse", "refs/tags/preserved-tag")
        base = self.repo.parent / ("." + self.repo.name + ".gitust-worktrees")
        self.repo.rename(self.root / "unavailable-original")
        base.rename(self.root / "unavailable-worktrees")
        auth.parent.rename(self.root / "unavailable-auth")
        self.h.command([sys.executable, str(script), "restore", "--backup", str(backup),
                        "--destination", str(restored)])
        repo = restored / "repository"
        self.h.require(self.query(repo) == history, "relocated history differs from the original repository checkpoint")
        self.h.require(self.f.authority(repo) == before == self.f.before_authority
                       and self.f.git(repo, "rev-parse", "refs/tags/preserved-tag") == tag
                       and self.f.git(repo, "cat-file", "-t", tag) == "tag",
                       "relocated history changed existing authority or annotated tags")
        for row in self.f.worktrees(repo):
            self.f.git(Path(row["worktree"]), "fsck", "--full", "--strict")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    local = load_script("cli-checks-rehearsal.py")
    h = local.helper_module()
    source, binary = args.source.resolve(strict=True), args.binary.resolve(strict=True)
    h.require(not args.report.exists() and not args.report.is_symlink(), "report path already exists")
    commit, dirty = h.git(source, "rev-parse", "HEAD"), bool(h.git(source, "status", "--porcelain"))
    names = [Path(__file__).name, "cli-checks-rehearsal.py", "alpha-rehearsal.py", "backup.py"]
    paths = [Path(__file__).with_name(name).resolve() for name in names]
    runtime = {"scripts/" + p.name: h.sha256(p) for p in paths}
    for name, digest in runtime.items():
        old = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                             env=h.clean_environment(), capture_output=True, timeout=30)
        dirty |= old.returncode != 0 or hashlib.sha256(old.stdout).hexdigest() != digest
    binary_sha = h.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-cli-history-") as directory:
        scratch = Path(directory).resolve()
        report = Rehearsal(local.Rehearsal(h, binary, scratch), scratch).execute()
    h.require(not scratch.exists(), "private fixture was not removed")
    h.require(runtime == {"scripts/" + p.name: h.sha256(p) for p in paths}
              and h.sha256(binary) == binary_sha and h.git(source, "rev-parse", "HEAD") == commit,
              "runtime source, binary or commit changed during rehearsal")
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or bool(h.git(source, "status", "--porcelain")),
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime, "private_fixture_removed": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("CLI history passed: retained actual results, preserved authority and relocated backup.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("CLI history rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
