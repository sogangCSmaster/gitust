#!/usr/bin/env python3
"""Offline Gitust backup/relocation for a trusted single-user installation (Unix)."""

import argparse
from contextlib import ExitStack, contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import uuid

MAX_MANIFEST_BYTES = 64 * 1024 * 1024
MAX_REPOSITORIES = 32


def git(repo, *args):
    result = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
         "-C", str(repo), *args], capture_output=True, check=True,
        env={**{key: value for key, value in os.environ.items() if not key.startswith("GIT_")},
             "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"},
        timeout=300,
    )
    return result.stdout.decode("utf-8", "surrogateescape").rstrip("\n")


@contextmanager
def exclusive_lock(path):
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError("lock must be a regular file; stop all writers first")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ValueError("stop Gitust and all auth/Git writers before backup") from exc
        yield
    finally:
        os.close(fd)


def require_no_unconfirmed_execution(common):
    path = common / "gitust-active-executions"
    try:
        fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return
    try:
        if stat.S_IMODE(os.fstat(fd).st_mode) != 0o700:
            raise ValueError("execution fence directory must be private")
        with os.scandir(fd) as entries:
            if next(entries, None) is not None:
                raise ValueError("unconfirmed sandbox execution: stop prior commands and explicitly recover before backup")
    finally:
        os.close(fd)


def inside(path, parent):
    return path == parent or parent in path.parents


def regular(path):
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("expected a regular file")


def layout(repo):
    repo = repo.resolve(strict=True)
    bare = git(repo, "rev-parse", "--is-bare-repository") == "true"
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    expected = repo if bare else repo / ".git"
    if common.resolve() != expected or expected.is_symlink():
        raise ValueError("use a main checkout or bare repository, without external common directories")
    if not bare and Path(git(repo, "rev-parse", "--show-toplevel")).resolve() != repo:
        raise ValueError("--repo must name the main checkout root")
    alternates = common / "objects/info/alternates"
    if alternates.exists() and alternates.stat().st_size:
        raise ValueError("external Git object alternates require a separate complete backup")
    config = git(repo, "config", "--null", "--list").split("\0")
    if any(item.split("\n", 1)[0].lower() == "extensions.partialclone"
           or (item.split("\n", 1)[0].lower().endswith(".promisor")
               and item.partition("\n")[2].lower() != "false") for item in config if item):
        raise ValueError("partial clones require a complete local object store before backup")
    base = repo / "gitust-worktrees" if bare else repo.parent / f".{repo.name}.gitust-worktrees"
    if base.is_symlink():
        raise ValueError("managed worktree base must be a real directory")
    return repo, common, base, bare


def validate_git_storage(common, base, bare):
    # Git permits symlinked objects/refs/config. Preserving those links would make a
    # supposedly independent backup depend on the original deployment's storage.
    for directory, dirs, files in os.walk(common, followlinks=False):
        for name in dirs + files:
            path = Path(directory) / name
            if bare and path == base:
                if path.is_symlink():
                    raise ValueError("managed worktree base must be a real directory")
                dirs.remove(name)
                continue
            if path.is_symlink():
                raise ValueError("symlinked Git metadata/storage requires a separate complete backup")


