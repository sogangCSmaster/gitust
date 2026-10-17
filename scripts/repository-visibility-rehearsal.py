#!/usr/bin/env python3
"""Exercise registered access-scope changes and a restored two-repository installation."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


backup = load("multi-repository-backup-rehearsal")


class Rehearsal(backup.Rehearsal):
    def patch(self, admin, index, expected, value, status=200):
        result = admin.request("PATCH", f"/api/v1/repos/{self.f.ids[index]}/visibility",
                               {"expected_is_private": expected, "is_private": value}, expected=status)
        if status == 200:
            self.require(result == {"id": self.f.ids[index], "is_private": value},
                         "visibility response does not identify the actual target and value")
        return result

    def access_catalog(self, client, private, allowed):
        rows = client.request("GET", "/api/v1/repos")["repositories"]
        self.require({row["id"] for row in rows} == {self.f.ids[i] for i in allowed},
                     "catalog does not enforce the current access scope")
        for row in rows:
            index = self.f.ids.index(row["id"])
            self.require(row.get("is_private") is private[index],
                         "catalog privacy differs from the committed access scope")
        return {row["id"]: row for row in rows}

    def read(self, client, repo, index, allowed):
        prefix = f"/api/v1/repos/{self.f.ids[index]}"
        if not allowed:
            client.request("GET", prefix + "/tree", expected=403)
            return
        sha = self.h.git(repo, "rev-parse", "HEAD")
        result = client.request("GET", prefix + "/blob?rev=" + sha + "&path=marker.txt")
        self.require(result["commit_sha"] == sha
                     and result["content"] == self.h.git(repo, "show", sha + ":marker.txt") + "\n"
                     and result["oid"] == self.h.git(repo, "rev-parse", sha + ":marker.txt"),
                     "authorized blob differs from independent native Git")

    def acl(self, url, private, repos):
        anonymous = self.h.Client(url)
        anonymous.request("GET", "/api/v1/repos", expected=401)
        for index in range(2):
            anonymous.request("GET", f"/api/v1/repos/{self.f.ids[index]}/tree", expected=401)
        for member_index, (member, _) in enumerate(self.f.members):
            member.url = url
            allowed = [i for i in range(2) if not private[i] or member_index == i]
            rows = self.access_catalog(member, private, allowed)
            self.require(all(row.get("can_change_visibility") is False for row in rows.values()),
                         "a non-administrator received a visibility management hint")
            for index in range(2):
                self.read(member, repos[index], index, index in allowed)

    def unchanged(self):
        self.require([self.git_state(repo) for repo in self.f.repos] == self.states,
                     "visibility mutation changed Git refs or symbolic HEAD")
        self.require(self.records(self.f.repos[0], self.tasks[:2], self.tasks[:1]) == self.durable[0]
                     and self.records(self.f.repos[1], self.tasks[2:]) == self.durable[1],
                     "visibility mutation changed identity, registration or task records")

    def mutation(self):
        process, admin, _ = self.start(self.f.repos[0], [self.f.repos[1]])
        url = admin.url
        route = f"/api/v1/repos/{self.f.ids[0]}/visibility"
        body = {"expected_is_private": True, "is_private": False}
        self.h.Client(url).request("PATCH", route, body, expected=401)
        for member, _ in self.f.members:
            member.url = url
            member.request("PATCH", route, body, expected=403)
        rows = self.access_catalog(admin, [True, True], [0, 1])
        self.require(all(row.get("can_change_visibility") is True for row in rows.values()),
                     "configured registered administrator contexts lack the management hint")
        self.acl(url, [True, True], self.f.repos)
        admin.request("PATCH", route, {**body, "extra": True}, expected=400)
        admin.request("PATCH", route, {"expected_is_private": "true", "is_private": False}, expected=400)
        admin.request("PATCH", route, {**body, "extra": "x" * 4096}, expected=413)
        admin.request("PATCH", f"/api/v1/repos/{self.f.unhosted_registered_id}/visibility",
                      body, expected=404)
        self.patch(admin, 0, False, False, 409)
        self.unchanged()

        self.patch(admin, 0, True, False)
        self.patch(admin, 0, True, True, 409)
        self.patch(admin, 0, False, False)
        self.acl(url, [False, True], self.f.repos)
        outsider = self.f.members[1][0]
        clone = self.scratch / "outside-team-clone"
        transport = {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.extraHeader",
                     "GIT_CONFIG_VALUE_0": "Authorization: Bearer " + outsider.token,
                     "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"}
        remote = url + f"/git/{self.f.ids[0]}.git"
        self.h.git(self.scratch, "clone", remote, str(clone), env=transport)
        self.require(self.h.git(clone, "rev-parse", "HEAD") == self.h.git(self.f.repos[0], "rev-parse", "HEAD"),
                     "outside-team clone resolved another commit")

        self.patch(admin, 0, False, True)
        self.acl(url, [True, True], self.f.repos)
        outsider.request("GET", f"/git/{self.f.ids[0]}.git/info/refs?service=git-upload-pack", expected=403)
        self.h.command(["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
                        "-C", str(clone), "fetch", "origin"], env=transport, expected=128)
        self.patch(admin, 1, True, False)
        self.acl(url, [True, False], self.f.repos)
        self.patch(admin, 1, False, True)
        self.patch(admin, 0, True, False)
        self.acl(url, [False, True], self.f.repos)
        self.h.git(clone, "fetch", "origin", env=transport)
        self.h.git(clone, "fsck", "--full", "--strict")
        self.unchanged()
        self.catalog = self.access_catalog(admin, [False, True], [0, 1])
        self.f.stop(process)
        self.auth_bytes = self.f.auth.read_bytes()
        print("PASS live visibility: strict CAS, current ACL, two teams, anonymous denial and real Git clone/fetch", flush=True)

    def verify_restored(self, auth, original_paths):
        process, admin, operator_id = self.start(self.restored[0], [self.restored[1]], auth=auth)
        self.require(operator_id == self.operator_id and all(not path.exists() for path in original_paths),
                     "restored visibility service depends on original installation paths")
        self.require(self.catalog_values(admin) == self.catalog,
                     "restart/restore changed repository identity, privacy or capability hints")
        self.acl(admin.url, [False, True], self.restored)
        rows = {row["id"]: row for row in admin.request("GET", "/api/v1/swarm/worktrees")}
        self.require(rows[self.tasks[0]["worktree_id"]]["status"] == "merged",
                     "restored human approval is missing")
        pending = rows[self.tasks[1]["worktree_id"]]
        self.require(pending["status"] == "pending" and pending["verified_commit"] is None
                     and pending["verified_checks_oid"] is None,
                     "restored diagnostic history granted verification authority")
        route = "/api/v1/swarm/worktrees/" + self.tasks[1]["worktree_id"]
        self.require(admin.request("GET", route + "/runs") == self.history,
                     "visibility restore changed retained execution history")
        admin.request("POST", route + "/approve", {"expected_commit": self.pending_sha}, expected=409)
        self.require([self.git_state(repo) for repo in self.restored] == self.states,
                     "denied restored approval changed refs")
        self.f.stop(process)
        print("PASS restored-only visibility: persisted scope, current team reads and fresh-verification requirement", flush=True)

    @staticmethod
    def catalog_values(client):
        return {row["id"]: row for row in client.request("GET", "/api/v1/repos")["repositories"]}

    def execute(self):
        self.prepare()
        self.mutation()
        auth, original_paths = self.restore()
        self.verify_restored(auth, original_paths)
        return {"version": 1, "passed": True, "actual_repositories": 2,
                "strict_current_value_cas": True, "role_and_anonymous_denials": True,
                "current_team_and_signed_in_scope": True, "primary_and_secondary_metadata": True,
                "outside_team_git_clone_fetch_and_revoke": True, "refs_and_task_records_preserved": True,
                "restored_scope_and_access": True, "all_original_paths_unavailable": True,
                "pending_requires_fresh_verification": True, "git_fsck": True,
                "external_github_publication": False, "platform": os.uname().sysname,
                "sandbox_backends": sorted(self.backends)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    script, source, binary = Path(__file__).resolve(), args.source.resolve(strict=True), args.binary.resolve(strict=True)
    if args.report.exists() or args.report.is_symlink():
        raise RuntimeError("report already exists; choose a new path")
    helper, multi = load("alpha-rehearsal"), load("multi-repository-rehearsal")
    backup_tool = script.with_name("backup.py")
    runtime_paths = [script, script.with_name("alpha-rehearsal.py"),
                     script.with_name("multi-repository-rehearsal.py"),
                     script.with_name("multi-repository-backup-rehearsal.py"), backup_tool]
    runtime_paths.extend(sorted(script.with_name("backup_lib").glob("*.py")))
    runtime = {str(path.relative_to(script.parent.parent)): helper.sha256(path) for path in runtime_paths}
    commit, dirty = backup.provenance(helper, source, runtime)
    binary_sha = helper.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-visibility-") as directory:
        scratch = Path(directory).resolve()
        installation = scratch / "installation"
        installation.mkdir()
        fixture = multi.Rehearsal(helper, binary, installation)
        try:
            report = Rehearsal(helper, fixture, backup_tool, scratch).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            helper.require(all(process.poll() is not None for process in fixture.processes),
                           "owned visibility fixture service was not reaped")
    helper.require(not scratch.exists(), "private visibility fixture was not removed")
    helper.require(runtime == {str(path.relative_to(script.parent.parent)): helper.sha256(path)
                               for path in runtime_paths} and helper.sha256(binary) == binary_sha,
                   "runtime source or binary changed during rehearsal")
    final_commit, final_dirty = backup.provenance(helper, source, runtime)
    helper.require(final_commit == commit, "provenance source commit changed")
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or final_dirty,
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "owned_services_reaped": True, "private_fixture_removed": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Repository visibility passed; owned services and private fixtures removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Repository visibility rehearsal failed:",
              str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
