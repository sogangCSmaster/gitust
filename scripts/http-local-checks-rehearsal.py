#!/usr/bin/env python3
"""Run committed default-branch Checks through a disposable active primary service."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import urllib.error
import urllib.request
import uuid


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


multi = load("multi-repository-rehearsal")
web = load("local-checks-web-rehearsal")


class Fixture(multi.Rehearsal):
    trusted = False

    def launch(self, primary, secondary=()):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        args = [str(self.binary), "serve", "--bind", f"127.0.0.1:{port}",
                "--repo", str(primary), "--auth-db", str(self.auth),
                "--sync-github", str(self.root / "unused-upstream.git")]
        for repo in secondary:
            args.extend(["--read-only-repo", str(repo)])
        if self.trusted:
            args.append("--trusted-single-user-sandbox")
        process = subprocess.Popen(args, env=self.helper.clean_environment(),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.processes.append(process)
        return process, f"http://127.0.0.1:{port}"


class Rehearsal(web.Rehearsal):
    def restart(self, *, trusted):
        self.f.stop(self.f.process)
        self.f.trusted = trusted
        self.f.process, self.f.url = self.f.launch(self.f.repos[0], [self.f.repos[1]])
        self.f.healthy(self.f.process, self.f.url)
        self.f.admin.url = self.f.url
        for client, _ in self.f.members:
            client.url = self.f.url

    def post(self, client, repo_id, payload, *, expected=200, expected_code=None):
        headers = {"Content-Type": "application/json"}
        if client.token:
            headers["Authorization"] = "Bearer " + client.token
        request = urllib.request.Request(
            client.url + f"/api/v1/repos/{repo_id}/local-check-runs",
            json.dumps(payload).encode(), headers, method="POST")
        try:
            response = client.opener.open(request, timeout=120)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            status, cache = response.status, response.headers.get("Cache-Control")
            body = response.read(3 * 1024 * 1024 + 1)
        self.h.require(status == expected, f"local Checks POST {status}, expected {expected}")
        self.h.require(cache == "no-store" and len(body) <= 3 * 1024 * 1024,
                       "execution response is cacheable or exceeds its bounded contract")
        if expected != 200:
            self.h.require(str(self.f.root).encode() not in body,
                           "execution rejection exposed a private host path")
            self.h.require(expected_code is not None and json.loads(body) == {"code": expected_code},
                           "execution rejection differs from its fixed error contract")
            return None
        report = json.loads(body)
        self.h.require(set(report) == {"version", "repo_id", "status", "reason", "cleanup",
                                        "history", "record"}
                       and report["version"] == 1 and report["repo_id"] == repo_id,
                       "execution response has an unexpected identity or schema")
        return report

    def reject(self, client, repo_id, payload, *, expected, code):
        before, history = self.state(), self.read(self.f.admin, self.f.ids[0])
        self.post(client, repo_id, payload, expected=expected, expected_code=code)
        self.h.require(self.state() == before
                       and self.read(self.f.admin, self.f.ids[0]) == history,
                       "rejected execution changed original Git or completed history")
        for repo in self.f.repos:
            self.h.require(not list((self.cli.common(repo) / "gitust-active-executions").glob("*.json")),
                           "rejected execution created an execution fence")

    def target(self):
        return {"expected_source_ref": self.cli.git(self.cli.repo, "symbolic-ref", "HEAD"),
                "expected_commit": self.cli.git(self.cli.repo, "rev-parse", "HEAD"),
                "timeout_secs": 30}

    def state(self):
        return [self.cli.state(repo) for repo in self.f.repos]

    def run(self, client, commands, codes):
        before, target = self.state(), self.target()
        report = self.post(client, self.f.ids[0], target)
        row = report["record"]
        self.h.require(report["status"] == ("passed" if codes[0] == 0 else "failed")
                       and report["reason"] is None and report["cleanup"] == "confirmed"
                       and report["history"] == {"status": "saved", "reason": None},
                       "execution or durable save was not confirmed")
        self.h.require(row["source_ref"] == target["expected_source_ref"]
                       and row["checked_commit"] == target["expected_commit"]
                       and row["checks_blob_oid"] == self.cli.git(
                           self.cli.repo, "rev-parse", target["expected_commit"] + ":.gitust-checks.json")
                       and row["status"] == report["status"],
                       "completed result is not bound to the committed default snapshot")
        result = row["result"]
        self.h.require(result["exit_code"] == codes[0] and result["network_isolated"] is True
                       and [step["command"] for step in result["steps"]] == commands
                       and [step["exit_code"] for step in result["steps"]] == codes
                       and all(step["required"] is True for step in result["steps"])
                       and isinstance(result["duration_ms"], int) and result["duration_ms"] >= 0,
                       "completed result does not describe actual required commands and outcomes")
        run_id = str(uuid.UUID(result["run_id"]))
        self.h.require(run_id not in self.run_ids, "explicit runs reused a previous run ID")
        self.run_ids.append(run_id)
        self.cli.backends.add(result["backend"])
        self.h.require(self.read(self.f.admin, self.f.ids[0])["records"][0] == row,
                       "existing history GET differs from the newly completed record")
        self.h.require(self.state() == before, "HTTP Checks changed original Git, tasks or approval state")
        self.h.require(not list((self.cli.common(self.cli.repo) / "gitust-active-executions").glob("*.json")),
                       "confirmed HTTP execution retained an execution fence")
        return row

    def seed_existing_tasks(self):
        self.cli.cli("sandbox", "run", "true", "--repo", self.cli.repo, "--keep", "--timeout", "30")
        tasks = [row for row in self.cli.worktrees(self.cli.repo)
                 if row.get("branch", "").startswith("refs/heads/agent/")]
        self.h.require(len(tasks) == 1, "fixture lacks its original managed task")
        task = Path(tasks[0]["worktree"])
        (task / "approved-before-http.txt").write_text("An existing approved task\n")
        self.cli.git(task, "add", "approved-before-http.txt")
        self.cli.git(task, "commit", "-m", "test: existing approved task")
        self.cli.cli("triage", "approve", tasks[0]["branch"][11:], "--repo", self.cli.repo)
        self.cli.cli("sandbox", "run", "true", "--repo", self.cli.repo, "--keep", "--timeout", "30")
        authority = self.cli.authority(self.cli.repo)
        self.h.require(len(list((self.cli.common(self.cli.repo) / "gitust-approvals").glob("*.json"))) == 1
                       and len([name for name in authority if name.endswith("gitust-origin.json")]) == 2,
                       "fixture must preserve a real approval and both original task identities")

    def execute(self):
        self.run_ids = []
        self.cli.git(self.f.root, "init", "--bare", str(self.f.root / "unused-upstream.git"))
        self.f.setup()
        self.f.stop(self.f.process)
        passed = ["test ! -f dirty-only.txt && test \"$(cat marker.txt)\" = repository-0-after && printf checked-content",
                  "printf checked-second"]
        config = json.dumps({"version": 1, "commands": passed, "auto_run": False})
        self.cli.commit_config(config, "test: committed HTTP Checks")
        self.seed_existing_tasks()
        original_authority = self.cli.authority(self.cli.repo)
        original_target = self.target()
        self.restart(trusted=False)
        before = self.state()
        self.reject(self.f.admin, self.f.ids[0], original_target, expected=403, code="local_checks_disabled")
        self.h.require(self.state() == before and self.read(self.f.admin, self.f.ids[0])["records"] == [],
                       "disabled execution created a run or changed Git")
        disabled_rows = self.f.admin.request("GET", "/api/v1/repos")["repositories"]
        self.h.require(not any(row.get("can_run_local_checks", False) for row in disabled_rows),
                       "disabled execution was advertised as available")
        self.restart(trusted=True)
        rows = self.f.admin.request("GET", "/api/v1/repos")["repositories"]
        by_id = {row["id"]: row for row in rows}
        self.h.require(by_id[self.f.ids[0]].get("can_run_local_checks") is True
                       and by_id[self.f.ids[1]].get("can_run_local_checks", False) is False,
                       "catalog execution hint does not match the trusted primary scope")
        for client, repo_id, status, code in [
                (self.h.Client(self.f.url), self.f.ids[0], 401, "local_checks_unauthorized"),
                (self.f.members[0][0], self.f.ids[0], 403, "local_checks_forbidden"),
                (self.f.admin, self.f.ids[1], 404, "local_checks_not_hosted"),
                (self.f.admin, str(uuid.uuid4()), 404, "local_checks_not_hosted")]:
            self.reject(client, repo_id, original_target, expected=status, code=code)
        self.reject(self.f.admin, self.f.ids[0], {**original_target, "verify": True},
                    expected=400, code="invalid_local_checks_request")
        self.h.require(self.state() == before and self.read(self.f.admin, self.f.ids[0])["records"] == [],
                       "rejected execution changed repository state or retained history")
        (self.cli.repo / ".gitust-checks.json").write_text("uncommitted invalid policy")
        dirty = self.cli.repo / "dirty-only.txt"
        dirty.write_text("Original working data\n")
        first = self.run(self.f.admin, passed, [0, 0])
        self.h.require("checked-content" in first["result"]["stdout"]
                       and dirty.read_text() == "Original working data\n"
                       and (self.cli.repo / ".gitust-checks.json").read_text() == "uncommitted invalid policy",
                       "HTTP execution did not use and preserve the intended committed snapshot")
        pat = self.f.admin.request("POST", "/api/v1/auth/pats", {"name": "local-checks-execution"}, expected=201)
        pat_client = self.h.Client(self.f.url, pat["raw_token"])
        self.run(pat_client, passed, [0, 0])
        self.f.admin.request("DELETE", "/api/v1/auth/pats/" + pat["metadata"]["id"])
        self.reject(pat_client, self.f.ids[0], original_target, expected=401, code="local_checks_unauthorized")
        self.f.stop(self.f.process)
        dirty.unlink()
        failed = ["printf expected-failure; exit 7", "printf must-not-run"]
        self.cli.commit_config(json.dumps({"version": 1, "commands": failed}), "test: failed HTTP Checks")
        self.restart(trusted=True)
        self.reject(self.f.admin, self.f.ids[0], original_target, expected=409, code="local_checks_target_changed")
        failure = self.run(self.f.admin, failed, [7, None])
        self.h.require("must-not-run" not in failure["result"]["stdout"]
                       and failure["result"]["steps"][1].get("output") is None,
                       "a skipped command was executed or acquired invented output")
        expected = self.read(self.f.admin, self.f.ids[0])
        self.h.require([row["result"]["run_id"] for row in expected["records"]]
                       == list(reversed(self.run_ids)),
                       "history contains an unexpected run or omits an explicitly completed run")
        self.restart(trusted=False)
        self.h.require(self.read(self.f.admin, self.f.ids[0]) == expected,
                       "restart or disabling execution changed the completed diagnostic history")
        self.reject(self.f.admin, self.f.ids[0], self.target(), expected=403, code="local_checks_disabled")
        self.h.require(self.cli.authority(self.cli.repo) == original_authority,
                       "HTTP Checks changed managed task or approval authority")
        self.f.stop(self.f.process)
        query = self.history.query(self.cli.repo)
        self.h.require(query["repo_id"] == expected["repo_id"] and query["records"] == expected["records"],
                       "stopped CLI history does not match actual HTTP-created results")
        for repo in self.f.repos:
            self.cli.git(repo, "fsck", "--full", "--strict")
        return {"version": 1, "passed": True, "actual_repositories": 2,
                "active_primary_executes_committed_checks": True, "exact_snapshot_and_policy_binding": True,
                "success_failure_and_skipped_steps": True, "saved_http_get_and_cli_results_match": True,
                "dirty_working_copy_preserved": True, "session_and_pat_execution": True,
                "disabled_member_and_revoked_credentials_denied": True,
                "secondary_unknown_stale_and_unknown_fields_denied": True,
                "capability_matches_execution_scope": True, "bounded_no_store_responses": True,
                "restart_and_disabled_reads_preserve_history": True,
                "original_git_tasks_and_approval_preserved": True, "confirmed_cleanup": True,
                "git_fsck": True, "platform": os.uname().sysname,
                "sandbox_backends": sorted(self.cli.backends), "external_github_publication": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    h = load("alpha-rehearsal")
    source, binary = args.source.resolve(strict=True), args.binary.resolve(strict=True)
    h.require(not args.report.exists() and not args.report.is_symlink(), "report already exists")
    commit, dirty = h.git(source, "rev-parse", "HEAD"), bool(h.git(source, "status", "--porcelain"))
    names = [Path(__file__).name, "alpha-rehearsal.py", "multi-repository-rehearsal.py",
             "local-checks-web-rehearsal.py", "cli-checks-rehearsal.py", "cli-checks-history-rehearsal.py"]
    paths = [Path(__file__).with_name(name).resolve() for name in names]
    runtime = {"scripts/" + path.name: h.sha256(path) for path in paths}
    for name, digest in runtime.items():
        old = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                             env=h.clean_environment(), capture_output=True, timeout=30)
        dirty |= old.returncode != 0 or hashlib.sha256(old.stdout).hexdigest() != digest
    binary_sha = h.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-http-local-checks-") as directory:
        scratch = Path(directory).resolve()
        fixture = Fixture(h, binary, scratch)
        try:
            report = Rehearsal(h, fixture).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            h.require(all(process.poll() is not None for process in fixture.processes),
                      "owned HTTP Checks service was not reaped")
    h.require(not scratch.exists(), "private fixture was not removed")
    h.require(runtime == {"scripts/" + path.name: h.sha256(path) for path in paths}
              and h.sha256(binary) == binary_sha and h.git(source, "rev-parse", "HEAD") == commit,
              "runtime source, binary or commit changed during rehearsal")
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or bool(h.git(source, "status", "--porcelain")),
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "private_fixture_removed": True, "owned_services_reaped": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("HTTP Local Checks passed: active primary execution, exact saved results and preserved Git state.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("HTTP Local Checks rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