def linked_worktrees(repo, common, base, bare):
    linked = []
    for record in git(repo, "worktree", "list", "--porcelain", "-z").split("\0\0"):
        fields = record.split("\0")
        if not fields[0]:
            continue
        checkout = Path(fields[0].removeprefix("worktree "))
        if checkout == repo:
            continue
        if checkout.parent != base or checkout.is_symlink():
            raise ValueError("external/custom worktrees are unsupported; stop and back up the complete layout")
        uuid.UUID(checkout.name)
        pointer = checkout / ".git"
        regular(pointer)
        raw = pointer.read_text().strip()
        if not raw.startswith("gitdir: "):
            raise ValueError("invalid linked worktree pointer")
        admin = Path(raw.removeprefix("gitdir: "))
        if not admin.is_absolute():
            admin = checkout / admin
        admin = admin.resolve(strict=True)
        if admin.parent != common / "worktrees":
            raise ValueError("linked worktree points outside repository administration")
        for name in ["gitdir", "commondir"]:
            regular(admin / name)
        reverse = Path((admin / "gitdir").read_text().strip())
        if not reverse.is_absolute():
            reverse = admin / reverse
        if reverse.resolve(strict=True) != pointer.resolve():
            raise ValueError("worktree reverse pointer mismatch")
        if (admin / (admin / "commondir").read_text().strip()).resolve() != common:
            raise ValueError("worktree common directory mismatch")
        prefix = "repository/gitust-worktrees" if bare else ".repository.gitust-worktrees"
        linked.append({"checkout": f"{prefix}/{checkout.name}", "admin": admin.name})
    # A stale/unregistered administration entry must not quietly disappear from restoration.
    admins = common / "worktrees"
    if admins.exists() and {p.name for p in admins.iterdir()} != {w["admin"] for w in linked}:
        raise ValueError("unregistered worktree metadata; reconcile the layout before backup")
    return linked


def inventory(root):
    """Stream hashes; preserve symlinks as links, never traverse their targets."""
    result = {}
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in sorted(dirs + files):
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                result[relative] = {"link": os.readlink(path)}
            elif stat.S_ISREG(mode):
                digest = hashlib.sha256()
                with path.open("rb") as source:
                    for chunk in iter(lambda: source.read(1024 * 1024), b""):
                        digest.update(chunk)
                result[relative] = {"sha256": digest.hexdigest(), "mode": stat.S_IMODE(mode)}
            elif stat.S_ISDIR(mode):
                result[relative] = {"directory": True, "mode": stat.S_IMODE(mode)}
            else:
                raise ValueError("special files cannot be included in an offline snapshot")
    return result


def new_directory(path):
    if not path.parent.is_dir():
        raise ValueError("destination parent must already exist")
    path.mkdir(mode=0o700)


def repository_path(index):
    return Path("repository") if index == 0 else Path(f"read-only/{index:04d}/repository")


def relocated_linked(linked, index):
    parent = repository_path(index).parent
    return [{"checkout": (parent / row["checkout"]).as_posix(), "admin": row["admin"]}
            for row in linked]


def source_layouts(args, auth, target):
    sources = [args.repo, *getattr(args, "read_only_repo", [])]
    if len(sources) > MAX_REPOSITORIES:
        raise ValueError("at most 32 repositories may share one backup checkpoint")
    multi = len(sources) > 1
    if multi and auth.stat().st_nlink != 1:
        raise ValueError("multi-repository auth database must not have hardlink aliases")
    if multi and any(source.is_symlink() for source in sources):
        raise ValueError("repository roots must not be symlinks")
    layouts = [layout(source) for source in sources]
    roots = []
    identities = set()
    for repo, common, base, bare in layouts:
        identity = (common.stat().st_dev, common.stat().st_ino)
        if identity in identities:
            raise ValueError("duplicate Git common directory")
        identities.add(identity)
        coverage = [repo, *([] if bare else [base.resolve()])]
        if any(inside(path, other) or inside(other, path) for path in coverage for other in roots):
            raise ValueError("source repository and managed-worktree trees must not overlap")
        roots.extend(coverage)
    if multi and any(inside(path, root) for root in roots
                     for path in [auth, Path(str(auth) + ".lock").resolve()]):
        raise ValueError("multi-repository auth database and lock must be outside every source tree")
    if any(inside(target, parent) or inside(parent, target) for parent in [*roots, auth]):
        raise ValueError("backup output must be separate from all source paths")
    if multi and args.output.is_symlink():
        raise ValueError("backup output must not be a symlink")
    return layouts


