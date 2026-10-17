#!/usr/bin/env python3
"""Run committed Repository Checks locally without granting verification or publication."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile


def helper_module():
    path = Path(__file__).with_name("alpha-rehearsal.py")
    spec = importlib.util.spec_from_file_location("alpha_rehearsal", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Rehearsal:
    def __init__(self, helper, binary, root):
        self.h, self.binary, self.root = helper, binary, root
        self.repo = root / "ordinary repository"
        self.backends = set()

    def git(self, repo, *args):
        return self.h.git(repo, *args)

    def cli(self, *args, expected=0):
        try:
            return self.h.command([str(self.binary), *map(str, args)], expected=expected)
        except RuntimeError as error:
            raise RuntimeError(f"{args[0]} {args[1]}: {error}") from None

    def worktrees(self, repo):
        rows = self.git(repo, "worktree", "list", "--porcelain").split("\n\n")
        return [dict(line.split(" ", 1) for line in row.splitlines() if " " in line) for row in rows]

    def common(self, repo):
        return Path(self.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))

    def authority(self, repo):
        common = self.common(repo)
        paths = sorted((common / "gitust-approvals").glob("*.json"))
        paths += sorted((common / "worktrees").glob("*/gitust-origin.json"))
        return {str(path.relative_to(common)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in paths}

    def state(self, repo):
        bare = self.git(repo, "rev-parse", "--is-bare-repository") == "true"
        base = self.common(repo) / "gitust-worktrees" if bare else repo.parent / ("." + repo.name + ".gitust-worktrees")
        checkout_entries = sorted(path.name for path in base.iterdir()) if base.exists() else []
        scratch = self.common(repo) / "gitust-sandbox-scratch"
        scratch_entries = sorted(path.name for path in scratch.iterdir()) if scratch.exists() else []
        return (self.git(repo, "symbolic-ref", "HEAD"),
                self.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)"),
                self.git(repo, "worktree", "list", "--porcelain"),
                None if bare else self.git(repo, "status", "--porcelain"), self.authority(repo),
                checkout_entries, scratch_entries)

    def commit_config(self, content, message):
        path = self.repo / ".gitust-checks.json"
        if content is None:
            path.unlink()
        else:
            path.write_text(content)
        self.git(self.repo, "add", "--all", "--", ".gitust-checks.json")
        self.git(self.repo, "commit", "-m", message)
        return self.git(self.repo, "rev-parse", "HEAD")

    def run(self, repo, expected_commit, *, status, exit_code, commands=None, step_exits=None):
        before = self.state(repo)
        raw = self.cli("sandbox", "checks", "--repo", repo, "--expected-commit", expected_commit,
                       "--timeout", "30", expected=exit_code)
        report = json.loads(raw)
        self.h.require(set(report) == {"version", "status", "reason", "source_ref", "checked_commit",
                                       "checks_blob_oid", "result", "cleanup"}, "unexpected Checks result schema")
        self.h.require(report["version"] == 1 and report["status"] == status,
                       "Checks result did not report its actual status")
        if commands is None:
            self.h.require(report["result"] is None and report["cleanup"] == "not_created"
                           and isinstance(report["reason"], str) and bool(report["reason"]),
                           "rejected Checks request started execution or hid its reason")
        else:
            result = report["result"]
            self.h.require(report["source_ref"] == before[0]
                           and report["checked_commit"] == expected_commit
                           and report["checks_blob_oid"] == self.git(repo, "rev-parse", expected_commit + ":.gitust-checks.json")
                           and report["reason"] is None and report["cleanup"] == "confirmed",
                           "Checks result is not bound to the actual committed snapshot and cleanup")
            self.h.require(result["exit_code"] == exit_code and result["network_isolated"] is True
                           and [step["command"] for step in result["steps"]] == commands
                           and [step["exit_code"] for step in result["steps"]] == step_exits
                           and all(step["required"] is True for step in result["steps"]),
                           "Checks steps do not match the committed commands and actual outcomes")
            self.h.require(isinstance(result["duration_ms"], int) and result["duration_ms"] >= 0,
                           "Checks result has no measured duration")
            for step, code in zip(result["steps"], step_exits):
                self.h.require((code is None and step.get("output") is None and step["duration_ms"] is None)
                               or (code is not None and isinstance(step.get("output"), dict)
                                   and isinstance(step["duration_ms"], int) and step["duration_ms"] >= 0),
                               "skipped or executed step output is inconsistent")
            self.backends.add(result["backend"])
        self.h.require(self.state(repo) == before, "Checks changed original refs, checkout, tasks or approval records")
        self.h.require(not list((self.common(repo) / "gitust-active-executions").glob("*.json")),
                       "completed Checks left an execution fence")
        return report

    def setup(self):
        self.repo.mkdir()
        self.git(self.repo, "init", "--initial-branch=main")
        self.git(self.repo, "config", "user.name", "Gitust CLI Checks Rehearsal")
        self.git(self.repo, "config", "user.email", "cli-checks@company.internal")
        (self.repo / "README.md").write_text("# Committed Checks fixture\n")
        self.good_commands = ["printf 'first-check\\n'; test -f README.md",
                              "test ! -f dirty-only.txt; printf 'second-check\\n'"]
        self.good_config = json.dumps({"version": 1, "commands": self.good_commands, "auto_run": False})
        (self.repo / ".gitust-checks.json").write_text(self.good_config)
        self.git(self.repo, "add", "README.md", ".gitust-checks.json")
        self.git(self.repo, "commit", "-m", "test: committed manual Checks")
        self.good = self.git(self.repo, "rev-parse", "HEAD")
        self.git(self.repo, "tag", "-a", "preserved-tag", "-m", "Preserve this raw annotated object")
        self.tag = self.git(self.repo, "rev-parse", "refs/tags/preserved-tag")
        self.cli("sandbox", "run", "true", "--repo", self.repo, "--keep", "--timeout", "30")
        tasks = [row for row in self.worktrees(self.repo) if row.get("branch", "").startswith("refs/heads/agent/")]
        self.h.require(len(tasks) == 1, "existing managed task fixture missing")
        approved = Path(tasks[0]["worktree"])
        (approved / "approved-fixture.txt").write_text("A real change approved before the Checks run\n")
        self.git(approved, "add", "approved-fixture.txt")
        self.git(approved, "commit", "-m", "test: preexisting approved task")
        self.cli("triage", "approve", tasks[0]["branch"][11:], "--repo", self.repo)
        self.good = self.git(self.repo, "rev-parse", "HEAD")
        self.git(self.repo, "branch", "checks-good", self.good)
        self.cli("sandbox", "run", "true", "--repo", self.repo, "--keep", "--timeout", "30")
        self.before_authority = self.authority(self.repo)
        self.h.require(len(list((self.common(self.repo) / "gitust-approvals").glob("*.json"))) == 1
                       and len([p for p in self.before_authority if p.endswith("gitust-origin.json")]) == 2,
                       "fixture needs a real approval and two original managed targets")

    def execute(self):
        self.setup()
        config = self.repo / ".gitust-checks.json"
        config.write_text("uncommitted invalid configuration")
        dirty = self.repo / "dirty-only.txt"
        dirty.write_text("Must stay in the original working copy\n")
        report = self.run(self.repo, self.good, status="passed", exit_code=0,
                          commands=self.good_commands, step_exits=[0, 0])
        self.h.require("first-check" in report["result"]["steps"][0]["output"]["stdout"]
                       and "second-check" in report["result"]["steps"][1]["output"]["stdout"]
                       and config.read_text() == "uncommitted invalid configuration"
                       and dirty.read_text() == "Must stay in the original working copy\n",
                       "committed Checks did not ignore and preserve the original dirty files")
        self.run(self.repo, self.good[:8], status="rejected", exit_code=2)
        config.write_text(self.good_config)
        dirty.unlink()
        failed_commands = ["printf 'intentional-failure\\n'; exit 7", "printf 'must-not-run\\n'"]
        failed = self.commit_config(json.dumps({"version": 1, "commands": failed_commands}), "test: failed Checks")
        self.run(self.repo, self.good, status="rejected", exit_code=2)
        report = self.run(self.repo, failed, status="failed", exit_code=7,
                          commands=failed_commands, step_exits=[7, None])
        self.h.require("must-not-run" not in report["result"]["stdout"], "a skipped check was executed")
        malformed = self.commit_config("{", "test: malformed Checks")
        self.run(self.repo, malformed, status="rejected", exit_code=2)
        missing = self.commit_config(None, "test: absent Checks")
        self.run(self.repo, missing, status="rejected", exit_code=2)
        bare = self.root / "bare.git"
        self.git(self.root, "clone", "--bare", "--no-local", str(self.repo), str(bare))
        self.git(bare, "symbolic-ref", "HEAD", "refs/heads/checks-good")
        self.run(bare, self.good, status="passed", exit_code=0,
                 commands=self.good_commands, step_exits=[0, 0])
        self.h.require(self.authority(self.repo) == self.before_authority,
                       "Checks changed preexisting approval or worktree-origin records")
        for repo in [self.repo, bare]:
            self.h.require(self.git(repo, "rev-parse", "refs/tags/preserved-tag") == self.tag
                           and self.git(repo, "cat-file", "-t", self.tag) == "tag",
                           "Checks or clone changed the raw annotated tag")
            self.git(repo, "fsck", "--full", "--strict")
        for row in self.worktrees(self.repo):
            self.git(Path(row["worktree"]), "fsck", "--full", "--strict")
        return {"version": 1, "passed": True, "actual_repositories": 2,
                "committed_config_and_sha_match": True, "dirty_working_copy_preserved": True,
                "manual_checks_ignore_auto_run": True, "failed_and_skipped_steps_match": True,
                "invalid_missing_and_stale_requests_not_executed": True,
                "original_refs_and_worktrees_preserved": True, "approval_and_origins_preserved": True,
                "temporary_worktrees_and_fences_removed": True, "bare_repository_supported": True,
                "annotated_tags_preserved": True, "git_fsck": True,
                "platform": os.uname().sysname, "sandbox_backends": sorted(self.backends),
                "external_github_publication": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    h = helper_module()
    source, binary = args.source.resolve(strict=True), args.binary.resolve(strict=True)
    h.require(not args.report.exists() and not args.report.is_symlink(), "report path already exists")
    commit, dirty = h.git(source, "rev-parse", "HEAD"), bool(h.git(source, "status", "--porcelain"))
    paths = [Path(__file__).resolve(), Path(__file__).with_name("alpha-rehearsal.py").resolve()]
    runtime = {"scripts/" + p.name: h.sha256(p) for p in paths}
    for name, digest in runtime.items():
        old = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                             env=h.clean_environment(), capture_output=True, timeout=30)
        dirty |= old.returncode != 0 or hashlib.sha256(old.stdout).hexdigest() != digest
    binary_sha = h.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-cli-checks-") as directory:
        scratch = Path(directory).resolve()
        report = Rehearsal(h, binary, scratch).execute()
    h.require(not scratch.exists(), "private fixture was not removed")
    h.require(runtime == {"scripts/" + p.name: h.sha256(p) for p in paths}
              and h.sha256(binary) == binary_sha and h.git(source, "rev-parse", "HEAD") == commit,
              "runtime source, binary or commit changed during rehearsal")
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or bool(h.git(source, "status", "--porcelain")),
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "private_fixture_removed": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("CLI Checks passed: committed snapshot, actual steps, unchanged authority and normal cleanup.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("CLI Checks rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
