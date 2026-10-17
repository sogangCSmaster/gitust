import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import test_backup as baseline

SCRIPT, git = baseline.SCRIPT, baseline.git


class MultiBackupTests(unittest.TestCase):
    setUp = baseline.BackupTests.setUp
    tearDown = baseline.BackupTests.tearDown
    run_script = baseline.BackupTests.run_script
    restore = baseline.BackupTests.restore

    def secondary(self):
        repo = self.root / "secondary.git"
        git(self.root, "clone", "--bare", str(self.repo), str(repo))
        return repo

    def create_many(self, additional, ok=True):
        arguments = ["create", "--repo", self.repo, "--auth-db", self.auth,
                     "--output", self.backup]
        for repo in additional:
            arguments.extend(["--read-only-repo", repo])
        return self.run_script(*arguments, ok=ok)

    def test_one_checkpoint_relocates_normal_and_bare_with_originals_unavailable(self):
        secondary = self.secondary()
        secondary_wt = secondary / "gitust-worktrees/AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"
        git(secondary, "worktree", "add", "-b", "agent/secondary/backup", str(secondary_wt), "main")
        (secondary_wt / "unfinished.txt").write_text("secondary dirty\n")
        pointers = [(path / ".git").read_bytes() for path in [self.wt, secondary_wt]]
        refs = [git(repo, "show-ref") for repo in [self.repo, secondary]]
        auth = self.auth.read_bytes()
        self.create_many([secondary])
        manifest = json.loads((self.backup / "manifest.json").read_text())
        self.assertEqual(manifest["version"], 2)
        self.assertEqual(len(manifest["repositories"]), 2)
        self.assertEqual((self.backup / "auth/auth-db.json").read_bytes(), auth)
        originals = [self.repo, self.worktrees, secondary, self.auth]
        hidden = [self.root / ("unavailable-" + str(i)) for i in range(len(originals))]
        for source, target in zip(originals, hidden):
            source.rename(target)
        try:
            self.restore()
            main = self.restored / "repository"
            restored_secondary = self.restored / "read-only/0001/repository"
            primary_wt = self.restored / ".repository.gitust-worktrees" / self.wt_id
            readonly_wt = restored_secondary / "gitust-worktrees" / secondary_wt.name
            for index, repo in enumerate([main, restored_secondary]):
                self.assertEqual(git(repo, "show-ref"), refs[index])
                git(repo, "fsck", "--full", "--strict")
            for path in [primary_wt, readonly_wt]:
                git(path, "fsck", "--full", "--strict")
                common = Path(git(path, "rev-parse", "--path-format=absolute", "--git-common-dir"))
                self.assertTrue(common.is_relative_to(self.restored))
            self.assertEqual((primary_wt / "dirty file.txt").read_text(), "uncommitted work\n")
            self.assertTrue((primary_wt / "local-link").is_symlink())
            self.assertEqual((readonly_wt / "unfinished.txt").read_text(), "secondary dirty\n")
            self.assertEqual((self.restored / "auth/auth-db.json").read_bytes(), auth)
            self.assertTrue((self.restored / "RESTORED").is_file())
        finally:
            for source, target in zip(hidden, originals):
                source.rename(target)
        self.assertEqual([(path / ".git").read_bytes() for path in [self.wt, secondary_wt]], pointers)

    def test_secondary_admission_rejects_aliases_nested_paths_private_dependencies_and_busy_locks(self):
        secondary = self.secondary()
        alias = self.root / "secondary-alias"
        alias.symlink_to(secondary, target_is_directory=True)
        nested = self.repo / "nested.git"
        git(self.repo, "clone", "--bare", str(self.repo), str(nested))
        for additional in [[secondary, secondary], [self.repo], [alias], [nested], [secondary] * 32]:
            with self.subTest(additional=len(additional)):
                result = self.create_many(additional, ok=False)
                if len(additional) == 32:
                    self.assertIn("at most 32 repositories", result.stderr)
                self.assertFalse(self.backup.exists())
        fence = secondary / "gitust-active-executions"
        fence.mkdir(mode=0o700)
        (fence / "incomplete.json").write_text("operator inspection required")
        self.create_many([secondary], ok=False)
        self.assertFalse(self.backup.exists())
        (fence / "incomplete.json").unlink()
        for lock in [secondary / "gitust-worktree-service.lock", Path(str(self.auth) + ".lock")]:
            with lock.open("w") as held:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.create_many([secondary], ok=False)
                self.assertFalse(self.backup.exists())
        alternates = secondary / "objects/info/alternates"
        alternates.write_text(str(self.common / "objects") + "\n")
        self.create_many([secondary], ok=False)
        self.assertFalse(self.backup.exists())
        alternates.unlink()
        git(secondary, "config", "remote.origin.promisor", "true")
        self.create_many([secondary], ok=False)
        self.assertFalse(self.backup.exists())
        git(secondary, "config", "--unset", "remote.origin.promisor")
        refs, saved_refs = secondary / "refs", self.root / "secondary-refs"
        refs.rename(saved_refs)
        refs.symlink_to(saved_refs, target_is_directory=True)
        self.create_many([secondary], ok=False)
        self.assertFalse(self.backup.exists())
        refs.unlink()
        saved_refs.rename(refs)
        original_auth = self.auth
        self.auth = self.repo / "private-auth.json"
        self.auth.write_bytes(original_auth.read_bytes())
        self.create_many([secondary], ok=False)
        self.assertFalse(self.backup.exists())
        self.auth = original_auth
        alias = self.repo / "auth-hardlink.json"
        os.link(self.auth, alias)
        self.create_many([secondary], ok=False)
        self.assertFalse(self.backup.exists())

    def test_v2_schema_and_cross_repository_worktree_pointer_are_not_authority(self):
        secondary = self.secondary()
        third = self.root / "third.git"
        git(self.root, "clone", "--bare", str(self.repo), str(third))
        self.create_many([secondary, third])
        manifest_path = self.backup / "manifest.json"
        original = manifest_path.read_text()
        variants = [
            lambda m: m.update(repositories=m["repositories"] * 17),
            lambda m: m["repositories"][1].update(bare=1),
            lambda m: m["repositories"][1].update(path="../../source"),
            lambda m: m["repositories"].pop(),
            lambda m: m["repositories"][0]["linked"][0].update(
                checkout="read-only/0001/repository/gitust-worktrees/" + self.wt_id),
        ]
        for change in variants:
            with self.subTest(change=change):
                manifest = json.loads(original)
                change(manifest)
                manifest_path.write_text(json.dumps(manifest))
                self.restore(ok=False)
                self.assertFalse(self.restored.exists())
        manifest_path.write_text(original)
        extra = self.backup / "unrecognized-payload.txt"
        extra.write_bytes(b"must not be silently dropped")
        manifest = json.loads(original)
        manifest["files"][extra.name] = {
            "sha256": hashlib.sha256(extra.read_bytes()).hexdigest(),
            "mode": extra.stat().st_mode & 0o777,
        }
        manifest_path.write_text(json.dumps(manifest))
        self.restore(ok=False)
        self.assertFalse(self.restored.exists())
        extra.unlink()
        manifest_path.write_text(original)
        self.restore()

    def test_copy_failure_keeps_all_source_locks_until_cleanup_and_publishes_no_checkpoint(self):
        secondary = self.secondary()
        spec = importlib.util.spec_from_file_location("backup_under_test", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        original_copy = module.shutil.copytree
        seen = []

        def copy(source, target, *args, **kwargs):
            if Path(source) == secondary:
                for lock in [self.common / "gitust-worktree-service.lock",
                             secondary / "gitust-worktree-service.lock", Path(str(self.auth) + ".lock")]:
                    with lock.open("r+") as held:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    seen.append(lock)
                raise OSError("synthetic second repository copy failure")
            return original_copy(source, target, *args, **kwargs)

        args = SimpleNamespace(repo=self.repo, read_only_repo=[secondary], auth_db=self.auth,
                               output=self.backup)
        with patch.object(module.shutil, "copytree", side_effect=copy):
            with self.assertRaises(OSError):
                module.create(args)
        self.assertEqual(len(seen), 3)
        self.assertFalse(self.backup.exists())
        self.create_many([secondary])


if __name__ == "__main__":
    unittest.main()
