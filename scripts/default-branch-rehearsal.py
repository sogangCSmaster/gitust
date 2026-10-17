#!/usr/bin/env python3
"""Change primary and opted-in secondary defaults without retargeting an existing task."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Rehearsal:
    def __init__(self, helper, fixture, creation):
        self.h, self.f, self.creation = helper, fixture, creation
        self.backends = set()

    def require(self, condition, message):
        self.h.require(condition, message)

    def start(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        upstream = self.f.root / "unused-upstream.git"
        if not upstream.exists():
            self.h.git(self.f.root, "init", "--bare", str(upstream))
        process = subprocess.Popen([
            str(self.f.binary), "serve", "--bind", f"127.0.0.1:{port}",
            "--repo", str(self.f.repos[0]), "--read-only-repo", str(self.f.repos[1]),
            "--git-write-repo", str(self.f.repos[2]),
            "--auth-db", str(self.f.auth), "--sync-github", str(upstream),
            "--trusted-single-user-sandbox"], env=self.h.clean_environment(),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.f.processes.append(process)
        self.url = f"http://127.0.0.1:{port}"
        self.f.healthy(process, self.url)
        self.admin, self.actor = self.f.login(self.url, "operator", self.f.password)
        self.process = process

    def transport(self):
        return {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.extraHeader",
                "GIT_CONFIG_VALUE_0": "Authorization: Bearer " + self.admin.token,
                "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"}

    def git_http(self, repo, *args):
        return self.h.git(repo, *args, env=self.transport())

    def refs(self):
        return self.h.git(self.f.repos[0], "for-each-ref", "--format=%(refname)%00%(objectname)")

    def git_state(self, repo):
        return (self.h.git(repo, "symbolic-ref", "HEAD"),
                self.h.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)"))

    def setup(self):
        self.h.command([str(self.f.binary), "admin", "create", "--username", "operator",
                        "--email", "operator@company.internal", "--auth-db", str(self.f.auth)],
                       env={"GITUST_ADMIN_PASSWORD": self.f.password})
        self.ids, self.anchors = [], []
        self.f.repos.append(self.f.root / "git-write.git")
        for index, repo in enumerate(self.f.repos):
            arguments = self.creation.create_arguments(self.f, index) if index < 2 else [
                str(self.f.binary), "repository", "create", "--repo", str(repo),
                "--auth-db", str(self.f.auth), "--admin-username", "operator",
                "--name", "writable-secondary", "--owner-team-id", self.f.teams[0],
                "--private", "true", "--default-branch", "main"]
            self.h.command(arguments,
                           env={"GITUST_ADMIN_PASSWORD": self.f.password})
            pair = self.creation.anchors(repo)
            self.ids.append(json.loads(pair[0])["repo_id"])
            self.anchors.append(pair)
        self.start()
        self.route = f"/api/v1/repos/{self.ids[0]}/default-branch"
        invitation = self.admin.request("POST", "/api/v1/admin/invites", {
            "target_email": "member@company.internal", "assigned_role": "Developer",
            "team_ids": [self.f.teams[0]]}, expected=201)
        password = secrets.token_urlsafe(24)
        self.h.Client(self.url).request("POST", "/api/v1/auth/invites/redeem", {
            "raw_token": invitation["raw_token"], "username": "member", "password": password},
            expected=201)
        self.member, _ = self.f.login(self.url, "member", password)
        self.client = self.f.root / "existing-clone"
        self.git_http(self.f.root, "clone", self.url + f"/git/{self.ids[0]}.git", str(self.client))
        self.h.git(self.client, "config", "user.name", "Gitust Default Branch Rehearsal")
        self.h.git(self.client, "config", "user.email", "default@company.internal")
        (self.client / "README.md").write_text("# Main snapshot\n")
        self.h.git(self.client, "add", "README.md")
        self.h.git(self.client, "commit", "-m", "test: initial main snapshot")
        self.main = self.h.git(self.client, "rev-parse", "HEAD")
        self.git_http(self.client, "push", "origin", "HEAD:refs/heads/main")
        self.h.git(self.client, "checkout", "-b", "develop")
        (self.client / "README.md").write_text("# Develop snapshot\n")
        self.h.git(self.client, "commit", "-am", "test: distinct develop snapshot")
        self.develop = self.h.git(self.client, "rev-parse", "HEAD")
        self.h.git(self.client, "tag", "-a", "preserved-tag", "-m", "Preserve the tag object")
        self.git_http(self.client, "push", "origin", "HEAD:refs/heads/develop", "refs/tags/preserved-tag")
        self.require(self.main != self.develop, "fixture must distinguish default branch contents")
        self.secondary_seed()

    def secondary_seed(self):
        self.secondary_client = self.f.root / "secondary-existing-clone"
        self.git_http(self.f.root, "clone", self.url + f"/git/{self.ids[2]}.git",
                      str(self.secondary_client))
        self.h.git(self.secondary_client, "config", "user.name", "Gitust Secondary Default Rehearsal")
        self.h.git(self.secondary_client, "config", "user.email", "secondary-default@company.internal")
        marker = self.secondary_client / "README.md"
        marker.write_text("# Secondary main\n")
        self.h.git(self.secondary_client, "add", "README.md")
        self.h.git(self.secondary_client, "commit", "-m", "test: secondary main")
        self.secondary_main = self.h.git(self.secondary_client, "rev-parse", "HEAD")
        self.git_http(self.secondary_client, "push", "origin", "HEAD:refs/heads/main")
        self.h.git(self.secondary_client, "checkout", "-b", "release")
        marker.write_text("# Secondary release\n")
        self.h.git(self.secondary_client, "commit", "-am", "test: secondary release")
        self.secondary_release = self.h.git(self.secondary_client, "rev-parse", "HEAD")
        self.h.git(self.secondary_client, "tag", "-a", "secondary-tag", "-m", "Keep raw annotated tag")
        self.secondary_tag = self.h.git(self.secondary_client, "rev-parse", "refs/tags/secondary-tag")
        self.git_http(self.secondary_client, "push", "origin", "HEAD:refs/heads/release", "refs/tags/secondary-tag")
        self.require(len({self.main, self.develop, self.secondary_main, self.secondary_release}) == 4,
                     "repositories and branches need independent commit identities")
        invite = self.admin.request("POST", "/api/v1/admin/invites", {
            "target_email": "maintainer@company.internal", "assigned_role": "Maintainer",
            "team_ids": [self.f.teams[0]]}, expected=201)
        password = secrets.token_urlsafe(24)
        self.h.Client(self.url).request("POST", "/api/v1/auth/invites/redeem", {
            "raw_token": invite["raw_token"], "username": "maintainer", "password": password}, expected=201)
        self.maintainer, _ = self.f.login(self.url, "maintainer", password)

    def worktree(self, task):
        path = self.f.repos[0] / "gitust-worktrees" / task["worktree_id"]
        self.require(path.is_dir(), "fixture managed checkout is missing")
        return path

    def origin(self, task):
        admin = Path(self.h.git(self.worktree(task), "rev-parse", "--absolute-git-dir"))
        return admin / "gitust-origin.json"

    def run(self, task, command, *, verify=False):
        result = self.admin.tool("gitust_sandbox_exec", {
            "worktree_id": task["worktree_id"], "command": command,
            "timeout_secs": 60, "verify": verify})
        self.require(result["exit_code"] == 0 and result["network_isolated"] is True,
                     "fixture command did not pass inside the OS sandbox")
        self.backends.add(result["backend"])
        return result

    def existing_task(self):
        task = self.admin.tool("gitust_worktree_spawn", {
            "branch": "agent/default-existing", "base_branch": "main", "agent": "codex",
            "prompt_summary": "Keep the original main approval target"})
        self.run(task, "printf 'existing main target\\n' > existing-target.txt")
        committed = self.admin.tool("gitust_worktree_commit", {
            "worktree_id": task["worktree_id"], "expected_head": self.main,
            "paths": ["existing-target.txt"], "message": "test: preserve the original task target"})
        sha = self.h.git(self.f.repos[0], "rev-parse", task["branch"])
        self.require(committed["commit"] == sha and committed["parent"] == self.main,
                     "MCP fixture commit differs from actual Git")
        result = self.run(task, "test -f existing-target.txt", verify=True)
        self.require(result["checked_commit"] == sha, "task verification is not bound to the exact commit")
        origin = self.origin(task).read_bytes()
        self.require(json.loads(origin)["base_branch"] == "main", "fixture has the wrong creation target")
        history = self.admin.request("GET", f"/api/v1/swarm/worktrees/{task['worktree_id']}/runs")
        self.require(bool(history), "fixture has no completed verification history")
        return task, sha, origin, history

    def change(self, expected, target, sha, *, client=None, route=None, status=200):
        return (client or self.admin).request("PATCH", route or self.route, {
            "expected_current_ref": "refs/heads/" + expected,
            "target_ref": "refs/heads/" + target,
            "expected_target_commit": sha}, expected=status)

    def mutation(self):
        task, checked, origin, history = self.existing_task()
        before = self.refs()
        self.secondary_before = self.git_state(self.f.repos[1])
        self.change("main", "develop", self.develop,
                    client=self.h.Client(self.url), status=401)
        self.change("main", "develop", self.develop, client=self.member, status=403)
        self.change("trunk", "develop", self.develop,
                    route=f"/api/v1/repos/{self.ids[1]}/default-branch", status=403)
        self.change("missing", "develop", self.develop, status=409)
        self.change("main", "develop", self.main, status=409)
        self.require(self.refs() == before and self.h.git(self.f.repos[0], "symbolic-ref", "HEAD")
                     == "refs/heads/main", "a denied mutation changed Git refs or HEAD")
        self.require(self.git_state(self.f.repos[1]) == self.secondary_before,
                     "a denied mutation changed the read-only repository's Git state")
        result = self.change("main", "develop", self.develop)
        self.require(result["id"] == self.ids[0] and result["branch"] == "develop"
                     and result["head_commit"] == self.develop, "mutation returned another repository or commit")
        self.require(self.refs() == before and self.h.git(self.f.repos[0], "symbolic-ref", "HEAD")
                     == "refs/heads/develop", "default change moved a branch/tag or failed to update symbolic HEAD")
        self.require(self.git_state(self.f.repos[1]) == self.secondary_before,
                     "primary default change modified the read-only repository's Git state")
        self.require([self.creation.anchors(repo) for repo in self.f.repos] == self.anchors,
                     "default change replaced a repository identity, registration or inode")
        self.require(self.origin(task).read_bytes() == origin and
                     self.admin.request("GET", f"/api/v1/swarm/worktrees/{task['worktree_id']}/runs") == history,
                     "default change modified existing creation metadata or completed history")
        rows = {row["id"]: row for row in self.admin.request("GET", "/api/v1/swarm/worktrees")}
        self.require(rows[task["worktree_id"]]["status"] == "verified"
                     and rows[task["worktree_id"]]["verified_commit"] == checked,
                     "default change retargeted or fabricated the existing verification")
        approved = self.admin.request("POST", f"/api/v1/swarm/worktrees/{task['worktree_id']}/approve",
                                      {"expected_commit": checked})
        self.require(approved["status"] == "merged" and
                     self.h.git(self.f.repos[0], "rev-parse", "main") == checked and
                     self.h.git(self.f.repos[0], "rev-parse", "develop") == self.develop,
                     "existing task approval did not retain its original main target")
        default_task = self.admin.tool("gitust_worktree_spawn", {
            "branch": "agent/default-new", "agent": "codex",
            "prompt_summary": "Resolve an omitted base from the current default"})
        record = json.loads(self.origin(default_task).read_bytes())
        self.require(record["base_branch"] == "develop" and record["base_commit"] == self.develop,
                     "new omitted-base task silently selected a different branch")
        self.require(self.h.git(self.worktree(default_task), "rev-parse", "HEAD") == self.develop
                     and (self.worktree(default_task) / "README.md").read_text() == "# Develop snapshot\n",
                     "the actual omitted-base checkout differs from the current default")
        self.existing, self.checked, self.default_task = task, checked, default_task
        self.require(self.h.git(self.f.repos[0], "cat-file", "-t", "refs/tags/preserved-tag") == "tag",
                     "default change peeled the annotated tag object")
        print("PASS actual default mutation: denied callers, symbolic HEAD, unchanged refs and original task target", flush=True)

    def secondary_mutation(self):
        route = f"/api/v1/repos/{self.ids[2]}/default-branch"
        before = [self.git_state(repo) for repo in self.f.repos]
        tasks = self.admin.request("GET", "/api/v1/swarm/worktrees")
        jobs = self.admin.request("GET", "/api/v1/repo/automatic-runs")
        origins = [self.origin(task).read_bytes() for task in [self.existing, self.default_task]]
        for client, status in [(self.h.Client(self.url), 401), (self.member, 403), (self.maintainer, 403)]:
            self.change("main", "release", self.secondary_release, client=client, route=route, status=status)
        self.change("develop", "release", self.secondary_release, route=route, status=409)
        self.change("main", "release", self.secondary_main, route=route, status=409)
        self.require([self.git_state(repo) for repo in self.f.repos] == before,
                     "denied or stale secondary default request changed a repository")
        catalog = {row["id"]: row for row in self.admin.request("GET", "/api/v1/repos")["repositories"]}
        self.require(catalog[self.ids[2]]["can_change_default_branch"] is True
                     and catalog[self.ids[2]]["capability"] == "browse"
                     and catalog[self.ids[2]]["is_default"] is False,
                     "writable secondary must support default management without becoming the primary")
        member_rows = self.maintainer.request("GET", "/api/v1/repos")["repositories"]
        self.require(all(row["can_change_default_branch"] is False for row in member_rows),
                     "ordinary push permission must not grant default management")
        result = self.change("main", "release", self.secondary_release, route=route)
        self.require(result == {"id": self.ids[2], "branch": "release", "head_commit": self.secondary_release},
                     "secondary default response selected the wrong repository or commit")
        after = [self.git_state(repo) for repo in self.f.repos]
        self.require(after[:2] == before[:2] and after[2] == ("refs/heads/release", before[2][1]),
                     "secondary default change must change only the selected symbolic HEAD")
        self.require([self.creation.anchors(repo) for repo in self.f.repos] == self.anchors
                     and self.admin.request("GET", "/api/v1/swarm/worktrees") == tasks
                     and self.admin.request("GET", "/api/v1/repo/automatic-runs") == jobs
                     and [self.origin(task).read_bytes() for task in [self.existing, self.default_task]] == origins,
                     "secondary default change altered identity, primary tasks, jobs or creation targets")
        self.secondary_after = after[2]
        print("PASS secondary default: explicit hosting capability, SiteAdmin authority and three-repository isolation", flush=True)

    def restart_and_clone(self):
        self.f.stop(self.process)
        self.start()
        catalog = {row["id"]: row for row in self.admin.request("GET", "/api/v1/repos")["repositories"]}
        primary = catalog[self.ids[0]]
        self.require(primary["branch"] == "develop" and primary["head_commit"] == self.develop
                     and primary["can_change_default_branch"] is True
                     and catalog[self.ids[1]]["can_change_default_branch"] is False
                     and catalog[self.ids[2]]["branch"] == "release"
                     and catalog[self.ids[2]]["head_commit"] == self.secondary_release
                     and catalog[self.ids[2]]["can_change_default_branch"] is True,
                     "restart lost default metadata or advertised unsupported management")
        self.require(self.git_state(self.f.repos[1]) == self.secondary_before,
                     "restart changed the read-only repository's Git state")
        url = self.url + f"/git/{self.ids[0]}.git"
        advertised = self.git_http(self.f.root, "ls-remote", "--symref", url, "HEAD")
        self.require("ref: refs/heads/develop\tHEAD" in advertised.splitlines()
                     and self.develop + "\tHEAD" in advertised.splitlines(),
                     "Git HTTP does not advertise the actual new default")
        clone = self.f.root / "new-clone"
        self.git_http(self.f.root, "clone", url, str(clone))
        self.require(self.h.git(clone, "symbolic-ref", "HEAD") == "refs/heads/develop"
                     and self.h.git(clone, "rev-parse", "HEAD") == self.develop
                     and (clone / "README.md").read_text() == "# Develop snapshot\n",
                     "a fresh real Git clone checked out the wrong branch or contents")
        self.git_http(self.client, "remote", "set-url", "origin", url)
        self.git_http(self.client, "fetch", "origin")
        self.require(self.h.git(self.client, "rev-parse", "origin/main") == self.checked
                     and self.h.git(self.client, "rev-parse", "origin/develop") == self.develop,
                     "existing clone fetch lost independent target branch updates")
        secondary_url = self.url + f"/git/{self.ids[2]}.git"
        advertised = self.git_http(self.f.root, "ls-remote", "--symref", secondary_url, "HEAD")
        self.require("ref: refs/heads/release\tHEAD" in advertised.splitlines()
                     and self.secondary_release + "\tHEAD" in advertised.splitlines(),
                     "secondary Git advertisement did not retain its new default")
        secondary_clone = self.f.root / "secondary-new-clone"
        self.git_http(self.f.root, "clone", secondary_url, str(secondary_clone))
        self.git_http(self.secondary_client, "remote", "set-url", "origin", secondary_url)
        self.git_http(self.secondary_client, "fetch", "origin")
        self.require(self.h.git(secondary_clone, "symbolic-ref", "HEAD") == "refs/heads/release"
                     and self.h.git(secondary_clone, "rev-parse", "HEAD") == self.secondary_release
                     and (secondary_clone / "README.md").read_text() == "# Secondary release\n"
                     and self.h.git(self.secondary_client, "rev-parse", "origin/main") == self.secondary_main
                     and self.h.git(self.secondary_client, "rev-parse", "origin/release") == self.secondary_release,
                     "secondary clone or fetch does not match its independent Git state")
        for repo in [self.f.repos[2], self.secondary_client, secondary_clone]:
            self.require(self.h.git(repo, "rev-parse", "refs/tags/secondary-tag") == self.secondary_tag
                         and self.h.git(repo, "cat-file", "-t", self.secondary_tag) == "tag",
                         "secondary default change or transport altered the annotated tag object")
        self.require(self.git_state(self.f.repos[2]) == self.secondary_after
                     and [self.creation.anchors(repo) for repo in self.f.repos] == self.anchors,
                     "restart changed secondary Git state or registered identities")
        for repo in [*self.f.repos, self.client, clone, self.secondary_client, secondary_clone,
                     self.worktree(self.existing), self.worktree(self.default_task)]:
            self.h.git(repo, "fsck", "--full", "--strict")
        self.f.stop(self.process)
        print("PASS restart and real Git HTTP: symref advertisement, fresh clone, fetch and strict fsck", flush=True)

    def execute(self):
        self.setup()
        self.mutation()
        self.secondary_mutation()
        self.restart_and_clone()
        return {"version": 2, "passed": True, "actual_repositories": 3,
                "default_head_only_changed": True, "registered_identity_preserved": True,
                "denied_and_stale_requests_unchanged": True, "existing_task_target_preserved": True,
                "read_only_git_state_preserved": True,
                "omitted_base_uses_current_default": True, "restart_default_preserved": True,
                "git_http_advertises_default": True, "fresh_clone_uses_default": True,
                "existing_clone_fetch_matches_git": True, "git_fsck": True,
                "secondary_head_only_changed": True, "secondary_requires_site_admin": True,
                "secondary_clone_and_fetch_match_git": True, "secondary_annotated_tag_preserved": True,
                "external_github_publication": False, "platform": os.uname().sysname,
                "sandbox_backends": sorted(self.backends)}


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
    args = parser.parse_args()
    script, source, binary = Path(__file__).resolve(), args.source.resolve(strict=True), args.binary.resolve(strict=True)
    if args.report.exists() or args.report.is_symlink():
        raise RuntimeError("report already exists; choose a new path")
    helper, multi, creation = load("alpha-rehearsal"), load("multi-repository-rehearsal"), load("empty-repository-rehearsal")
    runtime_paths = [script, script.with_name("alpha-rehearsal.py"),
                     script.with_name("multi-repository-rehearsal.py"), script.with_name("empty-repository-rehearsal.py")]
    runtime = {"scripts/" + path.name: helper.sha256(path) for path in runtime_paths}
    commit, dirty = provenance(helper, source, runtime)
    binary_sha = helper.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-default-branch-") as directory:
        scratch = Path(directory).resolve()
        fixture = multi.Rehearsal(helper, binary, scratch)
        try:
            report = Rehearsal(helper, fixture, creation).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            helper.require(all(process.poll() is not None for process in fixture.processes),
                           "owned fixture service was not reaped")
    helper.require(not scratch.exists(), "private fixture was not removed")
    helper.require(runtime == {"scripts/" + path.name: helper.sha256(path) for path in runtime_paths}
                   and helper.sha256(binary) == binary_sha, "runtime source or binary changed")
    final_commit, final_dirty = provenance(helper, source, runtime)
    helper.require(final_commit == commit, "provenance source commit changed")
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or final_dirty,
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "owned_services_reaped": True, "private_fixture_removed": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Default branch management passed; owned services and private fixtures removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Default-branch rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
