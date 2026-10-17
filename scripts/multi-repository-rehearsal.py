#!/usr/bin/env python3
"""Exercise two real repositories through one service without external publication."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
import urllib.error
import uuid


class Rehearsal:
    def __init__(self, helper, binary, root):
        self.helper, self.binary, self.root = helper, binary, root
        self.auth = root / "auth/auth-db.json"
        self.password = secrets.token_urlsafe(24)
        self.teams = [str(uuid.uuid4()), str(uuid.uuid4())]
        self.repos = [root / "primary", root / "secondary"]
        self.processes = []

    def require(self, condition, message):
        self.helper.require(condition, message)

    def git(self, repo, *args):
        return self.helper.git(repo, *args)

    def launch(self, primary, secondary=()):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        arguments = [str(self.binary), "serve", "--bind", f"127.0.0.1:{port}",
                     "--repo", str(primary), "--auth-db", str(self.auth),
                     "--sync-github", str(self.root / "unused-upstream.git")]
        for repo in secondary:
            arguments.extend(["--read-only-repo", str(repo)])
        process = subprocess.Popen(arguments, env=self.helper.clean_environment(),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.processes.append(process)
        return process, f"http://127.0.0.1:{port}"

    def healthy(self, process, url):
        for _ in range(100):
            self.require(process.poll() is None, "multi-repository service failed to start")
            try:
                self.helper.Client(url).request("GET", "/api/v1/health")
                return
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                time.sleep(0.1)
        raise RuntimeError("multi-repository service startup deadline")

    @staticmethod
    def stop(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def login(self, url, username, password):
        result = self.helper.Client(url).request("POST", "/api/v1/auth/login", {
            "identifier": username, "password": password})
        return self.helper.Client(url, result["raw_session_token"]), result["user"]["id"]

    def setup(self):
        for index, repo in enumerate(self.repos):
            repo.mkdir()
            self.git(repo, "init", "--initial-branch=main")
            self.git(repo, "config", "user.name", "Gitust Repository Rehearsal")
            self.git(repo, "config", "user.email", "repository@company.internal")
            (repo / "marker.txt").write_text(f"repository-{index}-before\n")
            self.git(repo, "add", "marker.txt")
            self.git(repo, "commit", "-m", "test: initial isolated repository")
            (repo / "marker.txt").write_text(f"repository-{index}-after\n")
            self.git(repo, "commit", "-am", "test: distinct repository content")
        self.helper.command([str(self.binary), "admin", "create", "--username", "operator",
                             "--email", "operator@company.internal", "--auth-db", str(self.auth)],
                            env={"GITUST_ADMIN_PASSWORD": self.password})
        self.register_repository(self.repos[0], "repository-0", self.teams[0])
        self.import_secondary()
        self.process, self.url = self.launch(self.repos[0], [self.repos[1]])
        self.healthy(self.process, self.url)
        self.admin, _ = self.login(self.url, "operator", self.password)
        self.members = []
        for index, team_ids in enumerate([[self.teams[0]], [self.teams[1]], []]):
            username = f"member-{index}"
            password = secrets.token_urlsafe(24)
            issued = self.admin.request("POST", "/api/v1/admin/invites", {
                "target_email": f"{username}@company.internal", "assigned_role": "Developer",
                "team_ids": team_ids}, expected=201)
            self.helper.Client(self.url).request("POST", "/api/v1/auth/invites/redeem", {
                "raw_token": issued["raw_token"], "username": username, "password": password},
                expected=201)
            self.members.append(self.login(self.url, username, password))
        catalog = self.admin.request("GET", "/api/v1/repos")["repositories"]
        self.require(len(catalog) == 2, "catalog does not contain both real repositories")
        by_default = {row["is_default"]: row for row in catalog}
        self.require(set(by_default) == {True, False}, "catalog default identity is ambiguous")
        self.rows = [by_default[True], by_default[False]]
        self.ids = [row["id"] for row in self.rows]
        self.require(self.ids[0] != self.ids[1], "catalog reused the primary repository UUID")
        self.require([row["capability"] for row in self.rows] == ["workbench", "browse"],
                     "catalog advertised unsupported repository mutations")
        serialized = json.dumps(catalog)
        self.require(str(self.root) not in serialized and "registration_nonce" not in serialized,
                     "catalog exposed private host metadata")

    def register_repository(self, repo, name, team):
        self.helper.command([str(self.binary), "repository", "register", "--repo", str(repo),
                             "--auth-db", str(self.auth), "--admin-username", "operator",
                             "--name", name, "--owner-team-id", team, "--private", "true"],
                            env={"GITUST_ADMIN_PASSWORD": self.password})

    def import_secondary(self):
        seed, upstream = self.root / "secondary-seed", self.root / "secondary-upstream.git"
        self.repos[1].rename(seed)
        self.git(self.root, "clone", "--bare", "--no-local", str(seed), str(upstream))
        self.git(upstream, "-c", "user.name=Gitust-Rehearsal",
                 "-c", "user.email=repository@company.internal", "tag", "-a", "import-proof",
                 "-m", "Annotated source tag must survive import")
        self.git(upstream, "branch", "upstream-default", "HEAD^")
        self.git(upstream, "symbolic-ref", "HEAD", "refs/heads/upstream-default")
        self.require(self.git(upstream, "rev-parse", "HEAD") != self.git(upstream, "rev-parse", "main"),
                     "fixture must distinguish requested and upstream default commits")
        self.register_repository(upstream, "upstream-fixture", self.teams[1])
        source_identity = upstream / "gitust-repository/identity.json"
        source_receipt = upstream / "gitust-repository/registration.json"
        before = (self.git(upstream, "show-ref"), source_identity.read_bytes(), source_receipt.read_bytes(),
                  self.git(upstream, "symbolic-ref", "HEAD"))
        arguments = [str(self.binary), "repository", "import", str(upstream),
                     "--repo", str(self.repos[1]), "--auth-db", str(self.auth),
                     "--admin-username", "operator", "--name", "repository-1",
                     "--owner-team-id", self.teams[1], "--private", "true", "--default-branch", "main"]
        self.import_arguments = arguments
        self.helper.command(arguments, env={"GITUST_ADMIN_PASSWORD": self.password})
        target_identity = self.repos[1] / "gitust-repository/identity.json"
        target_receipt = self.repos[1] / "gitust-repository/registration.json"
        target_before = (target_identity.read_bytes(), target_receipt.read_bytes(),
                         self.git(self.repos[1], "show-ref"))
        self.unhosted_registered_id = json.loads(before[1])["repo_id"]
        self.require(json.loads(target_before[0])["repo_id"] != self.unhosted_registered_id,
                     "import adopted the source installation's repository identity")
        self.require(self.git(self.repos[1], "rev-parse", "HEAD") == self.git(upstream, "rev-parse", "main")
                     and self.git(self.repos[1], "symbolic-ref", "HEAD") == "refs/heads/main",
                     "import did not establish the requested default branch")
        self.require(self.git(self.repos[1], "rev-parse", "refs/tags/import-proof")
                     == self.git(upstream, "rev-parse", "refs/tags/import-proof")
                     and self.git(self.repos[1], "cat-file", "-t", "refs/tags/import-proof") == "tag",
                     "import lost or peeled the annotated tag object")
        self.helper.command(arguments, env={"GITUST_ADMIN_PASSWORD": self.password})
        self.require(target_before == (target_identity.read_bytes(), target_receipt.read_bytes(),
                                       self.git(self.repos[1], "show-ref")),
                     "completed import retry replaced identity, registration or refs")
        self.require(before == (self.git(upstream, "show-ref"), source_identity.read_bytes(),
                                source_receipt.read_bytes(), self.git(upstream, "symbolic-ref", "HEAD")),
                     "import changed the source installation")
        self.git(upstream, "fsck", "--full", "--strict")

    def catalog_acl(self):
        for index, (client, _) in enumerate(self.members):
            rows = client.request("GET", "/api/v1/repos")["repositories"]
            expected = [self.ids[index]] if index < 2 else []
            self.require([row["id"] for row in rows] == expected,
                         "catalog did not filter by the caller's current team")
        self.helper.Client(self.url).request("GET", "/api/v1/repos", expected=401)

    def read_repository(self, index):
        client = self.members[index][0]
        repo, repo_id = self.repos[index], self.ids[index]
        prefix = f"/api/v1/repos/{repo_id}"
        head, base = self.git(repo, "rev-parse", "HEAD"), self.git(repo, "rev-parse", "HEAD^")
        row = self.rows[index]
        self.require(row["head_commit"] == head and row["branch"] == "main",
                     "catalog HEAD differs from the actual repository")
        commits = client.request("GET", prefix + "/commits?rev=HEAD&limit=2")
        self.require(commits["head_sha"] == head and
                     [commit["sha"] for commit in commits["commits"]] == [head, base],
                     "UUID commit routing selected the wrong repository")
        tree = client.request("GET", prefix + "/tree?rev=" + head + "&recursive=true")
        self.require(tree["commit_sha"] == head and
                     [entry["path"] for entry in tree["entries"]] == ["marker.txt"],
                     "UUID tree routing selected the wrong revision")
        blob = client.request("GET", prefix + "/blob?rev=" + head + "&path=marker.txt")
        self.require(blob["commit_sha"] == head and
                     blob["content"] == self.git(repo, "show", head + ":marker.txt") + "\n" and
                     blob["oid"] == self.git(repo, "rev-parse", head + ":marker.txt"),
                     "UUID blob routing differs from independent Git content/object identity")
        diff = client.request("GET", prefix + f"/diff?base={base}&head={head}&path=marker.txt")
        self.require(diff["head_sha"] == head and diff["base_sha"] == base and
                     [row["path"] for row in diff["files"]] == ["marker.txt"] and
                     any(row["content"] == f"repository-{index}-after" for row in diff["lines"]),
                     "UUID diff routing selected another repository's comparison")

    def rename_repositories(self):
        self.renamed_names = ["primary-renamed", "secondary-renamed"]
        for index, repo_id in enumerate(self.ids):
            repo, name = self.repos[index], self.renamed_names[index]
            common = Path(self.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
            route = f"/api/v1/repos/{repo_id}"
            metadata = self.admin.request("GET", route)
            before = (self.git(repo, "show-ref"), self.git(repo, "symbolic-ref", "HEAD"),
                      (common / "gitust-repository/identity.json").read_bytes(),
                      (common / "gitust-repository/registration.json").read_bytes())
            changed = self.admin.request("PATCH", route, {"name": name})
            self.require(changed == {"id": repo_id, "name": name}, "rename returned an unrelated target")
            self.members[index][0].request("PATCH", route, {"name": "must-not-change"}, expected=403)
            self.helper.Client(self.url).request("PATCH", route, {"name": "must-not-change"}, expected=401)
            self.admin.request("PATCH", route, {"name": name, "is_private": False}, expected=400)
            self.require(self.admin.request("GET", route) == {**metadata, "name": name},
                         "rename changed repository identity, ownership, privacy or creation time")
            after = (self.git(repo, "show-ref"), self.git(repo, "symbolic-ref", "HEAD"),
                     (common / "gitust-repository/identity.json").read_bytes(),
                     (common / "gitust-repository/registration.json").read_bytes())
            self.require(before == after, "display rename changed Git refs or registration anchors")
        rows = self.admin.request("GET", "/api/v1/repos")["repositories"]
        self.require({row["id"]: row["name"] for row in rows} == dict(zip(self.ids, self.renamed_names))
                     and all(row["can_rename"] is True for row in rows),
                     "catalog did not reflect the current administrator's renamed repositories")
        self.require(self.admin.request("GET", "/api/v1/repo")["name"] == self.renamed_names[0],
                     "primary summary returned the old repository name")
        for index in range(2):
            rows = self.members[index][0].request("GET", "/api/v1/repos")["repositories"]
            self.require(len(rows) == 1 and rows[0]["name"] == self.renamed_names[index]
                         and rows[0]["can_rename"] is False,
                         "catalog rename hint or name did not match the current member")

    def denial_boundaries(self):
        suffixes = ["/commits", "/tree", "/blob?path=marker.txt", "/diff"]
        for index in range(2):
            other = self.members[1 - index][0]
            for suffix in suffixes:
                other.request("GET", f"/api/v1/repos/{self.ids[index]}" + suffix, expected=403)
        metadata = self.admin.request("POST", "/api/v1/repos", {
            "name": "metadata-only", "owner_team_id": self.teams[1], "is_private": True},
            expected=201)
        for repo_id in [metadata["id"], self.unhosted_registered_id, str(uuid.uuid4())]:
            for suffix in suffixes:
                self.admin.request("GET", f"/api/v1/repos/{repo_id}" + suffix, expected=404)
            self.admin.request("PATCH", f"/api/v1/repos/{repo_id}", {"name": "not-hosted"}, expected=404)
        self.require(len(self.admin.request("GET", "/api/v1/repos")["repositories"]) == 2,
                     "metadata-only records became configured Git engines")
        self.require(self.admin.request("GET", "/api/v1/repo")["id"] == self.ids[0],
                     "legacy summary no longer selects the primary")
        secondary = self.members[1][0]
        for route in ["/api/v1/repo", "/api/v1/repo/commits", "/api/v1/swarm/worktrees"]:
            secondary.request("GET", route, expected=403)
        for suffix in ["/swarm/worktrees", "/automatic-runs", "/sync/github"]:
            self.admin.request("POST", f"/api/v1/repos/{self.ids[1]}" + suffix, {}, expected=404)
        before = [(self.git(repo, "show-ref"), self.git(repo, "worktree", "list", "--porcelain"))
                  for repo in self.repos]
        payload = {"repo_id": self.ids[1], "branch": "agent/read-only-probe",
                   "base_branch": "main", "prompt_summary": "Must not materialize a secondary worktree"}
        self.admin.request("POST", "/api/v1/swarm/worktrees", payload, expected=404)
        secondary.request("POST", "/api/v1/swarm/worktrees", payload, expected=404)
        secondary.request("POST", "/api/v1/swarm/worktrees", {
            **payload, "repo_id": self.ids[0]}, expected=403)
        after = [(self.git(repo, "show-ref"), self.git(repo, "worktree", "list", "--porcelain"))
                 for repo in self.repos]
        self.require(before == after, "rejected secondary mutations changed repository refs or worktrees")
        self.admin.request("GET", f"/git/{self.ids[1]}.git/info/refs?service=git-receive-pack",
                           expected=404)
        revoked, _ = self.login(self.url, "operator", self.password)
        revoked.request("POST", "/api/v1/auth/logout", expected=204)
        revoked.request("GET", "/api/v1/repos", expected=401)
        revoked.request("PATCH", f"/api/v1/repos/{self.ids[0]}", {"name": "revoked"}, expected=401)
        issued = self.members[1][0].request("POST", "/api/v1/auth/pats", {"name": "read-probe"},
                                            expected=201)
        pat = self.helper.Client(self.url, issued["raw_token"])
        pat.request("GET", f"/api/v1/repos/{self.ids[1]}/tree")
        self.admin.request("POST", f"/api/v1/admin/users/{self.members[1][1]}/deactivate", {})
        pat.request("GET", f"/api/v1/repos/{self.ids[1]}/tree", expected=403)
        self.members[1][0].request("GET", "/api/v1/repos", expected=403)

    def restart_and_rollback(self):
        self.stop(self.process)
        self.helper.command(self.import_arguments, env={"GITUST_ADMIN_PASSWORD": self.password}, expected=1)
        self.process, self.url = self.launch(self.repos[0], [self.repos[1]])
        self.healthy(self.process, self.url)
        self.admin, _ = self.login(self.url, "operator", self.password)
        rows = self.admin.request("GET", "/api/v1/repos")["repositories"]
        self.require({row["id"] for row in rows} == set(self.ids),
                     "repository UUIDs changed on multi-repository restart")
        self.require({row["id"]: row["name"] for row in rows} == dict(zip(self.ids, self.renamed_names)),
                     "restart or an obsolete import retry reverted renamed repository metadata")
        self.stop(self.process)
        blocker, url = self.launch(self.repos[1])
        self.healthy(blocker, url)
        rejected, _ = self.launch(self.repos[0], [self.repos[1]])
        self.require(rejected.wait(timeout=15) != 0, "startup accepted a busy secondary repository")
        recovered, url = self.launch(self.repos[0])
        self.healthy(recovered, url)
        self.stop(recovered)
        self.stop(blocker)
        duplicate, _ = self.launch(self.repos[0], [self.repos[0]])
        self.require(duplicate.wait(timeout=15) != 0, "startup accepted duplicate repository aliases")
        recovered, url = self.launch(self.repos[0], [self.repos[1]])
        self.healthy(recovered, url)
        self.stop(recovered)

    def execute(self):
        self.setup()
        self.catalog_acl()
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(self.read_repository, [0, 1]))
        self.rename_repositories()
        self.denial_boundaries()
        self.restart_and_rollback()
        for repo in self.repos:
            self.git(repo, "fsck", "--full", "--strict")
        source = Path(__file__).resolve().parent.parent
        return {"version": 1, "passed": True,
                "source_commit": self.git(source, "rev-parse", "HEAD"),
                "runtime_worktree_dirty": bool(self.git(source, "status", "--porcelain")),
                "binary_sha256": self.helper.sha256(self.binary), "platform": os.uname().sysname,
                "operator_script_sha256": self.helper.sha256(Path(__file__)),
                "actual_repositories": 2, "team_filtered_catalog": True,
                "imported_registered_secondary": True,
                "renamed_registered_repositories": True,
                "independent_git_read_oracles": True, "primary_only_mutations": True,
                "revoked_and_deactivated_denied": True, "restart_identity_preserved": True,
                "partial_startup_locks_released": True, "duplicate_repository_rejected": True,
                "external_github_publication": False, "git_fsck": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    if args.report.exists():
        raise RuntimeError("report already exists; choose a new path")
    spec = importlib.util.spec_from_file_location("gitust_alpha", Path(__file__).with_name("alpha-rehearsal.py"))
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    with tempfile.TemporaryDirectory(prefix="gitust-multi-repository-") as directory:
        fixture = Rehearsal(helper, binary, Path(directory).resolve())
        try:
            report = fixture.execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
    fd = os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Two-repository routing, ACL and startup rollback passed; private fixture removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        print("Multi-repository rehearsal failed:", str(error) if isinstance(error, RuntimeError)
              else type(error).__name__)
        raise SystemExit(1) from None