def validate_auth_history(auth):
    regular(auth)
    if auth.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("auth snapshot exceeds size limit")
    snapshot = json.loads(auth.read_text())
    if not isinstance(snapshot, dict):
        raise ValueError("auth snapshot must be an object")
    checkpoint = snapshot.get("sqlite_checkpoint")
    prs = snapshot.get("pull_requests", [])
    audit = snapshot.get("audit_log", [])
    required = checkpoint is not None or len(prs) == 64 or (audit and audit[0]["seq"] > 1)
    sidecar = Path(str(auth) + ".sqlite3")
    if required:
        regular(sidecar)
    if checkpoint is None:
        return
    if not isinstance(checkpoint, dict) or checkpoint.get("version") != 1:
        raise ValueError("unsupported auth SQLite checkpoint")
    # The surrounding auth lock excludes all writers, including SQLite WAL publication.
    with tempfile.TemporaryDirectory(prefix="gitust-auth-history-check-") as temporary:
        copied = Path(temporary) / "history.sqlite3"
        shutil.copyfile(sidecar, copied)
        wal = Path(str(sidecar) + "-wal")
        if wal.exists() or wal.is_symlink():
            regular(wal)
            shutil.copyfile(wal, Path(str(copied) + "-wal"))
        with sqlite3.connect(copied.as_uri() + "?mode=ro", uri=True) as db:
            for table, count_key, digest_key in [
                ("pull_requests", "pull_request_count", "pull_requests_sha256"),
                ("audit_log", "audit_log_count", "audit_log_sha256"),
            ]:
                rows = db.execute(f"SELECT payload_json FROM {table} ORDER BY seq ASC").fetchall()
                payload = ("[" + ",".join(row[0] for row in rows) + "]").encode()
                if (len(rows) != checkpoint[count_key]
                        or hashlib.sha256(payload).hexdigest() != checkpoint[digest_key]):
                    raise ValueError("auth SQLite history does not match its committed checkpoint")

def create(args):
    auth = args.auth_db.absolute()
    regular(auth)
    auth = auth.resolve(strict=True)
    target = args.output.resolve()
    layouts = source_layouts(args, auth, target)
    with ExitStack() as locks:
        for common in sorted((entry[1] for entry in layouts), key=str):
            locks.enter_context(exclusive_lock(common / "gitust-worktree-service.lock"))
        for _, common, _, _ in layouts:
            require_no_unconfirmed_execution(common)
        locks.enter_context(exclusive_lock(Path(str(auth) + ".lock")))
        validate_auth_history(auth)
        records = []
        for index, (repo, common, base, bare) in enumerate(layouts):
            if layout(repo) != (repo, common, base, bare):
                raise ValueError("source repository layout changed while acquiring ownership")
            validate_git_storage(common, base, bare)
            linked = linked_worktrees(repo, common, base, bare)
            git(repo, "fsck", "--full", "--strict")
            inventory(repo)
            if not bare and base.exists():
                inventory(base)
            records.append({"bare": bare, "linked": relocated_linked(linked, index)})
        new_directory(target)
        try:
            if len(layouts) > 1:
                (target / "read-only").mkdir(mode=0o700)
            for index, (repo, _, base, bare) in enumerate(layouts):
                destination = target / repository_path(index)
                if index:
                    destination.parent.mkdir(mode=0o700)
                shutil.copytree(repo, destination, symlinks=True)
                if not bare and base.exists():
                    shutil.copytree(base, destination.parent / ".repository.gitust-worktrees", symlinks=True)
            (target / "auth").mkdir(mode=0o700)
            shutil.copyfile(auth, target / "auth/auth-db.json")
            (target / "auth/auth-db.json").chmod(0o600)
            for suffix in (".sqlite3", ".sqlite3-wal"):
                sidecar = Path(str(auth) + suffix)
                if sidecar.exists() or sidecar.is_symlink():
                    regular(sidecar)
                    dest_sidecar = target / f"auth/auth-db.json{suffix}"
                    shutil.copyfile(sidecar, dest_sidecar)
                    dest_sidecar.chmod(0o600)
            manifest = ({"version": 1, **records[0]} if len(layouts) == 1
                        else {"version": 2, "repositories": records})
            manifest["files"] = inventory(target)
            # One completion marker binds all payloads to the same locked auth checkpoint.
            with (target / "manifest.json").open("x") as out:
                json.dump(manifest, out, sort_keys=True)
                if out.tell() > MAX_MANIFEST_BYTES:
                    raise ValueError("manifest exceeds the restore reader size limit")
                out.flush()
                os.fsync(out.fileno())
        except BaseException:
            shutil.rmtree(target)
            raise
    print("Backup complete. Store this private directory securely; it contains credentials and code.")


