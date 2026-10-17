#!/usr/bin/env python3
"""Create two disposable registered repositories and exercise their first HTTP push."""
import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import urlencode


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"),
                                                 Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def create_arguments(fixture, index):
    return [str(fixture.binary), "repository", "create", "--repo", str(fixture.repos[index]),
            "--auth-db", str(fixture.auth), "--admin-username", "operator",
            "--name", f"created-{index}", "--owner-team-id", fixture.teams[index],
            "--private", "true", "--default-branch", ["main", "trunk"][index]]


def anchors(repo):
    identity = (repo / "gitust-repository/identity.json").read_bytes()
    receipt = (repo / "gitust-repository/registration.json").read_bytes()
    stat = repo.stat()
    return identity, receipt, stat.st_dev, stat.st_ino


def restore_empty(fixture):
    helper = fixture.helper
    backup = Path(__file__).with_name("backup.py")
    snapshot, restored_root = fixture.root / "empty-backup", fixture.root / "empty-restored"
    helper.command([sys.executable, str(backup), "create", "--repo", str(fixture.repos[0]),
                    "--auth-db", str(fixture.auth), "--output", str(snapshot)])
    helper.command([sys.executable, str(backup), "restore", "--backup", str(snapshot),
                    "--destination", str(restored_root)])
    restored = type(fixture)(helper, fixture.binary, restored_root)
    restored_repo = restored_root / "repository"
    fixture.require(anchors(restored_repo)[:2] == anchors(fixture.repos[0])[:2]
                    and restored.auth.read_bytes() == fixture.auth.read_bytes(),
                    "empty restore changed registration anchors or authentication state")
    hidden_repo, hidden_auth = fixture.root / "original-unavailable", fixture.auth.with_suffix(".offline")
    fixture.repos[0].rename(hidden_repo)
    try:
        fixture.auth.rename(hidden_auth)
        try:
            process, url = restored.launch(restored_repo)
            fixture.processes.append(process)
            restored.healthy(process, url)
            admin, _ = restored.login(url, "operator", fixture.password)
            rows = admin.request("GET", "/api/v1/repos")["repositories"]
            fixture.require(len(rows) == 1 and rows[0]["id"] == fixture.ids[0]
                            and rows[0]["branch"] == "main" and rows[0]["head_commit"] is None,
                            "restored empty repository cannot serve its preserved identity")
            fixture.git(restored_repo, "fsck", "--full", "--strict")
        finally:
            for process in reversed(restored.processes):
                restored.stop(process)
            hidden_auth.rename(fixture.auth)
    finally:
        hidden_repo.rename(fixture.repos[0])


def setup(fixture):
    helper = fixture.helper
    helper.command([str(fixture.binary), "admin", "create", "--username", "operator",
                    "--email", "operator@company.internal", "--auth-db", str(fixture.auth)],
                   env={"GITUST_ADMIN_PASSWORD": fixture.password})
    ids = []
    for index, repo in enumerate(fixture.repos):
        arguments = create_arguments(fixture, index)
        helper.command(arguments, env={"GITUST_ADMIN_PASSWORD": fixture.password})
        helper.require(fixture.git(repo, "rev-parse", "--is-bare-repository") == "true",
                       "creation did not produce a bare repository")
        helper.require(fixture.git(repo, "symbolic-ref", "HEAD")
                       == "refs/heads/" + ["main", "trunk"][index],
                       "creation did not retain the requested default branch")
        helper.require(fixture.git(repo, "for-each-ref", "--format=%(refname)") == "",
                       "creation fabricated a reference or first commit")
        helper.require(repo.stat().st_mode & 0o7777 == 0o700,
                       "created repository is not private")
        for name in ["identity.json", "registration.json"]:
            helper.require((repo / "gitust-repository" / name).stat().st_mode & 0o7777 == 0o600,
                           "created registration metadata is not private")
        before = anchors(repo)
        ids.append(json.loads(before[0])["repo_id"])
        helper.command(arguments, env={"GITUST_ADMIN_PASSWORD": fixture.password})
        helper.require(anchors(repo) == before
                       and fixture.git(repo, "for-each-ref", "--format=%(refname)") == "",
                       "identical empty-create retry replaced its identity, binding or inode")
        fixture.git(repo, "fsck", "--full", "--strict")
    helper.require(len(set(ids)) == 2, "creation reused a repository UUID")
    fixture.ids = ids
    restore_empty(fixture)
    fixture.process, fixture.url = fixture.launch(fixture.repos[0], [fixture.repos[1]])
    fixture.healthy(fixture.process, fixture.url)
    fixture.admin, _ = fixture.login(fixture.url, "operator", fixture.password)


