#!/usr/bin/env python3
"""Verify new invitations and existing membership across three real repositories."""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
import re
from pathlib import Path
import secrets
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import uuid


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"),
                                                Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


read_only = load("read-only-git-rehearsal")


def utc_nanoseconds(value):
    match = re.fullmatch(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(?:Z|\+00:00)", value)
    if match is None:
        raise ValueError("invalid UTC invitation timestamp")
    whole = datetime.fromisoformat(match[1] + "+00:00")
    elapsed = whole - datetime(1970, 1, 1, tzinfo=timezone.utc)
    return (elapsed.days * 86400 + elapsed.seconds) * 1_000_000_000 + int((match[2] or "0").ljust(9, "0"))


class Rehearsal(read_only.Rehearsal):
    def prepare(self):
        self.f.setup()
        self.f.stop(self.f.process)
        seed, third = self.f.root / "group-a-seed", self.f.root / "group-a-secondary.git"
        seed.mkdir()
        self.h.git(seed, "init", "--initial-branch=main")
        self.h.git(seed, "config", "user.name", "Gitust Invitation Rehearsal")
        self.h.git(seed, "config", "user.email", "invitation@company.internal")
        (seed / "marker.txt").write_text("repository-2-after\n")
        self.h.git(seed, "add", "marker.txt")
        self.h.git(seed, "commit", "-m", "test: isolated group-A secondary")
        self.h.git(seed, "tag", "-a", "group-invite-proof", "-m", "Keep the raw tag object")
        self.h.git(self.f.root, "clone", "--bare", "--no-local", str(seed), str(third))
        self.f.register_repository(third, "repository-2", self.f.teams[0])
        self.f.repos.append(third)
        self.restart()
        catalog = self.f.admin.request("GET", "/api/v1/repos")["repositories"]
        self.require(len(catalog) == 3, "three distinct configured repositories are required")
        extra = next(row for row in catalog if row["name"] == "repository-2")
        self.require(extra["id"] not in self.f.ids and extra["capability"] == "browse",
                     "shared-group secondary has an ambiguous identity or write capability")
        self.f.ids.append(extra["id"])
        self.states, self.identities = [self.state(p) for p in self.f.repos], [self.anchors(p) for p in self.f.repos]

    def restart(self):
        self.f.stop(self.f.process)
        self.f.process, self.f.url = self.f.launch(self.f.repos[0], self.f.repos[1:])
        self.f.healthy(self.f.process, self.f.url)
        self.f.admin.url = self.f.url
        for member, _ in self.f.members:
            member.url = self.f.url

    def api(self, client, route, payload, expected):
        headers = {"Content-Type": "application/json"}
        if client.token:
            headers["Authorization"] = "Bearer " + client.token
        request = urllib.request.Request(client.url + route, json.dumps(payload).encode(), headers, method="POST")
        try:
            response = client.opener.open(request, timeout=120)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            code, body, cache = response.status, response.read(64 * 1024), response.headers.get("Cache-Control", "")
        expected = expected if isinstance(expected, tuple) else (expected,)
        self.require(code in expected, f"group-invite HTTP {code}, expected {expected}")
        self.require("no-store" in cache, "group-invite response may cache a credential or authority result")
        return code, json.loads(body) if body.startswith(b"{") else None

    def stored(self, invite_id):
        rows = json.loads(self.f.auth.read_text())["invites"]
        matches = [row for row in rows if row["id"] == invite_id]
        self.require(len(matches) == 1, "retry key does not identify exactly one persisted invitation")
        return matches[0]

    def check_scope(self, result, body):
        invite = result["invite"]
        self.require(result["repository_id"] == self.f.ids[0]
                     and result["owner_team_id"] == self.f.teams[0]
                     and invite["repository_id"] == self.f.ids[0]
                     and invite["id"] == body["invite_id"]
                     and invite["target_email"] == body["target_email"].strip().lower()
                     and invite["assigned_role"] == body["assigned_role"]
                     and invite["purpose"] == "account_invite"
                     and invite["team_ids"] == [self.f.teams[0]],
                     "invitation response does not confirm the actual origin and grant scope")
        self.require("registration_nonce" not in json.dumps(result)
                     and "repository_origin" not in json.dumps(result)
                     and "token_sha256" not in json.dumps(result), "private invitation metadata leaked")

    def unchanged(self):
        self.require([self.state(p) for p in self.f.repos] == self.states
                     and [self.anchors(p) for p in self.f.repos] == self.identities,
                     "invitation or denied write changed Git refs or registration anchors")

    def check_replay(self, result, body):
        self.require(result["code"] == "invite_already_issued", "same key was not recognized")
        self.check_scope(result, body)
        self.require(set(result) == {"code", "repository_id", "owner_team_id", "invite"}
                     and "invite_url_path" not in json.dumps(result) and "raw_token" not in json.dumps(result),
                     "replay returned a recoverable or freshly minted credential")

    def check_conflict(self, result):
        self.require(result == {"code": "invite_request_conflict"},
                     "conflicting intent exposed metadata or a credential")

    def replay(self, body):
        _, result = self.api(self.f.admin, self.route, body, 409)
        self.check_replay(result, body)
        return result

    def execute(self):
        self.prepare()
        membership = load("group-membership-checks").run(self)
        self.route = f"/api/v1/repos/{self.f.ids[0]}/group-invites"
        body = {"invite_id": str(uuid.uuid4()), "target_email": "new-group-member@company.internal",
                "assigned_role": "Developer"}
        # The previous binary has no route; this assertion must fail there.
        self.api(self.h.Client(self.f.url), self.route, body, 401)
        before = self.f.admin.request("GET", "/api/v1/admin/invites")
        for member, _ in self.f.members:
            self.api(member, self.route, body, 403)
            self.require(all(row.get("can_invite_to_group") is False for row in
                             member.request("GET", "/api/v1/repos")["repositories"]),
                         "a nonadministrator received an invitation capability hint")
        self.require(all(row.get("can_invite_to_group") is True for row in
                         self.f.admin.request("GET", "/api/v1/repos")["repositories"]),
                     "configured registered administrator context lacks its invitation hint")
        for payload, status in [({**body, "team_ids": [self.f.teams[1]]}, 400),
                                ({**body, "invite_id": str(uuid.UUID(int=0))}, 400),
                                ({**body, "extra": "x" * 4096}, 413)]:
            self.api(self.f.admin, self.route, payload, status)
        self.api(self.f.admin, f"/api/v1/repos/{self.f.unhosted_registered_id}/group-invites", body, 404)
        self.require(self.f.admin.request("GET", "/api/v1/admin/invites") == before,
                     "a denied or malformed request inserted an invitation")

        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda _: self.api(self.f.admin, self.route, body, (201, 409)), range(2)))
        self.require(sorted(code for code, _ in results) == [201, 409],
                     "concurrent identical intent created more than one invitation")
        created = next(result for code, result in results if code == 201)
        self.check_replay(next(result for code, result in results if code == 409), body)
        self.check_scope(created, body)
        self.require("raw_token" not in created, "HTTP creation returned an unnecessary raw-token field")
        link = urllib.parse.urlsplit(created["invite_url_path"])
        self.require(not link.scheme and not link.netloc and not link.fragment
                     and link.path == "/auth/accept-invite", "invitation link left the local account workflow")
        query = urllib.parse.parse_qs(link.query)
        self.require(set(query) == {"token"} and len(query["token"]) == 1, "invalid single-use link")
        token = query["token"][0]
        stored = self.stored(body["invite_id"])
        self.require(stored["token_sha256"] == list(hashlib.sha256(token.encode()).digest())
                     and token not in self.f.auth.read_text(), "invitation token is not stored hash-only")
        start = utc_nanoseconds(stored["created_at"])
        end = utc_nanoseconds(stored["expires_at"])
        self.require(end - start == 72 * 3600 * 1_000_000_000, "group invitation changed the 72-hour policy")
        self.replay(body)
        for changed in [{**body, "assigned_role": "Maintainer"},
                        {**body, "target_email": "different@company.internal"}]:
            _, response = self.api(self.f.admin, self.route, changed, 409)
            self.check_conflict(response)
        _, response = self.api(self.f.admin, f"/api/v1/repos/{self.f.ids[2]}/group-invites", body, 409)
        self.check_conflict(response)
        self.restart()
        self.replay(body)
        self.require(self.stored(body["invite_id"]) == stored, "replay or restart changed token identity/deadline")
        print("PASS strict authority/body checks, concurrent one-key issuance, scoped replay and durable restart", flush=True)

        password = secrets.token_urlsafe(24)
        profile = self.h.Client(self.f.url).request("POST", "/api/v1/auth/invites/redeem", {
            "raw_token": token, "username": "new-group-member", "password": password}, expected=201)
        self.require(profile["team_ids"] == [self.f.teams[0]] and profile["role"] == "Developer",
                     "redeemed account did not receive exactly the existing group/role grant")
        member, member_id = self.f.login(self.f.url, "new-group-member", password)
        rows = member.request("GET", "/api/v1/repos")["repositories"]
        self.require({row["id"] for row in rows} == {self.f.ids[0], self.f.ids[2]}
                     and all(row.get("can_clone") is True for row in rows)
                     and all(row.get("can_invite_to_group") is False for row in rows),
                     "owning-group grant did not expose exactly the two group-A repositories")
        member.request("GET", f"/api/v1/repos/{self.f.ids[1]}/tree", expected=403)
        member.request("GET", f"/git/{self.f.ids[1]}.git/info/refs?service=git-upload-pack", expected=403)
        self.require(self.f.members[2][0].request("GET", "/api/v1/repos")["repositories"] == [],
                     "account-only invitation unexpectedly grants private group access")
        issued = member.request("POST", "/api/v1/auth/pats", {"name": "group-read"}, expected=201)
        basic = "Basic " + base64.b64encode(("new-group-member:" + issued["raw_token"]).encode()).decode()
        env = self.environment(basic)
        clone = self.clone(2, "new-member-group-a-clone", env)
        self.h.git(clone, "fetch", "origin", env=env)
        self.require(self.h.git(clone, "rev-parse", "refs/tags/group-invite-proof")
                     == self.h.git(self.f.repos[2], "rev-parse", "refs/tags/group-invite-proof")
                     and self.h.git(clone, "cat-file", "-t", "refs/tags/group-invite-proof") == "tag",
                     "new group member lost the raw annotated tag")
        self.expect_git_failure(clone, env, "push", "origin", "HEAD:refs/heads/no-invite-write")
        for actor in [self.f.admin, member]:
            actor.request("GET", f"/git/{self.f.ids[2]}.git/info/refs?service=git-receive-pack", expected=404)
            actor.request("POST", "/api/v1/swarm/worktrees", {"repo_id": self.f.ids[2],
                "branch": "agent/no-secondary-invite", "base_branch": "main", "prompt_summary": "No write grant"}, expected=404)
        self.h.Client(self.f.url).request("POST", "/api/v1/auth/invites/redeem", {
            "raw_token": token, "username": "must-not-exist", "password": password}, expected=400)
        self.replay(body)
        consumed = self.stored(body["invite_id"])
        self.require(consumed["used_at"] is not None and all(consumed[k] == v for k, v in stored.items() if k != "used_at"),
                     "consumed-key replay reminted a credential or changed its original deadline")

        revoked_body = {**body, "invite_id": str(uuid.uuid4()), "target_email": "revoked-group@company.internal"}
        self.api(self.f.admin, self.route, revoked_body, 201)
        self.f.admin.request("DELETE", "/api/v1/admin/invites/" + revoked_body["invite_id"])
        revoked = self.stored(revoked_body["invite_id"])
        self.replay(revoked_body)
        self.require(revoked["revoked_at"] is not None and self.stored(revoked_body["invite_id"]) == revoked,
                     "revoked-key replay refreshed or reminted an invitation")
        revoked_admin, _ = self.f.login(self.f.url, "operator", self.f.password)
        revoked_admin.request("POST", "/api/v1/auth/logout", expected=204)
        self.api(revoked_admin, self.route, {**body, "invite_id": str(uuid.uuid4())}, 401)
        self.f.admin.request("POST", f"/api/v1/admin/users/{member_id}/deactivate", {})
        self.h.Client(self.f.url, issued["raw_token"]).request("GET",
            f"/git/{self.f.ids[2]}.git/info/refs?service=git-upload-pack", expected=403)
        self.expect_git_failure(clone, env, "fetch", "origin")
        self.unchanged()
        for repo in [*self.f.repos, clone]:
            self.h.git(repo, "fsck", "--full", "--strict")
        print("PASS invited group-A pair visibility, group-B denial, real clone/fetch and retained secondary write denial", flush=True)
        return {"version": 2, "passed": True, "actual_repositories": 3, **membership,
                "role_anonymous_and_malformed_denials": True, "one_key_concurrent_issuance": True,
                "hash_only_token_and_72_hour_expiry": True, "exact_origin_and_semantic_conflicts": True,
                "restart_replay_preserves_identity": True, "consumed_and_revoked_keys_do_not_remint": True,
                "new_member_sees_exact_group_pair": True, "other_group_and_account_only_denied": True,
                "actual_secondary_clone_fetch_and_tags": True, "secondary_push_and_worktrees_denied": True,
                "revoked_admin_and_deactivated_member_denied": True,
                "git_refs_and_registration_unchanged": True, "git_fsck": True,
                "external_github_publication": False, "platform": os.uname().sysname}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    h, multi = load("alpha-rehearsal"), load("multi-repository-rehearsal")
    script, source, binary = Path(__file__).resolve(), args.source.resolve(strict=True), args.binary.resolve(strict=True)
    h.require(not args.report.exists() and not args.report.is_symlink(), "report already exists")
    runtime_paths = [script, script.with_name("alpha-rehearsal.py"), script.with_name("multi-repository-rehearsal.py"),
                     script.with_name("read-only-git-rehearsal.py"), script.with_name("group-membership-checks.py")]
    runtime = {"scripts/" + p.name: h.sha256(p) for p in runtime_paths}
    commit, dirty, binary_sha = h.git(source, "rev-parse", "HEAD"), bool(h.git(source, "status", "--porcelain")), h.sha256(binary)
    with tempfile.TemporaryDirectory(prefix="gitust-group-invite-") as directory:
        scratch = Path(directory).resolve()
        f = multi.Rehearsal(h, binary, scratch)
        try:
            report = Rehearsal(h, f).execute()
        finally:
            for process in reversed(f.processes):
                f.stop(process)
            h.require(all(p.poll() is not None for p in f.processes), "owned group-invite service was not reaped")
    h.require(not scratch.exists(), "private group-invite fixture was not removed")
    h.require(runtime == {"scripts/" + p.name: h.sha256(p) for p in runtime_paths}
              and h.sha256(binary) == binary_sha and h.git(source, "rev-parse", "HEAD") == commit,
              "rehearsal source or binary changed")
    for name, digest in runtime.items():
        result = subprocess.run(["git", "-C", str(source), "show", commit + ":" + name],
                                env=h.clean_environment(), capture_output=True, timeout=30)
        dirty |= result.returncode != 0 or hashlib.sha256(result.stdout).hexdigest() != digest
    report.update({"source_commit": commit, "runtime_worktree_dirty": dirty or bool(h.git(source, "status", "--porcelain")),
                   "binary_sha256": binary_sha, "runtime_source_sha256": runtime,
                   "owned_services_reaped": True, "private_fixture_removed": True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as out:
        json.dump(report, out, indent=2)
        out.write("\n")
    print("Owning-group invitations and membership passed; owned services and private fixtures removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, StopIteration, subprocess.SubprocessError) as error:
        print("Group-invite rehearsal failed:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