def relative_path(raw):
    path = Path(raw)
    if not raw or path.is_absolute() or any(part in (".", "..") for part in path.parts):
        raise ValueError("invalid snapshot path")
    return path


def real_directory_chain(path, root):
    for candidate in [path, *path.parents]:
        if candidate.is_symlink() or not candidate.is_dir():
            raise ValueError("snapshot metadata directories must be real directories")
        if candidate == root:
            return
    raise ValueError("snapshot path is outside the restore destination")


def manifest_repositories(manifest):
    if not isinstance(manifest, dict):
        raise ValueError("snapshot manifest must be an object")
    if manifest.get("version") == 1 and type(manifest.get("bare")) is bool:
        return [{"bare": manifest["bare"], "linked": manifest["linked"]}]
    if (type(manifest.get("version")) is not int or manifest["version"] != 2
            or set(manifest) != {"version", "repositories", "files"}):
        raise ValueError("unsupported snapshot version/layout")
    records = manifest["repositories"]
    if not isinstance(records, list) or not 2 <= len(records) <= MAX_REPOSITORIES:
        raise ValueError("invalid repository checkpoint count")
    for index, record in enumerate(records):
        if (not isinstance(record, dict) or set(record) != {"bare", "linked"}
                or type(record["bare"]) is not bool or not isinstance(record["linked"], list)):
            raise ValueError("invalid repository checkpoint layout")
        expected = repository_path(index)
        prefix = (expected / "gitust-worktrees" if record["bare"] else
                  expected.parent / ".repository.gitust-worktrees")
        checkouts, admins = set(), set()
        for linked in record["linked"]:
            if (not isinstance(linked, dict) or set(linked) != {"checkout", "admin"}
                    or not all(isinstance(value, str) for value in linked.values())):
                raise ValueError("invalid managed worktree record")
            relative, admin = relative_path(linked["checkout"]), relative_path(linked["admin"])
            if (relative.parent != prefix or linked["checkout"] != relative.as_posix()
                    or len(admin.parts) != 1 or linked["admin"] != admin.as_posix()
                    or relative in checkouts or admin in admins):
                raise ValueError("managed worktree record crosses its repository layout")
            uuid.UUID(relative.name)
            checkouts.add(relative)
            admins.add(admin)
    return records