def catalog(fixture, head):
    rows = fixture.admin.request("GET", "/api/v1/repos")["repositories"]
    fixture.require(len(rows) == 2 and {row["id"] for row in rows} == set(fixture.ids),
                    "created repository catalog does not match actual UUIDs")
    by_id = {row["id"]: row for row in rows}
    for index, expected in enumerate([head, None]):
        row = by_id[fixture.ids[index]]
        fixture.require(row["head_commit"] == expected
                        and row["branch"] == ["main", "trunk"][index]
                        and row["is_default"] is (index == 0)
                        and row["capability"] == ["workbench", "browse"][index],
                        "created repository advertises the wrong HEAD or capabilities")
    summary = fixture.admin.request("GET", "/api/v1/repo")
    fixture.require(summary["id"] == fixture.ids[0] and summary["head_commit"] == head,
                    "primary summary differs from actual created repository")


def branch_pages(fixture, index):
    expected = fixture.git(fixture.repos[index], "for-each-ref",
                           "--format=%(refname)%09%(objectname)", "refs/heads/")
    expected = [{"ref_name": ref, "name": ref.removeprefix("refs/heads/"),
                 "commit_sha": sha} for ref, sha in
                (line.split("\t") for line in expected.splitlines())]
    actual, seen, cursor = [], set(), None
    for _ in range(4):
        query = {"limit": 1}
        if cursor is not None:
            query["cursor"] = cursor
        page = fixture.admin.request(
            "GET", f"/api/v1/repos/{fixture.ids[index]}/branches?" + urlencode(query))
        rows = page["branches"]
        fixture.require(isinstance(rows, list) and len(rows) <= 1,
                        "branch page exceeded its requested limit")
        actual.extend({key: row[key] for key in ["ref_name", "name", "commit_sha"]}
                      for row in rows)
        cursor = page["next_cursor"]
        if cursor is None:
            break
        fixture.require(bool(rows) and isinstance(cursor, str) and cursor not in seen,
                        "branch pagination did not advance")
        seen.add(cursor)
    else:
        raise RuntimeError("branch pagination exceeded the disposable fixture's bound")
    fixture.require(actual == expected, "UUID branch pages differ from independent Git refs")
    return actual


