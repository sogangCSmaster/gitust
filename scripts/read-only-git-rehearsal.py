#!/usr/bin/env python3
"""Clone and fetch two distinct repositories while refusing all secondary pushes."""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Rehearsal:
    def __init__(self, helper, fixture):
        self.h, self.f = helper, fixture

    def require(self, condition, message):
        self.h.require(condition, message)

    def state(self, repo):
        return (self.h.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)"),
                self.h.git(repo, "symbolic-ref", "HEAD"))

    def anchors(self, repo):
        common = Path(self.h.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
        return [(common / "gitust-repository" / name).read_bytes()
                for name in ["identity.json", "registration.json"]]

    @staticmethod
    def environment(header):
        return {"GIT_CONFIG_COUNT": "2", "GIT_CONFIG_KEY_0": "http.extraHeader",
                "GIT_CONFIG_VALUE_0": "Authorization: " + header,
                "GIT_CONFIG_KEY_1": "credential.helper", "GIT_CONFIG_VALUE_1": "",
                "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"}

    def url(self, index):
        return self.f.url + f"/git/{self.f.ids[index]}.git"

    def clone(self, index, name, env):
        path = self.f.root / name
        self.h.git(self.f.root, "clone", self.url(index), str(path), env=env)
        self.require(self.h.git(path, "rev-parse", "HEAD")
                     == self.h.git(self.f.repos[index], "rev-parse", "HEAD"),
                     "clone resolved a different repository or default commit")
        self.require(self.h.git(path, "show", "HEAD:marker.txt") == f"repository-{index}-after",
                     "clone contains another repository's marker")
        return path

    def expect_git_failure(self, clone, env, *args):
        self.h.command(["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
                        "-C", str(clone), *args], env=env, expected=128)

    def acl(self, private):
        route = f"/git/{self.f.ids[1]}.git/info/refs?service=git-upload-pack"
        self.h.Client(self.f.url).request("GET", route, expected=401)
        for index, (client, _) in enumerate(self.f.members):
            client.url = self.f.url
            client.request("GET", route, expected=403 if private and index != 1 else 200)

    def execute(self):
        self.f.setup()
        # This first request is deliberately absent on the previous binary.
        route = f"/git/{self.f.ids[1]}.git/info/refs"
        self.f.admin.request("GET", route + "?service=git-upload-pack")
        states = [self.state(repo) for repo in self.f.repos]
        anchors = [self.anchors(repo) for repo in self.f.repos]
        catalog = self.f.admin.request("GET", "/api/v1/repos")["repositories"]
        self.require(all(row.get("can_clone") is True for row in catalog)
                     and len(catalog) == 2, "hosted clone hints do not match actual transport")
        self.require(self.h.git(self.f.repos[0], "rev-parse", "HEAD")
                     != self.h.git(self.f.repos[1], "rev-parse", "HEAD"),
                     "fixture must distinguish primary and secondary object identities")
        self.acl(True)
        self.f.admin.request("GET", f"/git/{self.f.unhosted_registered_id}.git/info/refs?service=git-upload-pack",
                             expected=404)
        self.f.admin.request("GET", "/git/00000000-0000-0000-0000-000000000001.git/info/refs?service=git-upload-pack",
                             expected=404)
        owner = self.f.members[1][0]
        issued = owner.request("POST", "/api/v1/auth/pats", {"name": "secondary-clone"}, expected=201)
        basic = "Basic " + base64.b64encode(("member-1:" + issued["raw_token"]).encode()).decode()
        member_env = self.environment(basic)
        admin_env = self.environment("Bearer " + self.f.admin.token)
        secondary = self.clone(1, "member-secondary-clone", member_env)
        primary = self.clone(0, "admin-primary-clone", admin_env)
        tag = self.h.git(self.f.repos[1], "rev-parse", "refs/tags/import-proof")
        self.require(self.h.git(secondary, "rev-parse", "refs/tags/import-proof") == tag
                     and self.h.git(secondary, "cat-file", "-t", tag) == "tag",
                     "secondary clone peeled or omitted the annotated tag")
        for actor in [self.f.admin, owner]:
            actor.request("GET", route + "?service=git-receive-pack", expected=404)
            actor.request("POST", f"/git/{self.f.ids[1]}.git/git-receive-pack", {}, expected=404)
        for env in [member_env, admin_env]:
            self.expect_git_failure(secondary, env, "push", "origin", "HEAD:refs/heads/forbidden-secondary")
        self.require([self.state(repo) for repo in self.f.repos] == states,
                     "clone or denied secondary push changed Git refs")
        print("PASS real primary/session and secondary/Basic-PAT clone, annotated tags and all-role secondary push denial", flush=True)

        visibility = f"/api/v1/repos/{self.f.ids[1]}/visibility"
        self.f.admin.request("PATCH", visibility, {"expected_is_private": True, "is_private": False})
        self.acl(False)
        no_team = self.environment("Bearer " + self.f.members[2][0].token)
        outsider = self.clone(1, "no-team-secondary-clone", no_team)
        self.f.admin.request("PATCH", visibility, {"expected_is_private": False, "is_private": True})
        self.acl(True)
        self.expect_git_failure(outsider, no_team, "fetch", "origin")
        self.require([self.state(repo) for repo in self.f.repos] == states,
                     "access-scope change modified refs")
        print("PASS current private/shared ACL and outside-team clone/fetch revocation; anonymous access stays denied", flush=True)

        self.f.stop(self.f.process)
        seed = self.f.root / "secondary-seed"
        (seed / "next.txt").write_text("offline secondary update\n")
        self.h.git(seed, "add", "next.txt")
        self.h.git(seed, "commit", "-m", "test: offline secondary update")
        self.h.git(seed, "tag", "-a", "fetch-proof", "-m", "Keep the fetched tag object")
        self.h.git(self.f.repos[1], "fetch", "--no-tags", str(seed),
                   "refs/heads/main:refs/heads/main", "refs/tags/fetch-proof:refs/tags/fetch-proof")
        updated = self.h.git(self.f.repos[1], "rev-parse", "HEAD")
        new_tag = self.h.git(self.f.repos[1], "rev-parse", "refs/tags/fetch-proof")
        self.require(self.state(self.f.repos[0]) == states[0]
                     and updated != self.h.git(secondary, "rev-parse", "origin/main"),
                     "offline fixture update did not distinguish the next fetch or changed primary")
        updated_state = self.state(self.f.repos[1])
        self.f.process, self.f.url = self.f.launch(self.f.repos[0], [self.f.repos[1]])
        self.f.healthy(self.f.process, self.f.url)
        self.f.admin.url = self.f.url
        for clone, index in [(primary, 0), (secondary, 1), (outsider, 1)]:
            self.h.git(clone, "remote", "set-url", "origin", self.url(index))
        self.h.git(secondary, "fetch", "--tags", "origin", env=member_env)
        self.h.git(primary, "fetch", "origin", env=admin_env)
        self.require(self.h.git(secondary, "rev-parse", "origin/main") == updated
                     and self.h.git(secondary, "rev-parse", "refs/tags/import-proof") == tag
                     and self.h.git(secondary, "rev-parse", "refs/tags/fetch-proof") == new_tag
                     and self.h.git(secondary, "cat-file", "-t", new_tag) == "tag"
                     and self.h.git(secondary, "show", "origin/main:next.txt") == "offline secondary update",
                     "incremental secondary fetch or tag identity differs from native Git")
        self.require(self.h.git(primary, "rev-parse", "origin/main")
                     == self.h.git(self.f.repos[0], "rev-parse", "HEAD"), "primary fetch changed routing")
        self.acl(True)
        self.expect_git_failure(outsider, no_team, "fetch", "origin")
        self.require([self.anchors(repo) for repo in self.f.repos] == anchors,
                     "restart or reads changed registered identities/bindings")
        owner.url = self.f.url
        owner.request("DELETE", "/api/v1/auth/pats/" + issued["metadata"]["id"])
        self.h.Client(self.f.url, issued["raw_token"]).request("GET", route + "?service=git-upload-pack",
                                                             expected=401)
        self.expect_git_failure(secondary, member_env, "fetch", "origin")
        fresh = owner.request("POST", "/api/v1/auth/pats", {"name": "inactive-read"}, expected=201)
        self.f.admin.request("POST", f"/api/v1/admin/users/{self.f.members[1][1]}/deactivate", {})
        self.h.Client(self.f.url, fresh["raw_token"]).request("GET", route + "?service=git-upload-pack",
                                                           expected=403)
        self.require(self.state(self.f.repos[0]) == states[0]
                     and self.state(self.f.repos[1]) == updated_state,
                     "fetch or credential denial changed server refs")
        for repo in [*self.f.repos, primary, secondary, outsider]:
            self.h.git(repo, "fsck", "--full", "--strict")
        print("PASS restart/offline update fetch, stable UUID/tag identity, revoked/deactivated credentials and strict fsck", flush=True)
        return {"version": 1, "passed": True, "actual_repositories": 2,
                "primary_and_secondary_clone_match_git": True, "session_and_basic_pat_auth": True,
                "secondary_push_denied": True, "annotated_tag_objects_preserved": True,
                "current_private_and_shared_acl": True, "outside_team_revoked_fetch_denied": True,
                "unknown_or_unconfigured_denied": True, "offline_update_fetch_and_restart": True,
                "registered_identity_preserved": True, "primary_refs_unchanged": True,
                "revoked_and_deactivated_denied": True, "git_fsck": True,
                "external_github_publication": False, "platform": os.uname().sysname}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    helper, multi = load("alpha-rehearsal"), load("multi-repository-rehearsal")
    script, source, binary = Path(__file__).resolve(), args.source.resolve(strict=True), args.binary.resolve(strict=True)
    if args.report.exists() or args.report.is_symlink():
        raise RuntimeError("report already exists; choose a new path")
    runtime_paths = [script, script.with_name("alpha-rehearsal.py"), script.with_name("multi-repository-rehearsal.py")]
    runtime = {"scripts/" + path.name: helper.sha256(path) for path in runtime_paths}
    commit, dirty = helper.git(source, "rev-parse", "HEAD"), bool(helper.git(source, "status", "--porcelain"))
    binary_sha = helper.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-read-only-git-") as directory:
        scratch = Path(directory).resolve()
        fixture = multi.Rehearsal(helper, binary, scratch)
        try:
            report = Rehearsal(helper, fixture).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            helper.require(all(process.poll() is not None for process in fixture.processes),
                           "owned read-only Git fixture service was not reaped")
    helper.require(not scratch.exists(), "private Git transport fixture was not removed")
    helper.require(runtime == {"scripts/" + path.name: helper.sha256(path) for path in runtime_paths}
                   and helper.sha256(binary) == binary_sha, "rehearsal source or binary changed")
    helper.require(helper.git(source, "rev-parse", "HEAD") == commit, "provenance source commit changed")
    for name, digest in runtime.items():
        result = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                                env=helper.clean_environment(), capture_output=True, timeout=30)
        dirty |= result.returncode != 0 or hashlib.sha256(result.stdout).hexdigest() != digest
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or bool(helper.git(source, "status", "--porcelain")),
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "owned_services_reaped": True, "private_fixture_removed": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Read-only Git transport passed; owned services and private fixtures removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Read-only Git rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
