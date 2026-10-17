"""Exercise existing-account membership using the three-repository onboarding fixture."""
import base64
from concurrent.futures import ThreadPoolExecutor
import json
import re
import urllib.error
import urllib.parse
import urllib.request
import uuid


def request(rehearsal, client, method, route, payload, expected):
    headers = {"Content-Type": "application/json"}
    if client.token:
        headers["Authorization"] = "Bearer " + client.token
    body = None if payload is None else json.dumps(payload).encode()
    query = urllib.request.Request(client.url + route, body, headers, method=method)
    try:
        response = client.opener.open(query, timeout=120)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        status = response.status
        raw = response.read(16385)
        cache = response.headers.get("Cache-Control", "")
    allowed = expected if isinstance(expected, tuple) else (expected,)
    rehearsal.require(status in allowed, f"membership HTTP {status}, expected {allowed}")
    rehearsal.require(len(raw) <= 16384 and "no-store" in cache,
                      "membership response is unbounded or cacheable")
    return status, json.loads(raw) if raw.startswith(b"{") else None


def check_view(rehearsal, value, user_id, username, member, version, repository_index=0):
    fields = {"repository_id", "owner_team_id", "user_id", "username", "role",
              "status", "is_member", "membership_version"}
    rehearsal.require(isinstance(value, dict) and set(value) == fields,
                      "membership response has missing or unexpected fields")
    actual_version = value["membership_version"]
    rehearsal.require(isinstance(actual_version, str)
                      and re.fullmatch(r"0|[1-9][0-9]{0,19}", actual_version) is not None
                      and int(actual_version) <= 18446744073709551615,
                      "membership version is not an exact canonical u64 string")
    rehearsal.require(value["repository_id"] == rehearsal.f.ids[repository_index]
                      and value["owner_team_id"] == rehearsal.f.teams[0]
                      and value["user_id"] == user_id and value["username"] == username
                      and value["role"] == "Developer" and value["status"] == "active"
                      and value["is_member"] is member and actual_version == str(version),
                      "membership response does not confirm the exact account, group and version")


