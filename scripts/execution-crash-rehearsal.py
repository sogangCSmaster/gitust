#!/usr/bin/env python3
"""Prove a killed service cannot overlap an old command with a restarted service."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def wait_for(predicate, timeout, message):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.025)
    raise RuntimeError(message)


class CrashRehearsal:
    def __init__(self, helper, binary, root):
        self.helper, self.binary, self.root = helper, binary, root
        self.repo, self.upstream = root / "server.git", root / "upstream.git"
        self.auth = root / "auth/auth-db.json"
        self.nonce = "gitust-crash-" + secrets.token_hex(16)
        self.servers, self.groups = [], {}
        self.password = secrets.token_urlsafe(24)

    def ps(self):
        result = subprocess.run(["ps", "-ww", "-axo", "pid=,ppid=,pgid=,stat=,lstart=,args="],
                                capture_output=True, text=True, timeout=5)
        require(result.returncode == 0, "host process inspection failed")
        rows = {}
        for line in result.stdout.splitlines():
            columns = line.strip().split(None, 9)
            require(len(columns) == 10, "incomplete host process row")
            pid, parent, group = map(int, columns[:3])
            rows[pid] = {"pid": pid, "parent": parent, "group": group,
                         "state": columns[3], "start": " ".join(columns[4:9]),
                         "args": columns[9]}
        require(rows, "empty host process inspection")
        return rows

    def live_owned(self):
        return [row for row in self.ps().values()
                if row["group"] in self.groups and not row["state"].startswith("Z")]

    def launch(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        process = subprocess.Popen([
            str(self.binary), "serve", "--bind", f"127.0.0.1:{port}",
            "--repo", str(self.repo), "--auth-db", str(self.auth),
            "--sync-github", str(self.upstream), "--trusted-single-user-sandbox"],
            env=self.helper.clean_environment({"RUST_LOG": "off"}),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.servers.append(process)
        return process, f"http://127.0.0.1:{port}"

    def healthy(self, process, url):
        def check():
            require(process.poll() is None, "fixture service failed to start")
            try:
                self.helper.Client(url).request("GET", "/api/v1/health")
                return True
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                return False
        wait_for(check, 10, "fixture service startup deadline")

    def stop_owned_groups(self):
        rows = self.ps()
        for group, known in self.groups.items():
            live = [row for row in rows.values()
                    if row["group"] == group and not row["state"].startswith("Z")]
            authenticated = any(row["pid"] in known and all(
                row[key] == known[row["pid"]][key] for key in ["start", "args"])
                for row in live)
            if live:
                require(authenticated, "refusing to signal a reused or unknown process group")
                require(group > 1 and group != os.getpgrp(), "refusing own process group")
                try:
                    os.killpg(group, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        wait_for(lambda: not self.live_owned(), 5, "owned command did not stop")

    def remember_groups(self, process, rows):
        server = rows.get(process.pid)
        require(server and str(self.binary) in server["args"] and str(self.repo) in server["args"],
                "fixture service identity changed")
        for row in rows.values():
            if self.nonce in row["args"] and not row["state"].startswith("Z"):
                ancestor, seen = row, set()
                while ancestor and ancestor["pid"] not in seen:
                    if ancestor["parent"] == process.pid:
                        self.groups.setdefault(row["group"], {})[row["pid"]] = row
                        break
                    seen.add(ancestor["pid"])
                    ancestor = rows.get(ancestor["parent"])

    def cleanup(self):
        rows = self.ps()
        for process in self.servers:
            if process.poll() is None:
                self.remember_groups(process, rows)
        for process in self.servers:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        self.stop_owned_groups()

    def execute(self):
        git, command = self.helper.git, self.helper.command
        seed = self.root / "seed"
        seed.mkdir()
        git(seed, "init", "-b", "main")
        git(seed, "config", "user.name", "Crash Rehearsal")
        git(seed, "config", "user.email", "crash@company.internal")
        (seed / "README.md").write_text("Synthetic crash rehearsal\n")
        started, delayed = self.nonce + ".started", self.nonce + ".delayed"
        script = f": {self.nonce}; touch {started}; sleep 30; touch {delayed}"
        (seed / ".gitust-checks.json").write_text(json.dumps({
            "version": 1, "auto_run": True, "commands": [script]}))
        git(seed, "add", "README.md", ".gitust-checks.json")
        git(seed, "commit", "-m", "test: prepare service-crash execution fixture")
        git(self.root, "clone", "--bare", str(seed), str(self.upstream))
        git(self.root, "clone", "--bare", str(self.upstream), str(self.repo))
        self.auth.parent.mkdir(mode=0o700)
        command([str(self.binary), "admin", "create", "--username", "crash-operator",
                 "--email", "crash@company.internal", "--auth-db", str(self.auth)],
                env={"GITUST_ADMIN_PASSWORD": self.password})
        original, url = self.launch()
        self.healthy(original, url)
        login = self.helper.Client(url).request("POST", "/api/v1/auth/login", {
            "identifier": "crash-operator", "password": self.password})
        client = self.helper.Client(url, login["raw_session_token"])
        task = client.tool("gitust_worktree_spawn", {
            "branch": "agent/codex/" + self.nonce, "base_branch": "main", "agent": "codex",
            "prompt_summary": "Synthetic execution crash regression"})
        checkout = self.repo / "gitust-worktrees" / task["worktree_id"]
        expected = git(checkout, "rev-parse", "HEAD")
        (checkout / "README.md").write_text("Accepted automatic-check trigger\n")
        result = client.tool("gitust_worktree_commit", {
            "worktree_id": task["worktree_id"], "expected_head": expected,
            "paths": ["README.md"], "message": "test: trigger crash regression"})
        require(result.get("automatic_checks") == "queued", "automatic execution was not queued")
        wait_for(lambda: (checkout / started).is_file(), 10, "command did not start")
        lifecycle_route = "/api/v1/swarm/worktrees/" + task["worktree_id"] + "/automatic-runs"
        running = [row for row in client.request("GET", lifecycle_route) if row["phase"] == "running"]
        require(len(running) == 1, "started command has no unique durable Running attempt")
        attempt = running[0]
        require(attempt["expected"] == result["commit"]
                and attempt["user_id"] == login["user"]["id"]
                and attempt["origin"] == "mcp" and attempt["purpose"] == "automatic_checks",
                "durable automatic attempt has the wrong commit or actor binding")
        rows = self.ps()
        self.remember_groups(original, rows)
        require(self.groups, "no actual host execution group observed")
        server = rows.get(original.pid)
        require(server and str(self.binary) in server["args"] and str(self.repo) in server["args"],
                "fixture service identity changed")
        original.kill()
        original.wait(timeout=5)
        survived = bool(self.live_owned())
        restarted, _ = self.launch()
        try:
            status = restarted.wait(timeout=5)
        except subprocess.TimeoutExpired:
            raise RuntimeError("unsafe restart accepted while an execution was incomplete") from None
        require(status != 0, "incomplete execution did not reject fresh service ownership")
        markers = self.repo / "gitust-active-executions"
        require(markers.is_dir() and any(markers.iterdir()), "durable execution fence missing")
        self.stop_owned_groups()
        for name in [started, delayed]:
            (checkout / name).unlink(missing_ok=True)
        command([str(self.binary), "sandbox", "recover", "--repo", str(self.repo)], expected=2)
        require(any(markers.iterdir()), "recovery without acknowledgement changed fences")
        command([str(self.binary), "sandbox", "recover", "--repo", str(self.repo),
                 "--acknowledge-stopped-processes"])
        require(not any(markers.iterdir()), "confirmed recovery retained an execution fence")
        recovered, url = self.launch()
        self.healthy(recovered, url)
        client.url = url
        row = next(row for row in client.request("GET", "/api/v1/swarm/worktrees")
                   if row["id"] == task["worktree_id"])
        require(row["status"] == "failed" and row["verified_commit"] is None,
                "recovery restored verification authority")
        records = client.request("GET", lifecycle_route)
        interrupted = [row for row in records if row["run_id"] == attempt["run_id"]]
        require(len(interrupted) == 1 and interrupted[0]["phase"] == "interrupted"
                and interrupted[0]["expected"] == attempt["expected"],
                "uncertain execution was not retained as the same interrupted attempt")
        require(not any(row["phase"] in ["prepared", "queued", "running"] for row in records),
                "uncertain automatic attempt was rescheduled")
        result = client.tool("gitust_sandbox_exec", {
            "worktree_id": task["worktree_id"], "command": "true", "verify": False,
            "timeout_secs": 5})
        require(result["exit_code"] == 0 and result["network_isolated"],
                "ordinary execution did not recover safely")
        require(not any(markers.iterdir()), "normal cleanup retained a fence")
        require(not (checkout / started).exists() and not (checkout / delayed).exists(),
                "interrupted automatic command was replayed")
        git(self.repo, "fsck", "--full", "--strict")
        return {"version": 1, "passed": True, "source_commit": git(Path(__file__).resolve().parent.parent,
                 "rev-parse", "HEAD"), "binary_sha256": self.helper.sha256(self.binary),
                "runtime_worktree_dirty": bool(git(Path(__file__).resolve().parent.parent,
                                                   "status", "--porcelain")),
                "operator_script_sha256": self.helper.sha256(Path(__file__)),
                "platform": os.uname().sysname, "backend": result["backend"],
                "old_command_survived_sigkill": survived, "fresh_service_rejected": True,
                "explicit_recovery_required": True, "verification_restored": False,
                "post_recovery_execution_passed": True, "git_fsck": "passed",
                "automatic_attempt_retained_as_interrupted": True,
                "interrupted_automatic_attempt_replayed": False,
                "external_connections": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    require(not args.report.exists(), "report path already exists")
    spec = importlib.util.spec_from_file_location("alpha_helper", Path(__file__).with_name("alpha-rehearsal.py"))
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    with tempfile.TemporaryDirectory(prefix="gitust-crash-rehearsal-") as directory:
        fixture = CrashRehearsal(helper, args.binary.resolve(strict=True), Path(directory).resolve())
        try:
            report = fixture.execute()
        finally:
            fixture.cleanup()
    fd = os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Service-crash execution fence rehearsal passed; private fixture removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        # Never print process arguments, auth responses, captured logs or credentials.
        print("Service-crash rehearsal failed:", str(error) if isinstance(error, RuntimeError)
              else type(error).__name__)
        raise SystemExit(1) from None
