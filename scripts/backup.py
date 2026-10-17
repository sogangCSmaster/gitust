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
import stat
import subprocess
import sys
import uuid

MAX_MANIFEST_BYTES = 64 * 1024 * 1024


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


def create(args):
    repo, common, base, bare = layout(args.repo)
    auth = args.auth_db.absolute()
    regular(auth)
    auth = auth.resolve(strict=True)
    target = args.output.resolve()
    if any(inside(target, parent) or inside(parent, target) for parent in [repo, base, auth]):
        raise ValueError("backup output must be separate from all source paths")
    with ExitStack() as locks:
        locks.enter_context(exclusive_lock(common / "gitust-worktree-service.lock"))
        locks.enter_context(exclusive_lock(Path(str(auth) + ".lock")))
        validate_git_storage(common, base, bare)
        linked = linked_worktrees(repo, common, base, bare)
        git(repo, "fsck", "--full", "--strict")
        # Check special files before creating output; directory copies retain ignored/dirty files.
        inventory(repo)
        if not bare and base.exists():
            inventory(base)
        new_directory(target)
        try:
            shutil.copytree(repo, target / "repository", symlinks=True)
            if not bare and base.exists():
                shutil.copytree(base, target / ".repository.gitust-worktrees", symlinks=True)
            (target / "auth").mkdir(mode=0o700)
            shutil.copyfile(auth, target / "auth/auth-db.json")
            (target / "auth/auth-db.json").chmod(0o600)
            manifest = {"version": 1, "bare": bare, "linked": linked,
                        "files": inventory(target)}
            # A manifest is the completion marker. An interrupted directory is not restorable.
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


def restore(args):
    backup = args.backup.resolve(strict=True)
    regular(backup / "manifest.json")
    if (backup / "manifest.json").stat().st_size > MAX_MANIFEST_BYTES:
        raise ValueError("manifest exceeds 64 MiB")
    manifest = json.loads((backup / "manifest.json").read_text())
    if manifest.get("version") != 1 or type(manifest.get("bare")) is not bool:
        raise ValueError("unsupported snapshot version/layout")
    for raw in manifest["files"]:
        relative_path(raw)
    actual = inventory(backup)
    actual.pop("manifest.json")
    if actual != manifest["files"]:
        raise ValueError("snapshot integrity check failed")
    for path in [backup / "repository", backup / "auth"]:
        if not path.is_dir() or path.is_symlink():
            raise ValueError("snapshot payload roots must be real directories")
    regular(backup / "auth/auth-db.json")
    target = args.destination.resolve()
    if inside(target, backup) or inside(backup, target):
        raise ValueError("restore destination must be separate from backup")
    new_directory(target)
    try:
        for name in ["repository", "auth", ".repository.gitust-worktrees"]:
            source = backup / name
            if source.exists():
                if source.is_symlink():
                    raise ValueError("snapshot payload roots must not be symlinks")
                shutil.copytree(source, target / name, symlinks=True)
        common = target / "repository" if manifest["bare"] else target / "repository/.git"
        real_directory_chain(common, target)
        base = (target / "repository/gitust-worktrees" if manifest["bare"]
                else target / ".repository.gitust-worktrees")
        validate_git_storage(common, base, manifest["bare"])
        expected_prefix = "repository/gitust-worktrees" if manifest["bare"] else ".repository.gitust-worktrees"
        for linked in manifest["linked"]:
            relative = relative_path(linked["checkout"])
            if relative.parent.as_posix() != expected_prefix:
                raise ValueError("unsupported managed worktree destination")
            uuid.UUID(relative.name)
            admin_name = relative_path(linked["admin"])
            if len(admin_name.parts) != 1:
                raise ValueError("invalid worktree administration name")
            checkout = target / relative
            admin = common / "worktrees" / admin_name
            real_directory_chain(checkout, target)
            real_directory_chain(admin, target)
            for path in [checkout / ".git", admin / "gitdir", admin / "commondir"]:
                regular(path)
            # Rewrite only the destination copy. Git repair can follow an old .git pointer
            # back into a still-existing original checkout; never invoke it before relinking.
            (checkout / ".git").write_text(f"gitdir: {admin}\n")
            (admin / "gitdir").write_text(f"{checkout / '.git'}\n")
            (admin / "commondir").write_text("../..\n")
        (target / "auth").chmod(0o700)
        (target / "auth/auth-db.json").chmod(0o600)
        repo, common, base, bare = layout(target / "repository")
        if linked_worktrees(repo, common, base, bare) != manifest["linked"]:
            raise ValueError("restored worktree inventory differs from the snapshot")
        git(target / "repository", "fsck", "--full", "--strict")
        for linked in manifest["linked"]:
            git(target / linked["checkout"], "fsck", "--full", "--strict")
        (target / "RESTORED").write_text("Restart Gitust with repository/ and auth/auth-db.json; reverify pending work.\n")
    except BaseException:
        shutil.rmtree(target)
        raise
    print("Restore complete. Configure the upstream and restart Gitust; pending work requires fresh verification.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    backup = sub.add_parser("create", help="stop every writer, then take a private offline snapshot")
    backup.add_argument("--repo", type=Path, required=True)
    backup.add_argument("--auth-db", type=Path, required=True)
    backup.add_argument("--output", type=Path, required=True)
    restored = sub.add_parser("restore", help="restore into a new, absent directory")
    restored.add_argument("--backup", type=Path, required=True)
    restored.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        (create if args.action == "create" else restore)(args)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as exc:
        # Do not echo Git stderr/config/JSON: they may contain credentials or private content.
        print(f"Backup/restore failed ({type(exc).__name__}): "
              + (str(exc) if isinstance(exc, ValueError) else "check paths, permissions and stopped writers"),
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