def run(rehearsal):
    r, f = rehearsal, rehearsal.f
    member, user_id = f.members[1]
    username = "member-1"
    base = f"/api/v1/repos/{f.ids[0]}/group-members"
    lookup = base + "?" + urllib.parse.urlencode({"username": username})
    mutation = base + "/" + user_id
    grant = {"expected_membership_version": "0", "is_member": True}
    before = f.admin.request("GET", "/api/v1/admin/users")
    # The previous binary has no membership API; this is its feature-absence boundary.
    request(r, r.h.Client(f.url), "GET", lookup, None, 401)
    r.require(all(row.get("can_manage_group_members") is True for row in
                  f.admin.request("GET", "/api/v1/repos")["repositories"]),
              "registered administrator repositories lack their membership capability")
    for client, _ in f.members:
        request(r, client, "GET", lookup, None, 403)
        request(r, client, "PATCH", mutation, grant, 403)
        r.require(all(row.get("can_manage_group_members") is False for row in
                      client.request("GET", "/api/v1/repos")["repositories"]),
                  "nonadministrator was offered membership administration")
    request(r, f.admin, "GET",
            f"/api/v1/repos/{f.unhosted_registered_id}/group-members?username={username}", None, 404)
    _, missing = request(r, f.admin, "GET", base + "?username=missing-account", None, 404)
    r.require(missing == {"code": "group_member_not_found"},
              "unknown-account lookup does not distinguish its scope from repository failure")
    request(r, f.admin, "PATCH", base + "/" + str(uuid.uuid4()), grant, 404)
    for payload, expected in [({**grant, "expected_membership_version": 0}, 400),
                              ({**grant, "expected_membership_version": "00"}, 400),
                              ({**grant, "expected_membership_version": "18446744073709551616"}, 400),
                              ({**grant, "team_ids": [f.teams[1]]}, 400),
                              ({**grant, "padding": "x" * 4096}, 413)]:
        request(r, f.admin, "PATCH", mutation, payload, expected)
    r.require(f.admin.request("GET", "/api/v1/admin/users") == before,
              "denied or malformed membership requests changed account profiles")
    _, original = request(r, f.admin, "GET", lookup, None, 200)
    check_view(r, original, user_id, username, False, 0)
    r.require({row["id"] for row in member.request("GET", "/api/v1/repos")["repositories"]}
              == {f.ids[1]}, "existing account must start with only its original group")
    for index in [0, 2]:
        member.request("GET", f"/api/v1/repos/{f.ids[index]}/tree", expected=403)
    issued = member.request("POST", "/api/v1/auth/pats", {"name": "membership-read"}, expected=201)
    basic = "Basic " + base64.b64encode((username + ":" + issued["raw_token"]).encode()).decode()
    env = r.environment(basic)

    with ThreadPoolExecutor(max_workers=2) as pool:
        replies = list(pool.map(lambda _: request(r, f.admin, "PATCH", mutation, grant, (200, 409)), range(2)))
    r.require(sorted(status for status, _ in replies) == [200, 409],
              "concurrent membership changes did not compare the original durable version")
    check_view(r, next(value for status, value in replies if status == 200), user_id, username, True, 1)
    r.require(next(value for status, value in replies if status == 409) == {"code": "membership_conflict"},
              "stale membership write did not return an exact conflict")
    r.require({row["id"] for row in member.request("GET", "/api/v1/repos")["repositories"]}
              == set(f.ids), "group addition lost an existing grant or omitted the shared group pair")
    for index in [0, 2]:
        member.request("GET", f"/api/v1/repos/{f.ids[index]}/tree")
    clone = r.clone(2, "existing-member-group-a-clone", env)
    r.h.git(clone, "fetch", "origin", env=env)
    r.require(r.h.git(clone, "rev-parse", "refs/tags/group-invite-proof")
              == r.h.git(f.repos[2], "rev-parse", "refs/tags/group-invite-proof")
              and r.h.git(clone, "cat-file", "-t", "refs/tags/group-invite-proof") == "tag",
              "membership clone omitted or peeled the raw annotated tag")
    r.expect_git_failure(clone, env, "push", "origin", "HEAD:refs/heads/no-membership-write")
    member.request("POST", "/api/v1/swarm/worktrees", {"repo_id": f.ids[2],
        "branch": "agent/no-membership-write", "base_branch": "main", "prompt_summary": "Read only"}, expected=404)
    r.restart()
    r.h.git(clone, "remote", "set-url", "origin", r.url(2))
    _, persisted = request(r, f.admin, "GET", lookup, None, 200)
    check_view(r, persisted, user_id, username, True, 1)
    r.h.git(clone, "fetch", "origin", env=env)

    # The group is shared: use the other registered repository to remove it.
    secondary = f"/api/v1/repos/{f.ids[2]}/group-members/{user_id}"
    _, removed = request(r, f.admin, "PATCH", secondary,
                         {"expected_membership_version": "1", "is_member": False}, 200)
    check_view(r, removed, user_id, username, False, 2, repository_index=2)
    # A lost old grant response must never turn this later removal back into access.
    _, stale = request(r, f.admin, "PATCH", mutation, grant, 409)
    r.require(stale == {"code": "membership_conflict"}, "old grant replay restored access")
    r.require({row["id"] for row in member.request("GET", "/api/v1/repos")["repositories"]}
              == {f.ids[1]}, "removal changed another group or retained private group access")
    for index in [0, 2]:
        member.request("GET", f"/api/v1/repos/{f.ids[index]}/tree", expected=403)
        r.h.Client(f.url, issued["raw_token"]).request("GET",
            f"/git/{f.ids[index]}.git/info/refs?service=git-upload-pack", expected=403)
    r.expect_git_failure(clone, env, "fetch", "origin")
    _, noop = request(r, f.admin, "PATCH", mutation,
                      {"expected_membership_version": "2", "is_member": False}, 200)
    check_view(r, noop, user_id, username, False, 2)
    r.restart()
    _, restarted = request(r, f.admin, "GET", lookup, None, 200)
    check_view(r, restarted, user_id, username, False, 2)
    r.require(f.admin.request("GET", "/api/v1/admin/users") == before,
              "membership round trip changed another group, profile, role or account status")
    member.request("DELETE", "/api/v1/auth/pats/" + issued["metadata"]["id"])
    r.unchanged()
    r.h.git(clone, "fsck", "--full", "--strict")
    print("PASS existing member grant/removal, durable version CAS, unchanged other group and real Git access revocation", flush=True)
    return {"existing_member_group_grant_and_removal": True,
            "membership_version_replay_and_restart": True,
            "membership_preserves_other_groups_role_and_status": True,
            "membership_real_git_access_revocation": True}