def execute(fixture):
    setup(fixture)
    catalog(fixture, None)
    fixture.require(branch_pages(fixture, 0) == [] and branch_pages(fixture, 1) == [],
                    "empty creation fabricated a branch entry")
    maximum_cursor = "v1." + base64.urlsafe_b64encode(
        ("refs/heads/" + "é" * 16384).encode("utf-8")).decode("ascii").rstrip("=")
    fixture.require(len(maximum_cursor) == 43709, "cursor fixture does not reach the supported bound")
    page = fixture.admin.request("GET", f"/api/v1/repos/{fixture.ids[0]}/branches?"
                                 + urlencode({"limit": 1, "cursor": maximum_cursor}))
    fixture.require(page["branches"] == [] and page["next_cursor"] is None,
                    "maximum supported cursor did not round-trip through the actual HTTP listener")
    helper = fixture.helper
    url = fixture.url + "/git/" + fixture.ids[0] + ".git"
    env = {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.extraHeader",
           "GIT_CONFIG_VALUE_0": "Authorization: Bearer " + fixture.admin.token,
           "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"}
    clone = fixture.root / "http-client"
    helper.git(fixture.root, "clone", url, str(clone), env=env)
    fixture.git(clone, "checkout", "-b", "develop")
    (clone / "README.md").write_text("# Created repository\n\nFirst develop commit.\n")
    fixture.git(clone, "add", "README.md")
    fixture.git(clone, "-c", "user.name=Gitust Rehearsal", "-c",
                "user.email=rehearsal@company.internal", "commit", "-m", "First develop commit")
    helper.git(clone, "push", "origin", "HEAD:refs/heads/develop", env=env)
    develop = fixture.git(clone, "rev-parse", "HEAD")
    fixture.require(fixture.git(fixture.repos[0], "rev-parse", "develop") == develop,
                    "first alternate push differs from independent Git")
    catalog(fixture, None)
    branches = branch_pages(fixture, 0)
    fixture.require(len(branches) == 1 and branches[0]["name"] == "develop"
                    and branches[0]["commit_sha"] == develop,
                    "unborn default hid the first pushed branch")
    alternate = fixture.admin.request(
        "GET", f"/api/v1/repos/{fixture.ids[0]}/blob?rev={develop}&path=README.md")
    fixture.require(alternate["commit_sha"] == develop
                    and alternate["content"] == fixture.git(fixture.repos[0], "show",
                                                           develop + ":README.md") + "\n",
                    "unborn default hid the existing alternate branch's committed file")
    (clone / "README.md").write_text("# Created repository\n\nThe main branch is ready.\n")
    fixture.git(clone, "add", "README.md")
    fixture.git(clone, "-c", "user.name=Gitust Rehearsal", "-c",
                "user.email=rehearsal@company.internal", "commit", "-m", "First main commit")
    head = fixture.git(clone, "rev-parse", "HEAD")
    helper.git(clone, "push", "origin", "HEAD:refs/heads/main", env=env)
    helper.git(clone, "fetch", "origin", env=env)
    fixture.require(fixture.git(fixture.repos[0], "rev-parse", "HEAD") == head
                    and fixture.git(clone, "rev-parse", "origin/main") == head,
                    "first main push/fetch differs from independent Git")
    catalog(fixture, head)
    branch_pages(fixture, 0)
    fixture.require(branch_pages(fixture, 1) == [], "primary refs leaked into the secondary page")
    blob = fixture.admin.request("GET", f"/api/v1/repos/{fixture.ids[0]}/blob?rev={head}&path=README.md")
    fixture.require(blob["commit_sha"] == head and blob["content"] == (clone / "README.md").read_text(),
                    "created repository browser reads a different committed file")
    fixture.admin.request("GET", f"/git/{fixture.ids[1]}.git/info/refs?service=git-receive-pack",
                          expected=404)
    before = [anchors(repo) for repo in fixture.repos]
    refs = fixture.git(fixture.repos[0], "for-each-ref", "--format=%(refname) %(objectname)")
    fixture.stop(fixture.process)
    helper.command(create_arguments(fixture, 0), env={"GITUST_ADMIN_PASSWORD": fixture.password},
                   expected=1)
    fixture.require([anchors(repo) for repo in fixture.repos] == before
                    and fixture.git(fixture.repos[0], "for-each-ref",
                                    "--format=%(refname) %(objectname)") == refs,
                    "obsolete create retry changed a populated repository or its identity")
    fixture.process, fixture.url = fixture.launch(fixture.repos[0], [fixture.repos[1]])
    fixture.healthy(fixture.process, fixture.url)
    fixture.admin, _ = fixture.login(fixture.url, "operator", fixture.password)
    catalog(fixture, head)
    branch_pages(fixture, 0)
    for repo in [*fixture.repos, clone]:
        fixture.git(repo, "fsck", "--full", "--strict")
    source = Path(__file__).resolve().parent.parent
    return {"version": 1, "passed": True,
            "source_commit": fixture.git(source, "rev-parse", "HEAD"),
            "runtime_worktree_dirty": bool(fixture.git(source, "status", "--porcelain")),
            "binary_sha256": helper.sha256(fixture.binary), "platform": os.uname().sysname,
            "operator_script_sha256": helper.sha256(Path(__file__)), "actual_repositories": 2,
            "cli_created_registered_bare_repositories": True, "empty_retry_preserved_pair": True,
            "unborn_registration_restored_without_original": True,
            "unborn_default_did_not_hide_other_branches": True,
            "first_ordinary_push_and_fetch": True, "independent_git_read_oracles": True,
            "secondary_push_denied": True, "populated_retry_rejected_unchanged": True,
            "restart_identity_preserved": True, "git_fsck": True,
            "uuid_branch_pages_match_git": True,
            "unborn_default_exposes_existing_branches": True, "secondary_branch_reads": True,
            "max_cursor_http_round_trip": True,
            "external_github_publication": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    if args.report.exists():
        raise RuntimeError("report already exists; choose a new path")
    helper, module = load("alpha-rehearsal"), load("multi-repository-rehearsal")
    with tempfile.TemporaryDirectory(prefix="gitust-empty-repository-") as directory:
        fixture = module.Rehearsal(helper, binary, Path(directory).resolve())
        try:
            report = execute(fixture)
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("Empty creation, identical retry and first HTTP push passed; private fixture removed.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        print("Empty-repository rehearsal failed:", str(error) if isinstance(error, RuntimeError)
              else type(error).__name__)
        raise SystemExit(1) from None
