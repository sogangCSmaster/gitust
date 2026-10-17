#!/usr/bin/env python3
"""Ten real local Gitust tasks; no external remote or existing service is touched."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import secrets
import shlex
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def clean_environment(overrides=None):
    result = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    result.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
                   "GIT_TERMINAL_PROMPT": "0"})
    result.update(overrides or {})
    return result


def command(arguments, *, cwd=None, env=None, expected=0):
    result = subprocess.run(arguments, cwd=cwd, env=clean_environment(env),
                            capture_output=True, timeout=300)
    # Do not print command lines or captured output: auth commands can contain secrets.
    require(result.returncode == expected,
            f"{Path(arguments[0]).name} returned {result.returncode}, expected {expected}")
    return result.stdout.decode().strip()


def git(repo, *args, env=None):
    return command(["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
                    "-C", str(repo), *args], env=env)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class Client:
    def __init__(self, url, token=None):
        self.url, self.token, self.sequence = url, token, 0
        # These requests must never pass through a user-configured outbound proxy.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method, route, payload=None, expected=200):
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        data = None if payload is None else json.dumps(payload).encode()
        request = urllib.request.Request(self.url + route, data, headers, method=method)
        try:
            with self.opener.open(request, timeout=120) as response:
                status, body = response.status, response.read(4 * 1024 * 1024)
        except urllib.error.HTTPError as error:
            status, body = error.code, error.read(4 * 1024 * 1024)
        require(status == expected, f"{method} {route}: HTTP {status}, expected {expected}")
        if not body:
            return None
        try:
            return json.loads(body)
        except ValueError:
            return None

    def rpc(self, method, params):
        self.sequence += 1
        body = self.request("POST", "/mcp", {
            "jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params})
        require("error" not in body, f"MCP {method}: JSON-RPC error")
        return body["result"]

    def tool(self, name, arguments, *, error=False):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        payload = json.loads(result["content"][0]["text"])
        require(result["isError"] == error,
                f"MCP {name}: unexpected result (HTTP {payload.get('http_status', 'ok')}, "
                f"exit {payload.get('exit_code')})")
        return payload


class Rehearsal:
    def __init__(self, source, binary, scratch):
        self.source, self.binary, self.scratch = source, binary, scratch
        self.repo, self.upstream = scratch / "server.git", scratch / "upstream.git"
        self.auth = scratch / "auth/auth-db.json"
        self.process = self.log = None
        self.password = secrets.token_urlsafe(24)
        self.started_at = datetime.now(timezone.utc).isoformat()
        self.tasks, self.results, self.backends, self.edit_windows = {}, [], set(), {}
        self.mcp_commits = []

    def record(self, number, scenario, **evidence):
        self.results.append({"task": number, "scenario": scenario, "passed": True, **evidence})
        print(f"PASS {number:02d}/10 {scenario}", flush=True)

    def start(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        self.url = f"http://127.0.0.1:{port}"
        self.log = (self.scratch / f"server-{port}.log").open("wb")
        self.process = subprocess.Popen([
            str(self.binary), "serve", "--bind", f"127.0.0.1:{port}", "--repo", str(self.repo),
            "--auth-db", str(self.auth), "--sync-github", str(self.upstream),
            "--trusted-single-user-sandbox"], stdout=self.log, stderr=self.log,
            env=clean_environment())
        anonymous = Client(self.url)
        for _ in range(100):
            require(self.process.poll() is None, "fixture server stopped during startup")
            try:
                anonymous.request("GET", "/api/v1/health")
                return
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                time.sleep(0.1)
        raise RuntimeError("fixture server did not become healthy")

    def stop(self):
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
            self.process = None
        if self.log is not None:
            self.log.close()
            self.log = None

    def login(self):
        response = Client(self.url).request("POST", "/api/v1/auth/login", {
            "identifier": "alpha-operator", "password": self.password})
        self.operator = Client(self.url, response["raw_session_token"])
        self.operator_id = response["user"]["id"]
        repository_id = self.operator.request("GET", "/api/v1/repo")["id"]
        if hasattr(self, "repository_id"):
            require(repository_id == self.repository_id,
                    "configured repository ID changed after restart or relocation")
        self.repository_id = repository_id

    def setup(self):
        seed = self.scratch / "seed"
        git(self.scratch, "clone", "--no-local", "--single-branch", "--branch", "main",
            "--no-tags", str(self.source), str(seed))
        self.source_commit = git(seed, "rev-parse", "HEAD")
        git(seed, "config", "user.name", "Gitust Alpha Rehearsal")
        git(seed, "config", "user.email", "alpha@company.internal")
        (seed / "docs/alpha-rehearsal").mkdir()
        (seed / "docs/alpha-rehearsal/check-state").write_text("ready\n")
        (seed / ".gitust-checks.json").write_text(json.dumps({"version": 1, "commands": [
            'test -f README.md && test -f Cargo.toml && '
            'test "$(cat docs/alpha-rehearsal/check-state)" = ready', "git diff --check"]}))
        git(seed, "add", ".gitust-checks.json", "docs/alpha-rehearsal/check-state")
        git(seed, "commit", "-m", "test: prepare local Gitust alpha acceptance fixture")
        git(self.scratch, "clone", "--bare", "--no-local", str(seed), str(self.upstream))
        command([str(self.binary), "sync", str(self.upstream), "--repo", str(self.repo)])
        require(git(self.repo, "rev-parse", "main") == git(self.upstream, "rev-parse", "main"),
                "initial mirror SHA differs from upstream")
        command([str(self.binary), "admin", "create", "--username", "alpha-operator",
                 "--email", "alpha@company.internal", "--auth-db", str(self.auth)],
                env={"GITUST_ADMIN_PASSWORD": self.password})
        self.start()
        self.login()
        self.clients = []
        for name in ["alpha-client-a", "alpha-client-b"]:
            issued = self.operator.request("POST", "/api/v1/auth/pats", {"name": name}, expected=201)
            client = Client(self.url, issued["raw_token"])
            client.rpc("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                                      "clientInfo": {"name": name, "version": "1"}})
            client.rpc("tools/list", {})
            self.clients.append(client)

    def spawn(self, number, client=None, suffix=""):
        client = client or self.clients[number % 2]
        # Parallel materialization has an explicit 409 busy response; retry the same intent.
        for _ in range(100):
            result = client.rpc("tools/call", {"name": "gitust_worktree_spawn", "arguments": {
                "branch": f"agent/alpha/task-{number:02d}{suffix}", "base_branch": "main",
                "agent": "codex", "prompt_summary": f"Local alpha acceptance task {number}"}})
            payload = json.loads(result["content"][0]["text"])
            if not result["isError"]:
                self.tasks[number] = payload
                return payload
            require(payload.get("http_status") == 409, "MCP spawn failed without busy response")
            time.sleep(0.05)
        raise RuntimeError("MCP spawn remained busy")

    def run(self, number, script="true", *, verify=False, error=False):
        result = self.clients[number % 2].tool("gitust_sandbox_exec", {
            "worktree_id": self.tasks[number]["worktree_id"], "command": script,
            "timeout_secs": 60, "verify": verify}, error=error)
        if "backend" in result:
            require(result["network_isolated"], "sandbox did not report network isolation")
            self.backends.add(result["backend"])
        return result

    def edit(self, number, suffix="", broken=False):
        started = time.monotonic()
        target = f"docs/alpha-rehearsal/task-{number:02d}.md"
        text = f"# Gitust alpha task {number}\n\nSource checkout operational rehearsal {suffix}.\n"
        self.run(number, "sleep 0.2; printf '%s' " + shlex.quote(text) + " > " + target
                 + ("; printf 'broken\\n' > docs/alpha-rehearsal/check-state" if broken else ""))
        paths = [target]
        if broken:
            paths.append("docs/alpha-rehearsal/check-state")
        self.commit(number, f"test: complete alpha task {number}", paths)
        self.edit_windows[number] = (started, time.monotonic())
        return git(self.repo, "rev-parse", self.tasks[number]["branch"])

    def checkout(self, number):
        branch = "branch refs/heads/" + self.tasks[number]["branch"]
        for entry in git(self.repo, "worktree", "list", "--porcelain").split("\n\n"):
            fields = entry.splitlines()
            if branch in fields:
                path = Path(fields[0].removeprefix("worktree "))
                require(path.parent == self.repo / "gitust-worktrees", "unexpected fixture checkout")
                return path
        raise RuntimeError("managed fixture checkout not registered with Git")

    def commit(self, number, message, paths):
        task = self.tasks[number]
        expected = git(self.repo, "rev-parse", task["branch"])
        arguments = {"worktree_id": task["worktree_id"], "expected_head": expected,
                     "paths": paths, "message": message}
        for _ in range(100):
            result = self.clients[number % 2].rpc("tools/call", {
                "name": "gitust_worktree_commit", "arguments": arguments})
            payload = json.loads(result["content"][0]["text"])
            if not result["isError"]:
                break
            require(payload.get("http_status") == 409 and
                    payload.get("error") == "repository is busy",
                    "MCP commit failed without an explicit repository busy response")
            time.sleep(0.05)
        else:
            raise RuntimeError("MCP commit remained busy")
        require(payload["worktree_id"] == task["worktree_id"] and
                payload["branch"] == task["branch"] and payload["parent"] == expected,
                "MCP commit identity or expected parent differs")
        require(payload["status"] == "pending" and payload["verification_required"],
                "MCP commit incorrectly granted verification authority")
        require(payload["commit"] == git(self.repo, "rev-parse", task["branch"]) and
                git(self.repo, "rev-parse", payload["commit"] + "^") == expected,
                "MCP commit differs from independent Git oracle")
        changed = git(self.repo, "diff-tree", "--no-commit-id", "--name-only", "-r",
                      expected, payload["commit"]).splitlines()
        require(sorted(changed) == sorted(paths), "MCP commit changed unexpected paths")
        rows = self.operator.request("GET", "/api/v1/swarm/worktrees")
        row = next(row for row in rows if row["id"] == task["worktree_id"])
        require(row["status"] == "pending" and row["verified_commit"] is None,
                "MCP commit retained stale server verification authority")
        self.mcp_commits.append({"task": number, "worktree_id": task["worktree_id"],
                                 "branch": task["branch"], "parent": expected,
                                 "commit": payload["commit"], "paths": paths})
        return payload["commit"]

    def verify(self, number, *, error=False):
        outcome = self.run(number, "test -f README.md && printf 'requested-check\\n'",
                           verify=True, error=error)
        require(outcome.get("exit_code", -1) != 0 if error else outcome.get("exit_code") == 0,
                "unexpected verification exit code")
        if not error:
            require(len(outcome["steps"]) == 3 and
                    all(step["exit_code"] == 0 for step in outcome["steps"]),
                    "required repository Checks were not all executed")
            rows = self.operator.request("GET", "/api/v1/swarm/worktrees")
            row = next(row for row in rows if row["id"] == self.tasks[number]["worktree_id"])
            require(row["verified_commit"] == git(self.repo, "rev-parse", row["branch"]),
                    "Verified SHA differs from independent Git oracle")
            require(outcome.get("checked_commit") == row["verified_commit"],
                    "verification response differs from the observed Verified SHA")
        return outcome

    def action(self, number, action, expected=200, *, expected_commit=None):
        payload = {}
        if action == "approve":
            require(expected_commit is not None, "approval requires an observed commit SHA")
            payload["expected_commit"] = expected_commit
        return self.operator.request("POST", "/api/v1/swarm/worktrees/"
                                     + self.tasks[number]["worktree_id"] + "/" + action,
                                     payload, expected=expected)

    def merge_publish(self, number):
        expected = git(self.repo, "rev-parse", self.tasks[number]["branch"])
        rows = self.operator.request("GET", "/api/v1/swarm/worktrees")
        observed = next(row for row in rows if row["id"] == self.tasks[number]["worktree_id"])
        require(observed["verified_commit"] == expected, "approval target is not the observed SHA")
        row = self.action(number, "approve", expected_commit=observed["verified_commit"])
        require(row["status"] == "merged" and git(self.repo, "rev-parse", "main") == expected,
                "approval did not fast-forward the independently observed SHA")
        require(git(self.upstream, "rev-parse", "main") != expected,
                "local approval published without an explicit request")
        self.action(number, "publish")
        require(git(self.upstream, "rev-parse", "main") == expected, "published SHA differs")
        require(not git(self.upstream, "for-each-ref", "--format=%(refname)", "refs/heads/agent"),
                "private agent branch refs reached upstream")
        return expected

    def history(self, number):
        return self.operator.request("GET", "/api/v1/swarm/worktrees/"
                                     + self.tasks[number]["worktree_id"] + "/runs")

    def execute(self):
        self.setup()
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(self.spawn, [1, 2]))
            started = time.monotonic()
            commits = list(pool.map(self.edit, [1, 2]))
            parallel_ms = round((time.monotonic() - started) * 1000)
        require(commits[0] != commits[1], "two clients shared a branch or commit")
        require(max(self.edit_windows[n][0] for n in [1, 2]) <
                min(self.edit_windows[n][1] for n in [1, 2]), "client tasks did not overlap")
        for number, other in [(1, 2), (2, 1)]:
            files = git(self.repo, "ls-tree", "-r", "--name-only", self.tasks[number]["branch"],
                        "docs/alpha-rehearsal").splitlines()
            require(f"docs/alpha-rehearsal/task-{number:02d}.md" in files and
                    f"docs/alpha-rehearsal/task-{other:02d}.md" not in files,
                    "concurrent source edits were not isolated")
        self.run(2, "printf 'broken\\n' > docs/alpha-rehearsal/check-state")
        failed_commit = self.commit(2, "test: reproduce a normal failing repository Check",
                                    ["docs/alpha-rehearsal/check-state"])
        failure = self.verify(2, error=True)
        self.action(2, "approve", 409, expected_commit=failed_commit)
        self.run(2, "printf 'ready\\n' > docs/alpha-rehearsal/check-state")
        self.commit(2, "test: repair Check input", ["docs/alpha-rehearsal/check-state"])
        self.verify(2)
        prior_branch = self.tasks[2]["branch"]
        prior_commit = git(self.repo, "rev-parse", prior_branch)
        self.clients[0].tool("gitust_worktree_discard", {
            "worktree_id": self.tasks[2]["worktree_id"]})
        require(git(self.repo, "rev-parse", prior_branch) == prior_commit,
                "fresh-base retry destroyed committed private work")
        self.verify(1)
        self.record(1, "two HTTP MCP clients, isolated concurrent source edits and MCP commits",
                    commit=self.merge_publish(1), concurrent_edit_wall_ms=parallel_ms,
                    concurrent_edit_observed=True)
        self.spawn(2, suffix="-retry")
        self.edit(2, "explicit new-main worktree retry after preserving the original branch")
        self.verify(2)
        self.record(2, "required Check failure, MCP repair and fresh-main worktree retry",
                    failed_check_exit=failure["exit_code"], commit=self.merge_publish(2),
                    preserved_initial_branch=prior_branch, preserved_initial_commit=prior_commit,
                    retry_branch=self.tasks[2]["branch"])

        self.spawn(3)
        unverified_commit = self.edit(3)
        self.action(3, "approve", 409, expected_commit=unverified_commit)
        self.verify(3)
        self.record(3, "approval waits for exact commit verification", commit=self.merge_publish(3))

        self.spawn(4)
        self.edit(4)
        previous = self.verify(4)["checked_commit"]
        self.edit(4, "second commit after verification")
        self.action(4, "approve", 409, expected_commit=previous)
        current = self.verify(4)["checked_commit"]
        main_before = git(self.repo, "rev-parse", "main")
        self.action(4, "approve", 409, expected_commit=previous)
        rows = self.operator.request("GET", "/api/v1/swarm/worktrees")
        row = next(row for row in rows if row["id"] == self.tasks[4]["worktree_id"])
        require(row["status"] == "verified" and row["verified_commit"] == current,
                "stale human approval destroyed the newer valid verification")
        require(git(self.repo, "rev-parse", "main") == main_before,
                "stale human approval changed main")
        self.record(4, "stale approval is rejected before and after newer verification",
                    reviewed_previous_commit=previous, newly_verified_commit=current,
                    commit=self.merge_publish(4))

        self.spawn(5)
        self.edit(5)
        self.run(5, "printf 'uncommitted\\n' >> docs/alpha-rehearsal/task-05.md")
        rejected = self.run(5, verify=True, error=True)
        require(rejected.get("http_status") == 409, "dirty checkout was not rejected")
        self.edit(5, "committed replacement")
        self.verify(5)
        self.record(5, "commit pending edits before verification", commit=self.merge_publish(5))

        self.spawn(6)
        failed_commit = self.edit(6)
        self.run(6, "printf 'normal command failure\\n' >&2; exit 7", error=True)
        self.action(6, "approve", 409, expected_commit=failed_commit)
        self.verify(6)
        self.record(6, "failed command and successful rerun", commit=self.merge_publish(6),
                    saved_runs=len(self.history(6)))

        self.spawn(7)
        self.edit(7)
        checked = self.verify(7)["checked_commit"]
        expected = git(self.repo, "rev-parse", self.tasks[7]["branch"])
        self.action(7, "approve", expected_commit=checked)
        offline = self.scratch / "upstream-offline.git"
        self.upstream.rename(offline)
        self.upstream.mkdir()
        try:
            self.action(7, "publish", 502)
        finally:
            self.upstream.rmdir()
            offline.rename(self.upstream)
        require(git(self.upstream, "rev-parse", "main") != expected,
                "failed publication changed upstream")
        self.action(7, "publish")
        require(git(self.upstream, "rev-parse", "main") == expected, "publication retry failed")
        self.record(7, "unavailable local upstream and explicit publication retry", commit=expected)

        self.spawn(8)
        discarded_commit = self.edit(8)
        previous = git(self.repo, "rev-parse", "main")
        discarded_checkout = self.checkout(8)
        self.clients[0].tool("gitust_worktree_discard", {
            "worktree_id": self.tasks[8]["worktree_id"]})
        require(git(self.repo, "rev-parse", "main") == previous, "discard changed main")
        require(not discarded_checkout.exists(), "discard left its checkout")
        require(git(self.repo, "rev-parse", self.tasks[8]["branch"]) == discarded_commit,
                "discard destroyed committed private work")
        rows = self.operator.request("GET", "/api/v1/swarm/worktrees")
        require(all(row["id"] != self.tasks[8]["worktree_id"] for row in rows),
                "discard left its active worktree entry")
        self.record(8, "discard checkout while preserving committed private work",
                    discarded_commit=discarded_commit)

        self.backup_restore_task()
        self.http_git_task()
        for repo in [self.repo, self.upstream, self.scratch / "http-client"]:
            git(repo, "fsck", "--full", "--strict")
        return {"version": 2, "started_at": self.started_at,
                "completed_at": datetime.now(timezone.utc).isoformat(),
                "source_commit": self.source_commit, "binary_sha256": sha256(self.binary),
                "operator_script_sha256": {name: sha256(self.source / "scripts" / name)
                                           for name in ["alpha-rehearsal.py", "backup.py"]},
                "runtime_worktree_dirty": bool(git(self.source, "status", "--porcelain")),
                "platform": platform.platform(), "sandbox_backends": sorted(self.backends),
                "fixture": "committed Gitust main cloned locally; task commits only in scratch",
                "mcp_clients": 2, "mcp_only_commit_demonstrated": True,
                "repository_identity_preserved_after_restart_and_restore": True,
                "tasks_1_to_9_commit_actor": "gitust_worktree_commit via HTTP MCP",
                "mcp_commits": self.mcp_commits,
                "task_10_commit_actor": "separate Git Smart HTTP client clone",
                "external_github_publication": False, "tasks": self.results,
                "git_fsck": "server, local upstream and HTTP client passed --full --strict"}

    def backup_restore_task(self):
        self.spawn(9)
        self.edit(9)
        checked = self.verify(9)["checked_commit"]
        before = self.history(9)
        owner = (self.repo / "gitust-owners" / (self.tasks[9]["worktree_id"] + ".json")).read_bytes()
        require(json.loads(owner)["user_id"] == self.operator_id, "source task owner is incorrect")
        approvals = {p.name: p.read_bytes() for p in (self.repo / "gitust-approvals").glob("*.json")}
        main = git(self.repo, "rev-parse", "main")
        self.stop()
        self.start()
        self.login()
        for client in self.clients:
            client.url = self.url
        self.stop()
        backup, restored = self.scratch / "backup", self.scratch / "restored"
        tool = self.source / "scripts/backup.py"
        command([sys.executable, str(tool), "create", "--repo", str(self.repo),
                 "--auth-db", str(self.auth), "--output", str(backup)])
        self.repo.rename(self.scratch / "original-unavailable.git")
        command([sys.executable, str(tool), "restore", "--backup", str(backup),
                 "--destination", str(restored)])
        self.repo, self.auth = restored / "repository", restored / "auth/auth-db.json"
        require((self.repo / "gitust-owners" / (self.tasks[9]["worktree_id"] + ".json"))
                .read_bytes() == owner, "restored owner changed")
        require({p.name: p.read_bytes() for p in (self.repo / "gitust-approvals").glob("*.json")}
                == approvals, "restored human approval/publication receipts changed")
        require(git(self.repo, "rev-parse", "main") == main, "restored main SHA differs")
        self.start()
        self.login()
        require(json.loads(owner)["user_id"] == self.operator_id, "restored operator identity differs")
        for client in self.clients:
            client.url = self.url
        require(self.history(9) == before, "restored completed run history differs")
        rows = self.operator.request("GET", "/api/v1/swarm/worktrees")
        row = next(row for row in rows if row["id"] == self.tasks[9]["worktree_id"])
        require(row["status"] == "pending" and row["verified_commit"] is None,
                "restart restored stale verification authority")
        require(sum(row["status"] == "merged" for row in rows) == 7,
                "restart failed to restore durable merged approvals")
        self.action(9, "approve", 409, expected_commit=checked)
        self.edit(9, "MCP commit after relocated restore using the original PAT")
        self.verify(9)
        self.record(9, "offline backup, relocation, original-PAT MCP commit and re-verification",
                    commit=self.merge_publish(9), retained_runs=len(before), restored_approvals=7,
                    repository_identity_preserved_after_restart_and_restore=True)

    def http_git_task(self):
        self.spawn(10)
        clone = self.scratch / "http-client"
        auth_env = {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.extraHeader",
                    "GIT_CONFIG_VALUE_0": "Authorization: Bearer " + self.clients[0].token,
                    "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"}
        git(self.scratch, "clone", self.url + "/git/default.git", str(clone), env=auth_env)
        git(clone, "checkout", self.tasks[10]["branch"])
        path = clone / "docs/alpha-rehearsal/task-10.md"
        path.write_text("# Gitust alpha task 10\n\nReal Smart HTTP client source change.\n")
        git(clone, "add", str(path.relative_to(clone)))
        git(clone, "-c", "user.name=Gitust-Alpha", "-c", "user.email=alpha@company.internal",
            "commit", "-m", "test: update source through real Git Smart HTTP")
        expected = git(clone, "rev-parse", "HEAD")
        git(clone, "push", "origin", "HEAD:" + self.tasks[10]["branch"], env=auth_env)
        require(git(self.repo, "rev-parse", self.tasks[10]["branch"]) == expected,
                "HTTP receive-pack did not update managed agent branch")
        self.verify(10)
        require(self.merge_publish(10) == expected, "HTTP-pushed SHA was not approved exactly")
        git(clone, "fetch", "origin", env=auth_env)
        require(git(clone, "rev-parse", "origin/main") == expected,
                "HTTP fetch did not return published main")
        self.record(10, "real Git HTTP clone, commit, managed push, approve, publish and fetch",
                    commit=expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True,
                        help="new JSON evidence file; contains no credentials or captured output")
    args = parser.parse_args()
    source = Path(__file__).resolve().parent.parent
    binary = args.binary.resolve(strict=True)
    require(not args.report.exists(), "report already exists; choose a new path")
    with tempfile.TemporaryDirectory(prefix="gitust-alpha-") as directory:
        run = Rehearsal(source, binary, Path(directory).resolve())
        try:
            report = run.execute()
        finally:
            run.stop()
    descriptor = os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("All 10 local tasks passed; private scratch state removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"Rehearsal failed: {error}", file=sys.stderr)
        sys.exit(1)
