#!/usr/bin/env python3
"""Exercise local OpenSSH Git reads and stopped host-identity restoration on two repos."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import socket
import stat
import subprocess
import sys
import tempfile


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def refs(text):
    result = {}
    for line in text.splitlines():
        fields = line.split("\0")
        if (len(fields) != 2 or not fields[0].startswith("refs/")
                or not re.fullmatch(r"[0-9a-f]{40}", fields[1]) or fields[0] in result):
            raise RuntimeError("invalid native reference oracle")
        result[fields[0]] = fields[1]
    return result


class Rehearsal:
    def __init__(self, helper, fixture, scratch):
        self.h, self.f, self.scratch = helper, fixture, scratch
        self.stage = "prepare"
        self.keys = scratch / "client-keys"
        self.keys.mkdir(mode=0o700)
        self.clones = []
        self.upstream = scratch / "unused-upstream.git"
        self.h.git(scratch, "init", "--bare", str(self.upstream))

    def require(self, condition, message):
        self.h.require(condition, message)

    def common(self, repo):
        return Path(self.h.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))

    def state(self, repo):
        return (self.h.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)"),
                self.h.git(repo, "symbolic-ref", "HEAD"),
                self.h.git(repo, "rev-parse", "HEAD"),
                self.h.git(repo, "worktree", "list", "--porcelain"))

    def anchors(self, repo):
        return [(self.common(repo) / "gitust-repository" / name).read_bytes()
                for name in ["identity.json", "registration.json"]]

    def unchanged(self):
        self.require([self.state(repo) for repo in self.f.repos] == self.states
                     and [self.anchors(repo) for repo in self.f.repos] == self.identities,
                     "SSH read or rejection changed refs, HEAD, worktrees or registration")

    def generate(self, path):
        self.h.command(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "", "-f", str(path)])
        self.require(stat.S_IMODE(path.stat().st_mode) == 0o600,
                     "generated fixture private key is not private")
        return Path(str(path) + ".pub").read_text().strip()

    def register_key(self, client, name):
        path = self.keys / name
        public = self.generate(path)
        metadata = client.request("POST", "/api/v1/auth/ssh-keys", {
            "title": name, "public_key_openssh": public}, expected=201)
        self.require(metadata["revoked"] is False, "registered fixture key is revoked")
        return path, metadata["id"]

    def environment(self, key):
        command = ["ssh", "-F", "/dev/null", "-i", str(key),
                   "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none",
                   "-o", "PreferredAuthentications=publickey", "-o", "PasswordAuthentication=no",
                   "-o", "KbdInteractiveAuthentication=no", "-o", "HostKeyAlgorithms=ssh-ed25519",
                   "-o", "PubkeyAcceptedAlgorithms=ssh-ed25519", "-o", "StrictHostKeyChecking=yes",
                   "-o", "UserKnownHostsFile=" + str(self.known_hosts),
                   "-o", "GlobalKnownHostsFile=/dev/null", "-o", "HostKeyAlias=gitust-rehearsal",
                   "-o", "UpdateHostKeys=no", "-o", "VerifyHostKeyDNS=no",
                   "-o", "ProxyCommand=none", "-o", "ProxyJump=none",
                   "-o", "ClearAllForwardings=yes", "-o", "ConnectionAttempts=1",
                   "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=3",
                   "-o", "ServerAliveCountMax=2"]
        return {"GIT_SSH_COMMAND": shlex.join(command), "GIT_SSH_VARIANT": "ssh",
                "SSH_AUTH_SOCK": "", "SSH_ASKPASS": "/usr/bin/false",
                "GIT_ALLOW_PROTOCOL": "ssh:file"}

    def url(self, index):
        return f"ssh://git@127.0.0.1:{self.ssh_port}/{self.f.ids[index]}.git"

    def start(self):
        with socket.socket() as http, socket.socket() as ssh:
            http.bind(("127.0.0.1", 0))
            ssh.bind(("127.0.0.1", 0))
            http_port, self.ssh_port = http.getsockname()[1], ssh.getsockname()[1]
        arguments = [str(self.f.binary), "serve", "--bind", f"127.0.0.1:{http_port}",
                     "--ssh-bind", f"127.0.0.1:{self.ssh_port}", "--repo", str(self.f.repos[0]),
                     "--auth-db", str(self.f.auth), "--sync-github", str(self.upstream),
                     "--read-only-repo", str(self.f.repos[1])]
        process = subprocess.Popen(arguments, env=self.h.clean_environment(),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.f.processes.append(process)
        self.f.process, self.f.url = process, f"http://127.0.0.1:{http_port}"
        self.f.healthy(process, self.f.url)
        self.f.admin.url = self.f.url
        for client, _ in self.f.members:
            client.url = self.f.url

    def advertise(self, index, key):
        text = self.h.git(self.scratch, "ls-remote", "--refs", self.url(index), env=self.environment(key))
        actual = {}
        for line in text.splitlines():
            oid, name = line.split("\t")
            self.require(name not in actual and re.fullmatch(r"[0-9a-f]{40}", oid),
                         "invalid SSH advertised ref")
            actual[name] = oid
        expected = refs(self.h.git(self.f.repos[index], "for-each-ref", "--format=%(refname)%00%(objectname)"))
        expected = {name: oid for name, oid in expected.items() if not name.startswith("refs/remotes/")}
        self.require(actual == expected, "SSH advertised refs differ from native raw refs")

    def verify_clone(self, index, clone, revision="HEAD"):
        repo = self.f.repos[index]
        self.require(self.h.git(clone, "rev-parse", revision) == self.h.git(repo, "rev-parse", "HEAD")
                     and self.h.git(clone, "symbolic-ref", "HEAD") == self.h.git(repo, "symbolic-ref", "HEAD")
                     and self.h.git(clone, "ls-tree", "-r", "-z", revision)
                     == self.h.git(repo, "ls-tree", "-r", "-z", "HEAD")
                     and self.h.git(clone, "show", revision + ":marker.txt") == f"repository-{index}-after",
                     "SSH checkout resolved another repository, tree or default commit")
        tags = refs(self.h.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)", "refs/tags"))
        self.require(tags and tags == refs(self.h.git(clone, "for-each-ref", "--format=%(refname)%00%(objectname)",
                                                     "refs/tags")), "SSH clone/fetch lost raw tag refs")
        for oid in tags.values():
            self.require(self.h.git(repo, "cat-file", "-t", oid) == "tag"
                         and self.h.git(clone, "cat-file", "-t", oid) == "tag",
                         "SSH transfer peeled an annotated tag")
        heads = refs(self.h.git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)", "refs/heads"))
        remote = refs(self.h.git(clone, "for-each-ref", "--format=%(refname)%00%(objectname)",
                                "refs/remotes/origin"))
        self.require({name.replace("refs/heads/", "refs/remotes/origin/", 1): oid
                      for name, oid in heads.items()}
                     == {name: oid for name, oid in remote.items() if name != "refs/remotes/origin/HEAD"},
                     "SSH transfer omitted or substituted a branch")
        self.require(self.h.git(clone, "symbolic-ref", "refs/remotes/origin/HEAD")
                     == "refs/remotes/origin/" + self.h.git(repo, "symbolic-ref", "--short", "HEAD"),
                     "SSH clone did not preserve the advertised default branch")

    def clone(self, index, name):
        clone = self.scratch / name
        self.h.git(self.scratch, "clone", self.url(index), str(clone), env=self.environment(self.admin_key))
        self.verify_clone(index, clone)
        self.clones.append(clone)
        return clone

    def deny(self, index, key, *, url=None, push=None):
        arguments = ["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"]
        arguments += (["-C", str(push), "push", self.url(index), "HEAD:refs/heads/ssh-must-not-write"]
                      if push else ["ls-remote", "--refs", url or self.url(index)])
        result = subprocess.run(arguments, env=self.h.clean_environment(self.environment(key)),
                                capture_output=True, timeout=90)
        self.require(result.returncode in (1, 128), "a denied SSH operation did not fail normally")
        if push is None:
            self.require(result.stdout == b"", "a denied SSH read emitted repository refs")
        self.advertise(index, self.admin_key)
        self.unchanged()

    def prepare(self):
        self.h.git(self.f.root, "init", "--bare", str(self.f.root / "unused-upstream.git"))
        self.f.setup()
        self.f.stop(self.f.process)
        self.h.git(self.f.repos[0], "tag", "-a", "ssh-initial", "-m", "Preserve this raw annotated tag")
        self.states = [self.state(repo) for repo in self.f.repos]
        self.identities = [self.anchors(repo) for repo in self.f.repos]
        host = self.common(self.f.repos[0]) / "gitust-ssh"
        host.mkdir(mode=0o700)
        public = self.generate(host / "host_key")
        self.known_hosts = self.keys / "known_hosts"
        self.known_hosts.write_text("gitust-rehearsal " + " ".join(public.split()[:2]) + "\n")
        self.known_hosts.chmod(0o600)
        self.start()
        self.admin_key, _ = self.register_key(self.f.admin, "operator")
        self.primary_key, self.primary_key_id = self.register_key(self.f.members[0][0], "primary-member")
        self.secondary_key, _ = self.register_key(self.f.members[1][0], "secondary-member")
        self.unknown_key = self.keys / "unregistered"
        self.generate(self.unknown_key)
        self.require(self.states[0][2] != self.states[1][2], "fixture repository commits must differ")

    def update_stopped(self):
        self.f.stop(self.f.process)
        self.h.git(self.f.repos[0], "config", "user.name", "Gitust SSH Rehearsal")
        self.h.git(self.f.repos[0], "config", "user.email", "ssh@company.internal")
        for index, repo in enumerate([self.f.repos[0], self.f.root / "secondary-seed"]):
            (repo / "ssh-next.txt").write_text(f"ssh incremental repository-{index}\n")
            self.h.git(repo, "add", "ssh-next.txt")
            self.h.git(repo, "commit", "-m", "test: offline SSH fetch update")
            self.h.git(repo, "tag", "-a", "ssh-next", "-m", "Preserve incremental annotated tag")
        self.h.git(self.f.repos[1], "fetch", "--no-tags", str(self.f.root / "secondary-seed"),
                   "refs/heads/main:refs/heads/main", "refs/tags/ssh-next:refs/tags/ssh-next")
        self.states = [self.state(repo) for repo in self.f.repos]
        self.start()

    def restore(self):
        self.stage = "stopped-backup-restore"
        self.f.stop(self.f.process)
        before = [self.state(repo)[:3] for repo in self.f.repos]
        snapshot, restored = self.scratch / "snapshot", self.scratch / "restored"
        tool = Path(__file__).with_name("backup.py")
        self.h.command([sys.executable, str(tool), "create", "--repo", str(self.f.repos[0]),
                        "--read-only-repo", str(self.f.repos[1]), "--auth-db", str(self.f.auth),
                        "--output", str(snapshot)])
        self.f.root.rename(self.scratch / "offline-original")
        self.require(not self.f.root.exists(), "original installation remained at its old path")
        self.h.command([sys.executable, str(tool), "restore", "--backup", str(snapshot),
                        "--destination", str(restored)])
        self.f.root = restored
        self.f.repos = [restored / "repository", restored / "read-only/0001/repository"]
        self.f.auth = restored / "auth/auth-db.json"
        host = self.common(self.f.repos[0]) / "gitust-ssh"
        self.require(stat.S_IMODE(host.stat().st_mode) == 0o700
                     and stat.S_IMODE((host / "host_key").stat().st_mode) == 0o600,
                     "restored host identity permissions changed")
        self.require([self.state(repo)[:3] for repo in self.f.repos] == before
                     and [self.anchors(repo) for repo in self.f.repos] == self.identities,
                     "restore changed refs, HEAD or repository bindings")
        self.states = [self.state(repo) for repo in self.f.repos]
        self.start()
        self.require({row["id"] for row in self.f.admin.request("GET", "/api/v1/repos")["repositories"]}
                     == set(self.f.ids), "restored service did not retain both configured UUIDs")
        for index in range(2):
            # The original known_hosts stays strict: a replaced private host key cannot pass.
            self.advertise(index, self.admin_key)
            self.clone(index, f"restored-clone-{index}")
        self.unchanged()

    def execute(self):
        self.prepare()
        self.stage = "native-clone-and-authority"
        for index in range(2):
            self.advertise(index, self.admin_key)
        self.advertise(0, self.primary_key)
        self.advertise(1, self.secondary_key)
        primary, secondary = self.clone(0, "primary-clone"), self.clone(1, "secondary-clone")
        self.deny(0, self.unknown_key)
        self.deny(1, self.primary_key)
        unhosted = f"ssh://git@127.0.0.1:{self.ssh_port}/00000000-0000-0000-0000-000000000001.git"
        self.deny(0, self.admin_key, url=unhosted)
        self.deny(0, self.admin_key, push=primary)
        self.deny(1, self.admin_key, push=secondary)
        self.stage = "offline-update-and-fetch"
        self.update_stopped()
        for index, clone in enumerate([primary, secondary]):
            self.h.git(clone, "remote", "set-url", "origin", self.url(index))
            self.h.git(clone, "fetch", "--tags", "origin", env=self.environment(self.admin_key))
            self.verify_clone(index, clone, "refs/remotes/origin/main")
            self.require(self.h.git(clone, "show", "refs/remotes/origin/main:ssh-next.txt")
                         == f"ssh incremental repository-{index}", "SSH fetch omitted new content")
        self.unchanged()
        self.stage = "sequential-current-key-and-account"
        self.f.members[0][0].request("DELETE", "/api/v1/auth/ssh-keys/" + self.primary_key_id)
        self.deny(0, self.primary_key)
        self.f.admin.request("POST", f"/api/v1/admin/users/{self.f.members[1][1]}/deactivate", {})
        self.deny(1, self.secondary_key)
        self.restore()
        self.stage = "strict-git-integrity"
        for repo in [*self.f.repos, *self.clones]:
            self.h.git(repo, "fsck", "--full", "--strict")
        return {"version": 1, "passed": True, "actual_repositories": 2, "actual_clones": len(self.clones),
                "primary_secondary_native_clone": True, "native_raw_refs_head_tree_match": True,
                "annotated_tag_objects_preserved": True, "offline_incremental_fetch": True,
                "unknown_and_unhosted_denied": True, "current_team_acl_denied": True,
                "denied_read_emits_no_refs": True,
                "sequential_revoked_key_and_inactive_account_denied": True,
                "ssh_receive_pack_denied": True, "read_operations_preserve_refs_and_registration": True,
                "strict_pinned_host_identity_across_restart_restore": True,
                "stopped_two_repository_relocation": True, "host_private_modes_preserved": True,
                "git_fsck": True, "external_network_used": False, "platform": os.uname().sysname}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/gitust"))
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    helper, multi = load("alpha-rehearsal"), load("multi-repository-rehearsal")
    script, source, binary = Path(__file__).resolve(), args.source.resolve(strict=True), args.binary.resolve(strict=True)
    helper.require(not args.report.exists() and not args.report.is_symlink(), "report already exists")
    paths = [script, script.with_name("alpha-rehearsal.py"), script.with_name("multi-repository-rehearsal.py"),
             script.with_name("backup.py")]
    runtime = {"scripts/" + path.name: helper.sha256(path) for path in paths}
    commit, dirty = helper.git(source, "rev-parse", "HEAD"), bool(helper.git(source, "status", "--porcelain"))
    binary_sha = helper.sha256(binary)
    stage = "prepare"
    with tempfile.TemporaryDirectory(prefix="gitust-ssh-read-") as directory:
        scratch = Path(directory).resolve()
        installation = scratch / "installation"
        installation.mkdir(mode=0o700)
        fixture = multi.Rehearsal(helper, binary, installation)
        runner = Rehearsal(helper, fixture, scratch)
        try:
            report = runner.execute()
        except Exception:
            stage = runner.stage
            raise RuntimeError(stage) from None
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            helper.require(all(process.poll() is not None for process in fixture.processes),
                           "owned fixture service was not reaped")
    helper.require(not scratch.exists(), "private fixture was not removed")
    helper.require(runtime == {"scripts/" + path.name: helper.sha256(path) for path in paths}
                   and helper.sha256(binary) == binary_sha, "runtime source or binary changed")
    helper.require(helper.git(source, "rev-parse", "HEAD") == commit, "provenance commit changed")
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
    print(json.dumps({"passed": True, "checks": sum(value is True for value in report.values()),
                      "actual_repositories": 2, "binary_sha256": binary_sha}))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(json.dumps({"passed": False, "error_class": type(error).__name__,
                          "stage": str(error) if isinstance(error, RuntimeError) else "setup-or-cleanup"}))
        raise SystemExit(1) from None
