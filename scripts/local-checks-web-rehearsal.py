#!/usr/bin/env python3
"""Read actual CLI Checks history through a service with HTTP execution disabled."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
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


class Rehearsal:
    def __init__(self, helper, fixture):
        self.h, self.f = helper, fixture
        local = load("cli-checks-rehearsal")
        history = load("cli-checks-history-rehearsal")
        self.cli = local.Rehearsal(helper, fixture.binary, fixture.root)
        self.cli.repo = fixture.repos[0]
        self.history = history.Rehearsal(self.cli, fixture.root)

    def read(self, client, repo_id, *, expected=200):
        headers = {"Authorization": "Bearer " + client.token} if client.token else {}
        request = urllib.request.Request(client.url + f"/api/v1/repos/{repo_id}/local-check-runs",
                                         headers=headers)
        try:
            response = client.opener.open(request, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            status, headers = response.status, response.headers
            body = response.read(3 * 1024 * 1024 + 1)
        self.h.require(status == expected, f"local history HTTP {status}, expected {expected}")
        self.h.require(headers.get("Cache-Control") == "no-store" and len(body) <= 3 * 1024 * 1024,
                       "local history response is cacheable or exceeds its bounded contract")
        if expected != 200:
            self.h.require(str(self.f.root).encode() not in body, "history error exposed a host path")
            return None
        value = json.loads(body)
        self.h.require(set(value) == {"version", "repo_id", "records"} and value["version"] == 1
                       and value["repo_id"] == repo_id and isinstance(value["records"], list)
                       and len(value["records"]) <= 5, "unexpected local history response envelope")
        return value

    def restart(self):
        self.f.stop(self.f.process)
        self.f.process, self.f.url = self.f.launch(self.f.repos[0], [self.f.repos[1]])
        self.f.healthy(self.f.process, self.f.url)
        self.f.admin.url = self.f.url
        for client, _ in self.f.members:
            client.url = self.f.url
        self.h.require("--trusted-single-user-sandbox" not in self.f.process.args,
                       "history read fixture unexpectedly enabled HTTP execution")

    def execute(self):
        self.f.setup()
        self.h.require(self.read(self.f.admin, self.f.ids[0])["records"] == [],
                       "new repository history is not empty")
        self.f.stop(self.f.process)
        branch = "release/local-checks-observability-with-a-deliberately-long-branch-name"
        self.cli.git(self.cli.repo, "switch", "-c", branch)
        passed = ["awk 'BEGIN { for (i=0; i<10000; i++) printf \"한\" }'", "printf completed"]
        self.cli.commit_config(json.dumps({"version": 1, "commands": passed}), "test: recorded success")
        success, saved_success = self.history.record(self.cli.repo)
        failed = ["printf expected-failure; exit 7", "printf skipped"]
        self.cli.commit_config(json.dumps({"version": 1, "commands": failed}), "test: recorded failure")
        failure, _saved_failure = self.history.record(self.cli.repo, exit_code=7)
        self.h.require(saved_success["history_truncated"] is True
                       and len(success["result"]["stdout"].encode()) > 16384,
                       "actual fixture does not distinguish stored and live output")
        query = self.history.query(self.cli.repo)
        self.expected = {"version": 1, "repo_id": query["repo_id"], "records": query["records"]}
        self.h.require(query["repo_id"] == self.f.ids[0]
                       and [r["result"]["run_id"] for r in query["records"]]
                       == [failure["result"]["run_id"], success["result"]["run_id"]],
                       "fixture recorded IDs or append order differ")
        states = [self.cli.state(repo) for repo in self.f.repos]
        checkpoint = self.cli.common(self.cli.repo) / "gitust-local-check-runs/history.json"
        history_sha = self.h.sha256(checkpoint)
        self.restart()
        self.h.require(self.read(self.f.admin, self.f.ids[0]) == self.expected,
                       "HTTP history differs from the actual CLI checkpoint")
        self.read(self.h.Client(self.f.url), self.f.ids[0], expected=401)
        self.read(self.f.members[0][0], self.f.ids[0], expected=403)
        self.read(self.f.admin, self.f.ids[1], expected=404)
        self.read(self.f.admin, str(uuid.uuid4()), expected=404)
        pat = self.f.admin.request("POST", "/api/v1/auth/pats", {"name": "local-history-read"}, expected=201)
        pat_client = self.h.Client(self.f.url, pat["raw_token"])
        self.h.require(self.read(pat_client, self.f.ids[0]) == self.expected,
                       "authorized PAT did not read the same completed history")
        self.f.admin.request("DELETE", "/api/v1/auth/pats/" + pat["metadata"]["id"])
        self.read(pat_client, self.f.ids[0], expected=401)
        self.restart()
        self.h.require(self.read(self.f.admin, self.f.ids[0]) == self.expected,
                       "service restart changed retained local history")
        self.h.require(states == [self.cli.state(repo) for repo in self.f.repos]
                       and self.h.sha256(checkpoint) == history_sha,
                       "read-only history requests changed repository or retained bytes")
        for repo in self.f.repos:
            self.cli.git(repo, "fsck", "--full", "--strict")
        return {"version": 1, "passed": True, "actual_repositories": 2,
                "real_cli_success_failure_and_output": True, "exact_http_cli_history_match": True,
                "empty_history_distinguished": True, "http_execution_disabled": True,
                "authorized_session_and_pat": True, "anonymous_member_and_revoked_pat_denied": True,
                "secondary_and_unknown_not_routed": True, "bounded_no_store_responses": True,
                "restart_preserves_records": True, "git_and_history_unchanged_by_reads": True,
                "git_fsck": True, "platform": os.uname().sysname,
                "sandbox_backends": sorted(self.cli.backends), "external_github_publication": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    h, multi = load("alpha-rehearsal"), load("multi-repository-rehearsal")
    source, binary = args.source.resolve(strict=True), args.binary.resolve(strict=True)
    h.require(not args.report.exists() and not args.report.is_symlink(), "report already exists")
    commit, dirty = h.git(source, "rev-parse", "HEAD"), bool(h.git(source, "status", "--porcelain"))
    names = [Path(__file__).name, "alpha-rehearsal.py", "multi-repository-rehearsal.py",
             "cli-checks-rehearsal.py", "cli-checks-history-rehearsal.py"]
    paths = [Path(__file__).with_name(name).resolve() for name in names]
    runtime = {"scripts/" + p.name: h.sha256(p) for p in paths}
    for name, digest in runtime.items():
        original = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                                  env=h.clean_environment(), capture_output=True, timeout=30)
        dirty |= original.returncode != 0 or hashlib.sha256(original.stdout).hexdigest() != digest
    binary_sha = h.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-local-checks-web-") as directory:
        scratch = Path(directory).resolve()
        fixture = multi.Rehearsal(h, binary, scratch)
        try:
            report = Rehearsal(h, fixture).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            h.require(all(p.poll() is not None for p in fixture.processes), "owned service was not reaped")
    h.require(not scratch.exists(), "private fixture was not removed")
    h.require(runtime == {"scripts/" + p.name: h.sha256(p) for p in paths}
              and h.sha256(binary) == binary_sha and h.git(source, "rev-parse", "HEAD") == commit,
              "runtime source, binary or commit changed during rehearsal")
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or bool(h.git(source, "status", "--porcelain")),
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "private_fixture_removed": True, "owned_services_reaped": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Local Checks web history passed: actual CLI results, bounded read-only HTTP and restart.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Local Checks web rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
