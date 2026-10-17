import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git(repo, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update({"GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"})
    return subprocess.check_output(["git", "-C", str(repo), *args], env=env).decode().strip()


class OperatorScriptTests(unittest.TestCase):
    def test_rehearsal_git_ignores_an_inherited_repository(self):
        module = load("alpha-rehearsal")
        with tempfile.TemporaryDirectory() as directory:
            original, fixture = Path(directory) / "original", Path(directory) / "fixture"
            for repo in [original, fixture]:
                repo.mkdir()
                git(repo, "init", "-b", "main")
                git(repo, "config", "user.name", repo.name)
            with patch.dict(os.environ, {"GIT_DIR": str(original / ".git"),
                                         "GIT_WORK_TREE": str(original)}):
                module.git(fixture, "config", "user.name", "changed fixture")
                self.assertEqual(git(original, "config", "user.name"), "original")
                self.assertEqual(git(fixture, "config", "user.name"), "changed fixture")

    def test_platform_snapshot_ignores_an_inherited_repository(self):
        module = load("alpha-platform")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original, source, destination = root / "original", root / "source", root / "copy"
            destination.mkdir()
            for repo, filename in [(original, "other.txt"), (source, "expected.txt")]:
                repo.mkdir()
                git(repo, "init", "-b", "main")
                (repo / filename).write_text(repo.name)
                git(repo, "add", filename)
            with patch.dict(os.environ, {"GIT_DIR": str(original / ".git"),
                                         "GIT_WORK_TREE": str(original)}):
                module.snapshot(source, destination)
            self.assertEqual((destination / "expected.txt").read_text(), "source")
            self.assertFalse((destination / "other.txt").exists())

    def test_platform_snapshot_rejects_symlinked_source_parent(self):
        module = load("alpha-platform")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, external, destination = root / "source", root / "external", root / "copy"
            for path in [source, external, destination]:
                path.mkdir()
            git(source, "init", "-b", "main")
            docs = source / "docs"
            docs.mkdir()
            (docs / "private.txt").write_text("tracked source")
            git(source, "add", "docs/private.txt")
            (docs / "private.txt").unlink()
            docs.rmdir()
            (source / ".gitignore").write_text("/docs\n")
            (external / "private.txt").write_text("external sentinel")
            docs.symlink_to(external, target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, "symlink"):
                module.snapshot(source, destination)
            self.assertFalse((destination / "docs/private.txt").exists())


if __name__ == "__main__":
    unittest.main()