def restore(args):
    backup = args.backup.resolve(strict=True)
    regular(backup / "manifest.json")
    if (backup / "manifest.json").stat().st_size > MAX_MANIFEST_BYTES:
        raise ValueError("manifest exceeds 64 MiB")
    manifest = json.loads((backup / "manifest.json").read_text())
    records = manifest_repositories(manifest)
    if manifest["version"] == 2 and (args.backup.is_symlink() or args.destination.is_symlink()):
        raise ValueError("multi-repository backup and destination roots must not be symlinks")
    for raw in manifest["files"]:
        relative_path(raw)
    actual = inventory(backup)
    actual.pop("manifest.json")
    if actual != manifest["files"]:
        raise ValueError("snapshot integrity check failed")
    payloads = [Path("auth")]
    for index, record in enumerate(records):
        repo = repository_path(index)
        payloads.append(repo)
        if not record["bare"] and (backup / repo.parent / ".repository.gitust-worktrees").exists():
            payloads.append(repo.parent / ".repository.gitust-worktrees")
    if manifest["version"] == 2:
        containers = {parent for root in payloads for parent in root.parents if parent != Path(".")}
        for raw in manifest["files"]:
            relative = relative_path(raw)
            if (raw != relative.as_posix() or
                    not (relative in containers or any(inside(relative, root) for root in payloads))):
                raise ValueError("snapshot contains payload outside its fixed repository slots")
    for relative in payloads:
        real_directory_chain(backup / relative, backup)
    validate_auth_history(backup / "auth/auth-db.json")
    for suffix in (".sqlite3", ".sqlite3-wal"):
        sidecar = backup / f"auth/auth-db.json{suffix}"
        if sidecar.exists() or sidecar.is_symlink():
            regular(sidecar)
    target = args.destination.resolve()
    if inside(target, backup) or inside(backup, target):
        raise ValueError("restore destination must be separate from backup")
    new_directory(target)
    try:
        if len(records) > 1:
            (target / "read-only").mkdir(mode=0o700)
        for relative in payloads:
            destination = target / relative
            if relative.parts[0] == "read-only" and not destination.parent.exists():
                destination.parent.mkdir(mode=0o700)
            shutil.copytree(backup / relative, destination, symlinks=True)
        for index, record in enumerate(records):
            repo = target / repository_path(index)
            common = repo if record["bare"] else repo / ".git"
            real_directory_chain(common, target)
            base = repo / "gitust-worktrees" if record["bare"] else repo.parent / ".repository.gitust-worktrees"
            validate_git_storage(common, base, record["bare"])
            expected_prefix = base.relative_to(target)
            for linked in record["linked"]:
                relative = relative_path(linked["checkout"])
                if relative.parent != expected_prefix:
                    raise ValueError("unsupported managed worktree destination")
                uuid.UUID(relative.name)
                admin_name = relative_path(linked["admin"])
                if len(admin_name.parts) != 1:
                    raise ValueError("invalid worktree administration name")
                checkout, admin = target / relative, common / "worktrees" / admin_name
                real_directory_chain(checkout, target)
                real_directory_chain(admin, target)
                for path in [checkout / ".git", admin / "gitdir", admin / "commondir"]:
                    regular(path)
                # Relink every copy before any Git command can follow a source pointer.
                (checkout / ".git").write_text(f"gitdir: {admin}\n")
                (admin / "gitdir").write_text(f"{checkout / '.git'}\n")
                (admin / "commondir").write_text("../..\n")
        (target / "auth").chmod(0o700)
        (target / "auth/auth-db.json").chmod(0o600)
        for suffix in (".sqlite3", ".sqlite3-wal"):
            restored_sidecar = target / f"auth/auth-db.json{suffix}"
            if restored_sidecar.exists():
                restored_sidecar.chmod(0o600)
        for index, record in enumerate(records):
            repo, common, base, bare = layout(target / repository_path(index))
            if relocated_linked(linked_worktrees(repo, common, base, bare), index) != record["linked"]:
                raise ValueError("restored worktree inventory differs from the snapshot")
            git(repo, "fsck", "--full", "--strict")
            for linked in record["linked"]:
                git(target / linked["checkout"], "fsck", "--full", "--strict")
        text = "Restart Gitust with repository/ and auth/auth-db.json; reverify pending work.\n"
        if len(records) > 1:
            text += "Configure read-only repositories explicitly: " + ", ".join(
                repository_path(index).as_posix() for index in range(1, len(records))) + "\n"
        (target / "RESTORED").write_text(text)
    except BaseException:
        shutil.rmtree(target)
        raise
    print("Restore complete. Configure the upstream and restart Gitust; pending work requires fresh verification.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    backup = sub.add_parser("create", help="stop every writer, then take a private offline snapshot")
    backup.add_argument("--repo", type=Path, required=True)
    backup.add_argument("--read-only-repo", type=Path, action="append", default=[])
    backup.add_argument("--auth-db", type=Path, required=True)
    backup.add_argument("--output", type=Path, required=True)
    restored = sub.add_parser("restore", help="restore into a new, absent directory")
    restored.add_argument("--backup", type=Path, required=True)
    restored.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        (create if args.action == "create" else restore)(args)
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error, subprocess.SubprocessError) as exc:
        # Do not echo Git stderr/config/JSON: they may contain credentials or private content.
        print(f"Backup/restore failed ({type(exc).__name__}): "
              + (str(exc) if isinstance(exc, ValueError) else "check paths, permissions and stopped writers"),
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
