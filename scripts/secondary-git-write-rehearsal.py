#!/usr/bin/env python3
"""Round-trip ordinary Git on an opted-in bare secondary; preserve the other two repos."""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile

def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"),
                                                 Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Rehearsal:
    def __init__(self, helper, fixture, readonly):
        self.h, self.f = helper, fixture
        self.read = readonly.Rehearsal(helper, fixture)
        self.clients, self.clones = [], []
        self.restart_display_fields = set()

    def require(self, condition, message):
        self.h.require(condition, message)

    def launch(self, *, writable=True):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        args = [str(self.f.binary), "serve", "--bind", f"127.0.0.1:{port}",
                "--repo", str(self.f.repos[0]), "--auth-db", str(self.f.auth),
                "--sync-github", str(self.f.root / "unused-upstream.git"),
                "--read-only-repo", str(self.f.repos[1]),
                "--git-write-repo" if writable else "--read-only-repo", str(self.f.repos[2])]
        process = subprocess.Popen(args, env=self.h.clean_environment(),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.f.processes.append(process)
        self.f.process, self.f.url = process, f"http://127.0.0.1:{port}"
        self.f.healthy(process, self.f.url)
        for client in self.clients:
            client.url = self.f.url
        for clone, index in self.clones:
            self.h.git(clone, "remote", "set-url", "origin", self.read.url(index))

    def setup(self):
        self.f.repos.append(self.f.root / "git-write.git")
        self.h.command([str(self.f.binary), "admin", "create", "--username", "operator",
                        "--email", "operator@company.internal", "--auth-db", str(self.f.auth)],
                       env={"GITUST_ADMIN_PASSWORD": self.f.password})
        for index, repo in enumerate(self.f.repos):
            seed = repo if index == 0 else self.f.root / f"seed-{index}"
            seed.mkdir()
            self.h.git(seed, "init", "--initial-branch=main")
            self.h.git(seed, "config", "user.name", "Gitust Secondary Hosting Rehearsal")
            self.h.git(seed, "config", "user.email", "secondary@company.internal")
            (seed / "marker.txt").write_text(f"repository-{index}-after\n")
            self.h.git(seed, "add", "marker.txt")
            self.h.git(seed, "commit", "-m", f"test: distinct repository {index}")
            self.h.git(seed, "tag", "-a", "preexisting", "-m", f"Keep repository {index} tag object")
            if index:
                self.h.git(self.f.root, "clone", "--bare", "--no-local", str(seed), str(repo))
            self.f.register_repository(repo, f"repository-{index}", self.f.teams[[0, 1, 0][index]])
        self.launch()
        self.f.admin, _ = self.f.login(self.f.url, "operator", self.f.password)
        self.clients.append(self.f.admin)
        rows = self.f.admin.request("GET", "/api/v1/repos")["repositories"]
        by_name = {row["name"]: row for row in rows}
        self.require(len(rows) == len(by_name) == 3, "catalog must contain exactly three repositories")
        self.f.ids = [by_name[f"repository-{index}"]["id"] for index in range(3)]
        self.require(len(set(self.f.ids)) == 3
                     and len({self.h.git(repo, "rev-parse", "HEAD") for repo in self.f.repos}) == 3,
                     "fixture must distinguish all repository identities and contents")
        self.maintainer, self.maintainer_id = self.account("maintainer", "Maintainer", [self.f.teams[0]])
        self.outsider, _ = self.account("outsider", "Maintainer", [self.f.teams[1]])
        self.developer, _ = self.account("developer", "Developer", [self.f.teams[0]])
        self.inactive, self.inactive_id = self.account("inactive", "Maintainer", [self.f.teams[0]])

    def account(self, username, role, teams):
        password = secrets.token_urlsafe(24)
        issued = self.f.admin.request("POST", "/api/v1/admin/invites", {
            "target_email": username + "@company.internal", "assigned_role": role,
            "team_ids": teams}, expected=201)
        self.h.Client(self.f.url).request("POST", "/api/v1/auth/invites/redeem", {
            "raw_token": issued["raw_token"], "username": username, "password": password}, expected=201)
        client, user = self.f.login(self.f.url, username, password)
        self.clients.append(client)
        if username == "maintainer":
            self.maintainer_password = password
        return client, user

    def pat(self, client, name):
        return client.request("POST", "/api/v1/auth/pats", {"name": name}, expected=201)

    def basic(self, username, token):
        return self.read.environment("Basic " + base64.b64encode((username + ":" + token).encode()).decode())

    def bearer(self, token):
        return self.read.environment("Bearer " + token)

    def clone(self, index, name, env):
        clone = self.read.clone(index, name, env)
        self.clones.append((clone, index))
        self.h.git(clone, "config", "user.name", "Gitust Secondary Writer")
        self.h.git(clone, "config", "user.email", "writer@company.internal")
        return clone

    def denied_push(self, clone, env, *refspecs):
        before = [self.read.state(repo) for repo in self.f.repos]
        result = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
                                 "-C", str(clone), "push", "origin", *refspecs],
                                env=self.h.clean_environment(env), capture_output=True, timeout=120)
        self.require(result.returncode in (1, 128), "a forbidden native Git push did not fail normally")
        self.require([self.read.state(repo) for repo in self.f.repos] == before,
                     "a denied push changed a repository ref or symbolic HEAD")

    def receive_denied(self, client, index, status):
        route = f"/git/{self.f.ids[index]}.git/"
        client.request("GET", route + "info/refs?service=git-receive-pack", expected=status)
        client.request("POST", route + "git-receive-pack", {}, expected=status)

    def catalog(self, client, writable):
        rows = client.request("GET", "/api/v1/repos")["repositories"]
        expected_ids = set(self.f.ids if client is self.f.admin else [self.f.ids[0], self.f.ids[2]])
        self.require({row["id"] for row in rows} == expected_ids, "current catalog selected another group")
        for row in rows:
            index = self.f.ids.index(row["id"])
            expected = (index == 0 or index == 2 and writable) and client is not self.developer
            self.require(row.get("can_clone") is True and row.get("can_push") is expected
                         and row["is_default"] is (index == 0)
                         and row["capability"] == ("workbench" if index == 0 else "browse"),
                         "catalog push hint or workbench capability differs from current authority")
        self.require(str(self.f.root) not in json.dumps(rows)
                     and "registration_nonce" not in json.dumps(rows), "catalog exposed private metadata")

    def primary_state(self):
        primary = self.f.repos[0]
        return (self.read.state(primary), self.h.git(primary, "status", "--porcelain"),
                self.h.git(primary, "worktree", "list", "--porcelain"),
                self.f.admin.request("GET", "/api/v1/swarm/worktrees"),
                self.f.admin.request("GET", "/api/v1/repo/automatic-runs"))

    def recovered_primary(self, before):
        after = self.primary_state()
        self.require(after[:3] == before[:3] and after[4] == before[4],
                     "restart changed primary Git refs/status/worktree administration or automatic jobs")
        self.require(len(before[3]) == len(after[3]) == 1,
                     "restart lost or created a managed primary worktree")
        old, new = before[3][0], after[3][0]
        self.require(old.keys() == new.keys(), "restart changed the worktree response schema")
        changed = {key for key in old if old[key] != new[key]}
        # Existing recovery reconstructs these display measurements; authority fields remain exact.
        self.require(changed <= {"created_at", "materialize_micros"}
                     and new["materialize_micros"] == 0
                     and isinstance(new["created_at"], str),
                     "restart changed primary worktree fields: " + ", ".join(sorted(changed)))
        self.restart_display_fields.update(changed)
        print("PASS primary restart changed only reconstructed display fields: " + ", ".join(sorted(changed)), flush=True)
        return after

    def mcp_token(self):
        verifier = secrets.token_urlsafe(48)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
        redirect = "http://127.0.0.1:49151/callback"
        client = self.h.Client(self.f.url)
        registered = client.request("POST", "/oauth/register", {
            "client_name": "Isolated secondary Git rehearsal", "redirect_uris": [redirect]}, expected=201)
        issued = self.f.admin.request("POST", "/oauth/authorize", {
            "client_id": registered["client_id"], "redirect_uri": redirect, "response_type": "code",
            "code_challenge": challenge, "code_challenge_method": "S256", "scope": "mcp"})
        exchanged = client.request("POST", "/oauth/token", {
            "grant_type": "authorization_code", "client_id": registered["client_id"],
            "redirect_uri": redirect, "code": issued["code"], "code_verifier": verifier})
        self.require(exchanged["scope"] == "mcp" and exchanged["token_type"] == "Bearer",
                     "OAuth exchange did not issue an actual MCP-purpose credential")
        mcp = self.h.Client(self.f.url, exchanged["access_token"])
        mcp.rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                               "clientInfo": {"name": "secondary-rehearsal", "version": "1"}})
        return mcp

    def push(self, clone, env, name):
        (clone / "ordinary.txt").write_text(name + "\n")
        self.h.git(clone, "add", "ordinary.txt")
        self.h.git(clone, "commit", "-m", "test: " + name)
        branch, tag = "refs/heads/" + name, "refs/tags/" + name
        self.h.git(clone, "tag", "-a", name, "-m", "Preserve annotated " + name)
        head, raw_tag = self.h.git(clone, "rev-parse", "HEAD"), self.h.git(clone, "rev-parse", tag)
        self.h.git(clone, "push", "origin", "HEAD:" + branch, tag + ":" + tag, env=env)
        server = self.f.repos[2]
        self.require(self.h.git(server, "rev-parse", branch) == head
                     and self.h.git(server, "rev-parse", tag) == raw_tag
                     and self.h.git(server, "cat-file", "-t", raw_tag) == "tag"
                     and self.h.git(server, "show", branch + ":ordinary.txt") == name,
                     "successful push differs from independent Git commit, content or raw tag oracle")
        return branch, tag, head, raw_tag

    def execute(self):
        self.setup()
        self.f.admin.request("POST", "/api/v1/swarm/worktrees", {
            "repo_id": self.f.ids[0], "branch": "agent/primary-preserved", "base_branch": "main",
            "prompt_summary": "Must survive secondary ordinary Git hosting"})
        primary = self.primary_state()
        self.require(len(primary[3]) == 1, "primary preservation fixture must contain an actual managed worktree")
        readonly = self.read.state(self.f.repos[1])
        anchors = [self.read.anchors(repo) for repo in self.f.repos]
        preexisting = [self.h.git(repo, "rev-parse", "refs/tags/preexisting") for repo in self.f.repos]
        for client in [self.f.admin, self.maintainer, self.developer]:
            self.catalog(client, True)
        admin_pat = self.pat(self.f.admin, "secondary-admin")
        owner_pat = self.pat(self.maintainer, "secondary-maintainer")
        actors = [("admin-session", self.bearer(self.f.admin.token)),
                  ("admin-pat", self.basic("operator", admin_pat["raw_token"])),
                  ("maintainer-session", self.bearer(self.maintainer.token)),
                  ("maintainer-pat", self.basic("maintainer", owner_pat["raw_token"]))]
        proofs = []
        for name, env in actors:
            clone = self.clone(2, name, env)
            proofs.append(self.push(clone, env, name))
        observer = self.clone(2, "independent-reader", self.bearer(self.developer.token))
        self.h.git(observer, "fetch", "--tags", "origin", env=self.bearer(self.developer.token))
        for branch, tag, head, raw_tag in proofs:
            self.require(self.h.git(observer, "rev-parse", "refs/remotes/origin/" + branch.removeprefix("refs/heads/")) == head
                         and self.h.git(observer, "rev-parse", tag) == raw_tag
                         and self.h.git(observer, "cat-file", "-t", raw_tag) == "tag",
                         "independent native fetch differs from the pushed branch or annotated tag")
        primary_clone = self.clone(0, "primary-reader", actors[0][1])
        readonly_clone = self.clone(1, "readonly-reader", actors[1][1])
        print("PASS admin and Maintainer sessions/Basic-PAT: four real clone/commit/push/fetch paths with raw annotated-tag oracles", flush=True)

        for client in [self.f.admin, self.maintainer]:
            self.receive_denied(client, 1, 404)
        self.denied_push(readonly_clone, actors[0][1], "HEAD:refs/heads/forbidden-readonly")
        self.receive_denied(self.h.Client(self.f.url), 2, 401)
        for client in [self.developer, self.outsider]:
            self.receive_denied(client, 2, 403)
            self.denied_push(observer, self.bearer(client.token), "HEAD:refs/heads/forbidden-role")
        self.f.admin.request("GET", "/git/00000000-0000-0000-0000-000000000001.git/info/refs?service=git-receive-pack", expected=404)
        self.f.admin.request("POST", "/api/v1/swarm/worktrees", {
            "repo_id": self.f.ids[2], "branch": "agent/forbidden-secondary", "base_branch": "main",
            "prompt_summary": "Must stay primary-only"}, expected=404)
        mcp = self.mcp_token()
        self.receive_denied(mcp, 2, 401)
        self.denied_push(observer, self.bearer(mcp.token), "HEAD:refs/heads/forbidden-mcp")
        # These are ordinary native Git pushes, without custom wire/race/exploit machinery.
        self.denied_push(observer, actors[0][1], "HEAD:refs/heads/agent/forbidden")
        self.denied_push(observer, actors[3][1], "HEAD:refs/heads/mixed-forbidden", "HEAD:refs/heads/agent/mixed-forbidden")
        revoked, _ = self.f.login(self.f.url, "maintainer", self.maintainer_password)
        revoked.request("POST", "/api/v1/auth/logout", {}, expected=204)
        self.receive_denied(revoked, 2, 401)
        self.denied_push(observer, self.bearer(revoked.token), "HEAD:refs/heads/forbidden-session")
        self.maintainer.request("DELETE", "/api/v1/auth/pats/" + owner_pat["metadata"]["id"])
        self.receive_denied(self.h.Client(self.f.url, owner_pat["raw_token"]), 2, 401)
        self.denied_push(observer, actors[3][1], "HEAD:refs/heads/forbidden-pat")
        inactive_pat = self.pat(self.inactive, "secondary-inactive")
        self.f.admin.request("POST", f"/api/v1/admin/users/{self.inactive_id}/deactivate", {})
        for token, env in [(self.inactive.token, self.bearer(self.inactive.token)),
                           (inactive_pat["raw_token"], self.basic("inactive", inactive_pat["raw_token"]))]:
            self.receive_denied(self.h.Client(self.f.url, token), 2, 403)
            self.denied_push(observer, env, "HEAD:refs/heads/forbidden-inactive")
        print("PASS read-only/role/team/anonymous/MCP/revoked/inactive denials and all-or-nothing agent-mixed refusal; all refs unchanged by denials", flush=True)

        final_state = self.read.state(self.f.repos[2])
        self.require(self.primary_state() == primary and self.read.state(self.f.repos[1]) == readonly,
                     "secondary writes changed primary worktrees/jobs/refs or the read-only repository")
        self.f.stop(self.f.process)
        self.launch()
        recovered_primary = self.recovered_primary(primary)
        self.require(self.read.state(self.f.repos[2]) == final_state,
                     "configured restart changed an ordinary hosted ref or symbolic HEAD")
        for client in [self.f.admin, self.maintainer, self.developer]:
            self.catalog(client, True)
        self.h.git(observer, "fetch", "--tags", "origin", env=self.bearer(self.developer.token))
        after_restart = self.push(self.clones[0][0], actors[0][1], "after-restart")
        self.h.git(observer, "fetch", "--tags", "origin", env=self.bearer(self.developer.token))
        self.require(self.h.git(observer, "rev-parse", "origin/after-restart") == after_restart[2]
                     and self.h.git(observer, "rev-parse", after_restart[1]) == after_restart[3],
                     "restart did not retain effective Git-write hosting and exact fetched refs")
        after_restart_state = self.read.state(self.f.repos[2])
        self.require(self.primary_state() == recovered_primary,
                     "ordinary writes after restart changed primary worktree state or jobs")
        self.f.stop(self.f.process)
        self.launch(writable=False)
        for client in [self.f.admin, self.maintainer, self.developer]:
            self.catalog(client, False)
        for client in [self.f.admin, self.maintainer]:
            self.receive_denied(client, 2, 404)
        self.denied_push(observer, actors[0][1], "HEAD:refs/heads/forbidden-with-old-flag")
        self.h.git(observer, "fetch", "--tags", "origin", env=self.bearer(self.developer.token))
        self.h.git(primary_clone, "fetch", "origin", env=actors[0][1])
        self.h.git(readonly_clone, "fetch", "origin", env=actors[1][1])
        self.recovered_primary(recovered_primary)
        self.require(self.read.state(self.f.repos[1]) == readonly
                     and self.read.state(self.f.repos[2]) == after_restart_state
                     and [self.read.anchors(repo) for repo in self.f.repos] == anchors,
                     "restart or fallback configuration changed primary state, read-only refs or registration anchors")
        self.require(final_state != self.read.state(self.f.repos[2]), "restart write did not create distinct proof refs")
        for index, repo in enumerate(self.f.repos):
            self.require(self.h.git(repo, "rev-parse", "refs/tags/preexisting") == preexisting[index]
                         and self.h.git(repo, "cat-file", "-t", preexisting[index]) == "tag",
                         "ordinary hosting replaced or peeled a preexisting annotated tag")
        for repo in [*self.f.repos, *(path for path, _ in self.clones)]:
            self.h.git(repo, "fsck", "--full", "--strict")
        print("PASS configured restart/UUID/anchors, original read-only flag denies pushes, primary managed worktree/jobs preserved and strict fsck", flush=True)
        return {"version": 1, "passed": True, "actual_repositories": 3,
                "admin_and_maintainer_session_basic_pat_round_trips": True,
                "independent_clone_fetch_commit_tag_oracles": True,
                "read_only_secondary_push_denied": True, "developer_and_wrong_team_denied": True,
                "anonymous_mcp_revoked_inactive_denied": True, "agent_and_mixed_pushes_preserve_refs": True,
                "secondary_workbench_denied": True, "current_catalog_push_hints": True,
                "configured_restart_write_and_uuid_preserved": True, "old_read_only_flag_push_denied": True,
                "primary_refs_worktrees_jobs_preserved": True, "registered_identity_and_preexisting_tags_preserved": True,
                "restart_reconstructed_display_fields": sorted(self.restart_display_fields),
                "git_fsck": True, "external_github_publication": False, "platform": os.uname().sysname}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    helper, multi, readonly = load("alpha-rehearsal"), load("multi-repository-rehearsal"), load("read-only-git-rehearsal")
    script, source, binary = Path(__file__).resolve(), args.source.resolve(strict=True), args.binary.resolve(strict=True)
    if args.report.exists() or args.report.is_symlink():
        raise RuntimeError("report already exists; choose a new path")
    runtime_paths = [script, *(script.with_name(name + ".py") for name in
                     ["alpha-rehearsal", "multi-repository-rehearsal", "read-only-git-rehearsal"])]
    runtime = {"scripts/" + path.name: helper.sha256(path) for path in runtime_paths}
    commit, dirty = helper.git(source, "rev-parse", "HEAD"), bool(helper.git(source, "status", "--porcelain"))
    binary_sha = helper.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-secondary-write-") as directory:
        scratch = Path(directory).resolve()
        fixture = multi.Rehearsal(helper, binary, scratch)
        try:
            report = Rehearsal(helper, fixture, readonly).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            helper.require(all(process.poll() is not None for process in fixture.processes),
                           "owned secondary Git-write service was not reaped")
    helper.require(not scratch.exists(), "private secondary Git-write fixture was not removed")
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
    print("Secondary Git-write rehearsal passed; owned services and private fixtures removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Secondary Git-write rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
