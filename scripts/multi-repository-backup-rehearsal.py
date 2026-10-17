#!/usr/bin/env python3
"""Restore a stopped two-repository installation with every original path unavailable."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import tempfile


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


class Rehearsal:
    def __init__(self, helper, fixture, backup_tool, scratch):
        self.h, self.f, self.tool, self.scratch = helper, fixture, backup_tool, scratch
        self.backends = set()

    def require(self, condition, message):
        self.h.require(condition, message)

    def start(self, repo, secondaries=(), *, auth=None):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        # Approval recovery validates the configured destination even without publishing.
        # Each service gets a fresh local placeholder, never a source installation path.
        upstream = self.scratch / f"unused-upstream-{port}.git"
        self.h.git(self.scratch, "init", "--bare", str(upstream))
        argv = [str(self.f.binary), "serve", "--bind", f"127.0.0.1:{port}",
                "--repo", str(repo), "--auth-db", str(auth or self.f.auth),
                "--sync-github", str(upstream),
                "--trusted-single-user-sandbox"]
        for secondary in secondaries:
            argv.extend(["--read-only-repo", str(secondary)])
        process = subprocess.Popen(argv, env=self.h.clean_environment(),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.f.processes.append(process)
        url = f"http://127.0.0.1:{port}"
        self.f.healthy(process, url)
        client, user_id = self.f.login(url, "operator", self.f.password)
        return process, client, user_id

    def spawn(self, client, label):
        return client.tool("gitust_worktree_spawn", {
            "branch": "agent/backup-" + label, "base_branch": "main", "agent": "codex",
            "prompt_summary": "Stopped multi-repository restore rehearsal " + label})

    def run(self, client, task, command, *, verify=False):
        result = client.tool("gitust_sandbox_exec", {
            "worktree_id": task["worktree_id"], "command": command,
            "timeout_secs": 60, "verify": verify})
        self.require(result["exit_code"] == 0 and result["network_isolated"] is True,
                     "fixture command did not complete inside the OS sandbox")
        self.backends.add(result["backend"])
        return result

    def commit(self, client, repo, task, path):
        parent = self.h.git(repo, "rev-parse", task["branch"])
        result = client.tool("gitust_worktree_commit", {
            "worktree_id": task["worktree_id"], "expected_head": parent,
            "paths": [path], "message": "test: durable backup fixture change"})
        sha = self.h.git(repo, "rev-parse", task["branch"])
        self.require(result["parent"] == parent and result["commit"] == sha
                     and result["status"] == "pending" and result["verification_required"],
                     "fixture commit granted authority or differs from actual Git")
        return sha

    def checkout(self, repo, task):
        bare = self.h.git(repo, "rev-parse", "--is-bare-repository") == "true"
        base = repo / "gitust-worktrees" if bare else repo.parent / f".{repo.name}.gitust-worktrees"
        for entry in self.h.git(repo, "worktree", "list", "--porcelain").split("\n\n"):
            fields = entry.splitlines()
            if "branch refs/heads/" + task["branch"] in fields:
                path = Path(fields[0].removeprefix("worktree "))
                self.require(path.parent == base and path.name == task["worktree_id"],
                             "fixture checkout is outside the managed base")
                return path
        raise RuntimeError("managed fixture checkout is not registered with Git")

    def common(self, repo):
        return Path(self.h.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))

    def records(self, repo, tasks, approved=()):
        common, records = self.common(repo), {}
        for name in ["identity.json", "registration.json"]:
            records["common/gitust-repository/" + name] = (common / "gitust-repository" / name)
        for task in tasks:
            task_id = task["worktree_id"]
            checkout = self.checkout(repo, task)
            admin = Path(self.h.git(checkout, "rev-parse", "--absolute-git-dir"))
            records["origin/" + task_id] = admin / "gitust-origin.json"
            for directory in ["gitust-owners", "gitust-check-runs"]:
                records[f"common/{directory}/{task_id}.json"] = common / directory / (task_id + ".json")
        for task in approved:
            task_id = task["worktree_id"]
            records[f"common/gitust-approvals/{task_id}.json"] = (
                common / "gitust-approvals" / (task_id + ".json"))
        return {key: (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
                for key, path in records.items()}

    def git_state(self, repo):
        return (self.h.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)"),
                self.h.git(repo, "symbolic-ref", "HEAD"))

    def prepare(self):
        self.f.setup()
        self.f.rename_repositories()
        self.f.stop(self.f.process)
        secondary_process, secondary_client, user_id = self.start(self.f.repos[1])
        secondary = self.spawn(secondary_client, "secondary")
        self.run(secondary_client, secondary, "printf 'secondary uncommitted\\n' > dirty-secondary.txt")
        self.f.stop(secondary_process)
        process, client, operator_id = self.start(self.f.repos[0], [self.f.repos[1]])
        self.require(operator_id == user_id, "shared AuthDB changed the operator identity")
        approved = self.spawn(client, "approved")
        self.run(client, approved, "printf 'approved source\\n' > approved.txt")
        approved_sha = self.commit(client, self.f.repos[0], approved, "approved.txt")
        checked = self.run(client, approved, "test -f approved.txt", verify=True)
        self.require(checked["checked_commit"] == approved_sha,
                     "verification is not bound to the committed fixture revision")
        row = client.request("POST", "/api/v1/swarm/worktrees/" + approved["worktree_id"] + "/approve",
                             {"expected_commit": approved_sha})
        self.require(row["status"] == "merged"
                     and self.h.git(self.f.repos[0], "rev-parse", "main") == approved_sha,
                     "source human approval did not update main to the reviewed SHA")
        pending = self.spawn(client, "pending")
        self.run(client, pending, "printf 'pending source\\n' > pending.txt")
        pending_sha = self.commit(client, self.f.repos[0], pending, "pending.txt")
        result = self.run(client, pending, "test -f pending.txt", verify=True)
        self.require(result["checked_commit"] == pending_sha,
                     "pending fixture has no actual passing verification")
        history = client.request("GET", "/api/v1/swarm/worktrees/" + pending["worktree_id"] + "/runs")
        self.require(any(run["verification_requested"] and run["result"]["exit_code"] == 0
                         and run["checked_commit"] == pending_sha for run in history),
                     "pending fixture lacks stored passing diagnostics")
        catalog = client.request("GET", "/api/v1/repos")["repositories"]
        self.f.stop(process)
        dirty = self.checkout(self.f.repos[0], pending) / "marker.txt"
        dirty.write_text("primary uncommitted after verification\n")
        self.require(bool(self.h.git(dirty.parent, "status", "--porcelain")),
                     "pending source checkout must contain uncommitted data")
        self.tasks = [approved, pending, secondary]
        self.operator_id, self.history, self.pending_sha = operator_id, history, pending_sha
        self.catalog = {row["id"]: row for row in catalog}
        self.states = [self.git_state(repo) for repo in self.f.repos]
        self.durable = [self.records(self.f.repos[0], [approved, pending], [approved]),
                        self.records(self.f.repos[1], [secondary])]
        self.auth_bytes = self.f.auth.read_bytes()
        for index, tasks in enumerate([[approved, pending], [secondary]]):
            for task in tasks:
                owner = json.loads(self.durable[index][f"common/gitust-owners/{task['worktree_id']}.json"][0])
                self.require(owner["user_id"] == operator_id and owner["branch"] == task["branch"],
                             "source worktree owner record differs from actual actor")
        print("PASS source fixtures: shared ACL, approval, passing history and dirty worktrees", flush=True)

    def restore(self):
        backup, restored = self.scratch / "checkpoint", self.scratch / "restored"
        self.h.command([sys.executable, str(self.tool), "create", "--repo", str(self.f.repos[0]),
                        "--read-only-repo", str(self.f.repos[1]), "--auth-db", str(self.f.auth),
                        "--output", str(backup)])
        manifest = json.loads((backup / "manifest.json").read_text())
        self.require(manifest["version"] == 2 and len(manifest["repositories"]) == 2,
                     "multi snapshot is not an explicit two-repository v2 checkpoint")
        auth_files = [key for key in manifest["files"] if key.endswith("auth-db.json")]
        self.require(auth_files == ["auth/auth-db.json"] and
                     (backup / auth_files[0]).read_bytes() == self.auth_bytes,
                     "snapshot duplicated or changed the shared authentication checkpoint")
        original_paths = [*self.f.repos, self.f.repos[0].parent / ".primary.gitust-worktrees", self.f.auth]
        self.f.root.rename(self.scratch / "original-unavailable")
        self.require(all(not path.exists() for path in original_paths),
                     "an original repository, external worktree base or AuthDB is still available")
        self.h.command([sys.executable, str(self.tool), "restore", "--backup", str(backup),
                        "--destination", str(restored)])
        self.restored = [restored / "repository", restored / "read-only/0001/repository"]
        auth = restored / "auth/auth-db.json"
        self.require(auth.read_bytes() == self.auth_bytes and stat.S_IMODE(auth.stat().st_mode) == 0o600,
                     "restored shared AuthDB bytes or private permissions differ")
        for index, tasks in enumerate([self.tasks[:2], self.tasks[2:]]):
            repo = self.restored[index]
            self.require(self.git_state(repo) == self.states[index], "restored refs or symbolic HEAD differ")
            self.require(self.records(repo, tasks, self.tasks[:1] if index == 0 else []) == self.durable[index],
                         "restore changed identity, registration, worktree owner/origin, approval or history")
            self.h.git(repo, "fsck", "--full", "--strict")
            for task in tasks:
                self.h.git(self.checkout(repo, task), "fsck", "--full", "--strict")
        self.require((self.checkout(self.restored[0], self.tasks[1]) / "marker.txt").read_text()
                     == "primary uncommitted after verification\n", "primary dirty working file was lost")
        self.require((self.checkout(self.restored[1], self.tasks[2]) / "dirty-secondary.txt").read_text()
                     == "secondary uncommitted\n", "secondary dirty working file was lost")
        self.require(self.h.git(self.restored[1], "cat-file", "-t", "refs/tags/import-proof") == "tag",
                     "restore peeled the annotated tag object")
        print("PASS stopped v2 restore: all original paths unavailable, exact durable records and Git fsck", flush=True)
        return auth, original_paths

    def verify_service(self, auth, original_paths):
        process, client, operator_id = self.start(self.restored[0], [self.restored[1]], auth=auth)
        self.require(operator_id == self.operator_id and all(not p.exists() for p in original_paths),
                     "restore service used a changed actor or an original source path")
        catalog = {row["id"]: row for row in client.request("GET", "/api/v1/repos")["repositories"]}
        self.require(catalog == self.catalog, "restored catalog UUID, name, branch, capability or ACL hint differs")
        for index, (member, _) in enumerate(self.f.members):
            member.url = client.url
            rows = member.request("GET", "/api/v1/repos")["repositories"]
            expected = [self.f.ids[index]] if index < 2 else []
            self.require([row["id"] for row in rows] == expected,
                         "restored catalog does not enforce the current team membership")
            for repo_index, repo_id in enumerate(self.f.ids):
                if index != repo_index:
                    member.request("GET", f"/api/v1/repos/{repo_id}/tree", expected=403)
        self.h.Client(client.url).request("GET", "/api/v1/repos", expected=401)
        states = [self.git_state(repo) for repo in self.restored]
        for actor in [client, self.f.members[1][0]]:
            actor.request("POST", "/api/v1/swarm/worktrees", {
                "repo_id": self.f.ids[1], "branch": "agent/no-secondary-write", "base_branch": "main",
                "prompt_summary": "Must remain browse only"}, expected=404)
            actor.request("GET", "/git/" + self.f.ids[1] + ".git/info/refs?service=git-upload-pack", expected=404)
        for index, repo in enumerate(self.restored):
            actor, repo_id = self.f.members[index][0], self.f.ids[index]
            sha = self.h.git(repo, "rev-parse", "HEAD")
            prefix = f"/api/v1/repos/{repo_id}"
            commits = actor.request("GET", prefix + "/commits?rev=" + sha + "&limit=2")
            self.require(commits["head_sha"] == sha and commits["commits"][0]["sha"] == sha,
                         "restored UUID commit routing selected another repository")
            tree = actor.request("GET", prefix + "/tree?rev=" + sha + "&recursive=true")
            self.require(tree["commit_sha"] == sha and any(row["path"] == "marker.txt" for row in tree["entries"]),
                         "restored UUID tree does not match its actual Git revision")
            blob = actor.request("GET", prefix + "/blob?rev=" + sha + "&path=marker.txt")
            self.require(blob["commit_sha"] == sha and
                         blob["content"] == self.h.git(repo, "show", sha + ":marker.txt") + "\n" and
                         blob["oid"] == self.h.git(repo, "rev-parse", sha + ":marker.txt"),
                         "restored UUID blob differs from independent Git contents/identity")
            self.require(self.git_state(repo) == states[index], "read or denied secondary mutation changed refs")
        rows = {row["id"]: row for row in client.request("GET", "/api/v1/swarm/worktrees")}
        approved, pending = self.tasks[:2]
        self.require(rows[approved["worktree_id"]]["status"] == "merged",
                     "restored durable human approval did not recover its merged task")
        row = rows[pending["worktree_id"]]
        self.require(row["status"] == "pending" and row["verified_commit"] is None
                     and row["verified_checks_oid"] is None,
                     "restored passing diagnostics incorrectly granted verification authority")
        self.require(client.request("GET", "/api/v1/swarm/worktrees/" + pending["worktree_id"] + "/runs")
                     == self.history, "restored completed diagnostics differ from the stopped source")
        client.request("POST", "/api/v1/swarm/worktrees/" + pending["worktree_id"] + "/approve",
                       {"expected_commit": self.pending_sha}, expected=409)
        self.require(self.git_state(self.restored[0]) == states[0], "unverified restored approval changed refs")
        self.f.stop(process)
        for repo in self.restored:
            self.h.git(repo, "fsck", "--full", "--strict")
        print("PASS restored-only service: UUID routing, team ACL, browse-only writes and fresh-verification gate", flush=True)

    def execute(self):
        self.prepare()
        auth, original_paths = self.restore()
        self.verify_service(auth, original_paths)
        return {"version": 1, "passed": True, "repository_count": 2,
                "all_sources_unavailable": True, "repository_ids_preserved": True,
                "catalog_and_team_acl": True, "secondary_writes_denied": True,
                "refs_and_git_fsck": True, "worktree_records_preserved": True,
                "pending_requires_fresh_verification": True, "single_auth_checkpoint": True,
                "external_github_tested": False, "platform": os.uname().sysname,
                "sandbox_backends": sorted(self.backends)}


def runtime_hashes(helper, script, backup):
    files = {"scripts/" + path.name: path for path in
             [script, script.with_name("alpha-rehearsal.py"), script.with_name("multi-repository-rehearsal.py")]}
    files["scripts/backup.py"] = backup
    return {name: helper.sha256(path) for name, path in sorted(files.items())}


def provenance(helper, source, runtime):
    commit = helper.git(source, "rev-parse", "HEAD")
    dirty = bool(helper.git(source, "status", "--porcelain"))
    for name, digest in runtime.items():
        result = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                                env=helper.clean_environment(), capture_output=True, timeout=30)
        dirty |= result.returncode != 0 or hashlib.sha256(result.stdout).hexdigest() != digest
    return commit, bool(dirty)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--backup-tool", type=Path, default=Path(__file__).with_name("backup.py"))
    args = parser.parse_args()
    script, source = Path(__file__).resolve(), args.source.resolve(strict=True)
    binary, backup = args.binary.resolve(strict=True), args.backup_tool.resolve(strict=True)
    if args.report.exists() or args.report.is_symlink():
        raise RuntimeError("report already exists; choose a new path")
    helper = module("backup_alpha", script.with_name("alpha-rehearsal.py"))
    multi = module("backup_multi", script.with_name("multi-repository-rehearsal.py"))
    runtime, binary_sha = runtime_hashes(helper, script, backup), helper.sha256(binary)
    commit, dirty = provenance(helper, source, runtime)
    with tempfile.TemporaryDirectory(prefix="gitust-multi-backup-") as directory:
        scratch = Path(directory).resolve()
        installation = scratch / "installation"
        installation.mkdir()
        fixture = multi.Rehearsal(helper, binary, installation)
        try:
            report = Rehearsal(helper, fixture, backup, scratch).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            helper.require(all(process.poll() is not None for process in fixture.processes),
                           "owned fixture service was not reaped")
    helper.require(not scratch.exists(), "private rehearsal fixture was not removed")
    helper.require(runtime_hashes(helper, script, backup) == runtime and helper.sha256(binary) == binary_sha,
                   "rehearsal source or binary changed during execution")
    final_commit, final_dirty = provenance(helper, source, runtime)
    helper.require(final_commit == commit,
                   "provenance source commit changed during execution")
    report.update({"source_commit": commit, "source_worktree_dirty": dirty or final_dirty,
                   "binary_sha256": binary_sha, "operator_script_sha256": helper.sha256(script),
                   "runtime_source_sha256": runtime, "owned_services_reaped": True,
                   "private_fixture_removed": True})
    fd = os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Stopped multi-repository restore passed; all private fixtures and services removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print("Multi-repository backup rehearsal failed:",
              str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
