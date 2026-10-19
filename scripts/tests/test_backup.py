import fcntl
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "backup.py"


def git(path, *args):
    result = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false", "-C", str(path), *args],
        capture_output=True, check=True,
        env={**{key: value for key, value in os.environ.items() if not key.startswith("GIT_")},
             "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"},
    )
    return result.stdout.decode().strip()


class BackupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="gitust-backup-test-")
        self.root = Path(self.tmp.name).resolve()
        self.repo = self.root / "source repository"
        self.repo.mkdir()
        git(self.repo, "init", "-b", "main")
        git(self.repo, "-c", "user.name=Test", "-c", "user.email=test@example.test",
            "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "initial")
        self.common = self.repo / ".git"
        self.wt_id = "11111111-1111-4111-8111-111111111111"
        self.worktrees = self.root / ".source repository.gitust-worktrees"
        self.wt = self.worktrees / self.wt_id
        git(self.repo, "worktree", "add", "-b", "agent/test/backup", str(self.wt), "main")
        (self.wt / "dirty file.txt").write_text("uncommitted work\n")
        (self.wt / "local-link").symlink_to("dirty file.txt")
        self.auth = self.root / "auth.json"
        self.auth.write_text(json.dumps({"version": 1, "users": []}))
        self.auth.chmod(0o600)
        self.backup = self.root / "backup"
        self.restored = self.root / "restored"

    def tearDown(self):
        self.tmp.cleanup()

    def run_script(self, *args, ok=True):
        result = subprocess.run([sys.executable, str(SCRIPT), *map(str, args)],
                                capture_output=True, text=True)
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def create(self, ok=True):
        return self.run_script("create", "--repo", self.repo, "--auth-db", self.auth,
                               "--output", self.backup, ok=ok)

    def restore(self, ok=True):
        return self.run_script("restore", "--backup", self.backup,
                               "--destination", self.restored, ok=ok)

    def test_git_fixture_helper_does_not_select_an_inherited_repository(self):
        victim = self.root / "unrelated"
        victim.mkdir()
        git(victim, "init", "-b", "main")
        git(victim, "-c", "user.name=Test", "-c", "user.email=test@example.test",
            "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "preserve")
        before = git(victim, "show-ref")
        target = self.root / "fixture target"
        code = ("import runpy, pathlib, sys; "
                "git=runpy.run_path(sys.argv[1])['git']; p=pathlib.Path(sys.argv[2]); p.mkdir(); "
                "git(p,'init','-b','main'); assert (p/'.git').is_dir(); "
                "git(p,'-c','user.name=Test','-c','user.email=test@example.test',"
                "'-c','commit.gpgsign=false','commit','--allow-empty','-m','own fixture'); "
                "git(p.parent,'clone','--bare',str(p),str(p.parent/'fixture-clone.git'))")
        env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        env.update(GIT_DIR=str(victim / ".git"), GIT_WORK_TREE=str(victim))
        result = subprocess.run([sys.executable, "-c", code, str(Path(__file__)), str(target)],
                                env=env, capture_output=True)
        self.assertEqual(result.returncode, 0, "fixture Git must ignore inherited repository selectors")
        self.assertEqual(git(victim, "show-ref"), before)
        self.assertEqual(git(victim, "status", "--porcelain"), "")

    def test_unconfirmed_execution_blocks_backup_even_after_service_lock_is_released(self):
        fences = self.common / "gitust-active-executions"
        fences.mkdir(mode=0o700)
        (fences / "incomplete.json").write_text("incomplete or corrupt evidence")
        self.create(ok=False)
        self.assertFalse(self.backup.exists())
        (fences / "incomplete.json").unlink()
        self.create()

    def test_symlinked_execution_fence_directory_is_never_treated_as_empty(self):
        target = self.root / "empty-fences"
        target.mkdir(mode=0o700)
        (self.common / "gitust-active-executions").symlink_to(target, target_is_directory=True)
        self.create(ok=False)
        self.assertFalse(self.backup.exists())

    def test_relocation_preserves_dirty_work_and_never_repoints_original(self):
        original_pointer = (self.wt / ".git").read_bytes()
        admin = Path(original_pointer.decode().strip().removeprefix("gitdir: "))
        original_reverse = (admin / "gitdir").read_bytes()
        origin = {
            "version": 1, "worktree_id": self.wt_id, "branch": "agent/test/backup",
            "base_branch": "main", "base_commit": git(self.repo, "rev-parse", "main"),
            "prompt_summary": "Keep the task label — 복구 후에도 작업 이름 유지",
        }
        origin_bytes = json.dumps(origin, ensure_ascii=False).encode()
        (admin / "gitust-origin.json").write_bytes(origin_bytes)
        (admin / "gitust-origin.json").chmod(0o600)
        (self.common / "gitust-approvals").mkdir()
        (self.common / "gitust-approvals" / "receipt.json").write_text("durable evidence")
        identity_dir = self.common / "gitust-repository"
        identity_dir.mkdir(mode=0o700)
        identity = identity_dir / "identity.json"
        identity_bytes = b'{"version":1,"repo_id":"22222222-2222-4222-8222-222222222222"}'
        identity.write_bytes(identity_bytes)
        identity.chmod(0o600)
        registration = identity_dir / "registration.json"
        registration_bytes = json.dumps({"version": 1, "repo_id": "22222222-2222-4222-8222-222222222222",
            "registration_nonce": "33333333-3333-4333-8333-333333333333"}).encode()
        registration.write_bytes(registration_bytes)
        registration.chmod(0o600)
        auth_state = json.loads(self.auth.read_text())
        auth_state["repositories"] = [{"metadata": {
            "id": "22222222-2222-4222-8222-222222222222", "name": "backup-fixture",
            "owner_team_id": "44444444-4444-4444-8444-444444444444", "is_private": True,
            "created_at": "2026-10-17T00:00:00Z"}, "binding": json.loads(registration_bytes)}]
        self.auth.write_text(json.dumps(auth_state))
        auth_before = self.auth.read_bytes()
        self.create()
        self.restore()
        destination = self.restored / "repository"
        wt = self.restored / ".repository.gitust-worktrees" / self.wt_id
        self.assertEqual(git(destination, "rev-parse", "main"), git(self.repo, "rev-parse", "main"))
        self.assertEqual(git(wt, "branch", "--show-current"), "agent/test/backup")
        self.assertEqual((wt / "dirty file.txt").read_text(), "uncommitted work\n")
        self.assertTrue((wt / "local-link").is_symlink())
        self.assertEqual((destination / ".git/gitust-approvals/receipt.json").read_text(), "durable evidence")
        restored_identity = destination / ".git/gitust-repository/identity.json"
        self.assertEqual(restored_identity.read_bytes(), identity_bytes)
        self.assertEqual(restored_identity.stat().st_mode & 0o777, 0o600)
        self.assertEqual(restored_identity.parent.stat().st_mode & 0o777, 0o700)
        restored_registration = restored_identity.with_name("registration.json")
        self.assertEqual(restored_registration.read_bytes(), registration_bytes)
        self.assertEqual(restored_registration.stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.restored / "auth/auth-db.json").read_bytes(), auth_before)
        restored_admin = Path((wt / ".git").read_text().strip().removeprefix("gitdir: "))
        self.assertEqual((restored_admin / "gitust-origin.json").read_bytes(), origin_bytes)
        self.assertEqual((restored_admin / "gitust-origin.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.wt / ".git").read_bytes(), original_pointer)
        self.assertEqual((admin / "gitdir").read_bytes(), original_reverse)
        self.assertEqual((self.restored / "auth/auth-db.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.restored.stat().st_mode & 0o777, 0o700)
        git(destination, "fsck", "--full", "--strict")
        git(wt, "fsck", "--full", "--strict")

    def test_live_service_and_auth_locks_refuse_backup(self):
        for path in [self.common / "gitust-worktree-service.lock", Path(str(self.auth) + ".lock")]:
            with path.open("w") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                result = self.create(ok=False)
                self.assertIn("stop", result.stderr.lower())
                self.assertFalse(self.backup.exists())

    def test_tampered_payload_and_existing_destination_are_rejected(self):
        self.create()
        original = (self.backup / "auth/auth-db.json").read_bytes()
        (self.backup / "auth/auth-db.json").write_text("altered")
        self.restore(ok=False)
        self.assertFalse(self.restored.exists())
        (self.backup / "auth/auth-db.json").write_bytes(original)
        self.restored.mkdir()
        marker = self.restored / "keep.txt"
        marker.write_text("keep")
        self.restore(ok=False)
        self.assertEqual(marker.read_text(), "keep")

    def test_external_worktrees_and_nested_backup_are_rejected(self):
        self.backup = self.wt / "backup"
        self.create(ok=False)
        self.assertFalse(self.backup.exists())
        self.backup = self.root / "backup"
        external = self.root / "unmanaged"
        git(self.repo, "worktree", "add", "-b", "other", str(external), "main")
        self.create(ok=False)
        self.assertFalse(self.backup.exists())

    def test_bare_repository_relocates_internal_worktrees(self):
        bare = self.root / "source.git"
        git(self.root, "clone", "--bare", str(self.repo), str(bare))
        git(bare, "worktree", "add", "-b", "agent/test/bare",
            str(bare / "gitust-worktrees" / self.wt_id), "main")
        self.repo = bare
        self.create()
        self.restore()
        wt = self.restored / "repository/gitust-worktrees" / self.wt_id
        self.assertEqual(git(wt, "branch", "--show-current"), "agent/test/bare")
        git(wt, "fsck", "--full", "--strict")

    def test_external_git_storage_symlinks_are_rejected(self):
        for name in ["objects", "refs"]:
            path = self.common / name
            external = self.root / f"external-{name}"
            path.rename(external)
            path.symlink_to(external, target_is_directory=True)
            self.create(ok=False)
            self.assertFalse(self.backup.exists())
            path.unlink()
            external.rename(path)

    def test_partial_clone_configuration_is_rejected(self):
        git(self.repo, "config", "remote.origin.promisor", "true")
        self.create(ok=False)
        self.assertFalse(self.backup.exists())

    def test_permission_damage_is_detected_before_restore(self):
        script = self.repo / "check.sh"
        script.write_text("#!/bin/sh\nexit 0\n")
        script.chmod(0o755)
        self.create()
        (self.backup / "repository/check.sh").chmod(0o644)
        self.restore(ok=False)
        self.assertFalse(self.restored.exists())

    def test_writer_rejects_a_manifest_larger_than_the_reader_limit(self):
        spec = importlib.util.spec_from_file_location("gitust_backup", SCRIPT)
        backup_module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(backup_module)
        backup_module.MAX_MANIFEST_BYTES = 128
        with self.assertRaisesRegex(ValueError, "manifest"):
            backup_module.create(SimpleNamespace(repo=self.repo, auth_db=self.auth,
                                                  output=self.backup))
        self.assertFalse(self.backup.exists())

    def test_auth_sqlite_sidecar_is_backed_up_restored_and_rejects_symlinks(self):
        sidecar = Path(str(self.auth) + ".sqlite3")
        sidecar_bytes = b"SQLite format 3\x00gitust-sidecar-payload"
        sidecar.write_bytes(sidecar_bytes)
        sidecar.chmod(0o600)
        self.create()
        self.assertEqual((self.backup / "auth/auth-db.json.sqlite3").read_bytes(), sidecar_bytes)
        self.restore()
        restored_sidecar = self.restored / "auth/auth-db.json.sqlite3"
        self.assertEqual(restored_sidecar.read_bytes(), sidecar_bytes)
        self.assertEqual(restored_sidecar.stat().st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
