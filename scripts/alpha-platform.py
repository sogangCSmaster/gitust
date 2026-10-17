#!/usr/bin/env python3
"""Validate an isolated source snapshot; never publish or use deployment state."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import secrets
import signal
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def run(args, cwd, env, log, timeout=1800, expected_tests=()):
    started = time.monotonic()
    with log.open("ab") as output:
        offset = output.tell()
        output.write(("\n$ " + " ".join(map(str, args)) + "\n").encode())
        output.flush()
        process = subprocess.Popen(args, cwd=cwd, env=env, stdout=output, stderr=output,
                                   start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
            if code:
                raise subprocess.CalledProcessError(code, args)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            raise
    if expected_tests:
        with log.open("rb") as output:
            output.seek(offset)
            result = output.read().decode("utf-8", errors="replace")
        for name in expected_tests:
            if not re.search(r"^test (?:\S*::)?" + re.escape(name) + r" \.\.\. ok$",
                             result, re.MULTILINE):
                raise RuntimeError("required test did not execute successfully: " + name)
    return round(time.monotonic() - started, 3)


def source_file(root, relative):
    source = root / relative
    for ancestor in [source, *source.parents]:
        if ancestor == root:
            break
        if ancestor.is_symlink():
            raise RuntimeError("snapshot rejects symlink source paths: " + str(relative))
    return source


def snapshot(root, destination):
    root = root.resolve(strict=True)
    paths = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=root, env={**{key: value for key, value in os.environ.items()
                         if not key.startswith("GIT_")},
                       "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}).split(b"\0")
    digest = hashlib.sha256()
    manifest = {}
    count = 0
    for raw in sorted(set(paths)):
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        if relative.is_absolute() or ".." in relative.parts:
            raise RuntimeError("unsafe source path")
        if any(part in {".git", ".gitust", ".agents", ".codex", "node_modules",
                        "target", ".pnpm-store"} for part in relative.parts):
            continue
        if relative.name.startswith(".env") and relative.name not in {
                ".env.example", ".env.sample"}:
            continue
        source = source_file(root, relative)
        if not source.exists():
            continue  # Preserve tracked deletions in the snapshot.
        if source.is_symlink() or not source.is_file():
            raise RuntimeError("snapshot requires regular source files: " + str(relative))
        content = source.read_bytes()
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        shutil.copymode(source, target)
        file_digest = hashlib.sha256(content).digest()
        digest.update(raw + b"\0" + file_digest)
        manifest[str(relative)] = file_digest.hex()
        count += 1
    for relative, expected in manifest.items():
        if hashlib.sha256(source_file(root, relative).read_bytes()).hexdigest() != expected:
            raise RuntimeError("source changed during snapshot: " + relative)
    (destination.parent / "source-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return {"file_count": count, "sha256": digest.hexdigest()}


def runtime_probe(binary, assets, directory, env, log):
    """Run a copied binary with no Node/pnpm or source assets in its working directory."""
    deployed = directory / "gitust"
    shutil.copy2(binary, deployed)
    expected = {"/": (assets.parent / "index.html").read_bytes()}
    expected.update({"/assets/" + p.name: p.read_bytes() for p in assets.iterdir()})
    runtime_bin = directory / "bin"
    runtime_bin.mkdir()
    git = shutil.which("git", path=env["PATH"])
    if not git:
        raise RuntimeError("Git is unavailable")
    (runtime_bin / "git").symlink_to(git)
    runtime_env = {"PATH": str(runtime_bin), "HOME": str(directory / "home"),
                   "LANG": "C", "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0"}
    Path(runtime_env["HOME"]).mkdir()
    fixture = directory / "repository"
    fixture.mkdir()
    for args in [["init", "-q", "-b", "main"], ["config", "user.name", "Platform test"],
                 ["config", "user.email", "platform@company.internal"]]:
        run([git, "-C", str(fixture)] + args, directory, runtime_env, log)
    (fixture / "README.md").write_text("Platform validation fixture\n")
    run([git, "-C", str(fixture), "add", "."], directory, runtime_env, log)
    run([git, "-C", str(fixture), "-c", "commit.gpgsign=false", "commit", "-qm",
         "platform fixture"], directory, runtime_env, log)
    auth = directory / "state" / "auth-db.json"
    password = secrets.token_urlsafe(24)
    admin_env = dict(runtime_env, GITUST_ADMIN_PASSWORD=password)
    run([str(deployed), "admin", "create", "--username", "platform",
         "--email", "platform@company.internal", "--auth-db", str(auth)],
        directory, admin_env, log)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    # Keep source assets unavailable throughout the runtime test.
    hidden_assets = assets.parent.with_name("dist.platform-hidden")
    assets.parent.rename(hidden_assets)
    process = None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with log.open("ab") as output:
            process = subprocess.Popen(
                [str(deployed), "serve", "--bind", "127.0.0.1:" + str(port),
                 "--repo", str(fixture), "--auth-db", str(auth),
                 "--sync-github", str(directory / "unused-upstream.git")],
                cwd=directory, env=runtime_env, stdout=output, stderr=output)
            base = "http://127.0.0.1:" + str(port)
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError("copied binary exited during startup")
                try:
                    with opener.open(base + "/api/v1/health", timeout=1) as response:
                        if response.status == 200:
                            break
                except (OSError, urllib.error.URLError):
                    time.sleep(0.1)
            else:
                raise RuntimeError("copied binary health check timed out")
            for path, content in expected.items():
                with opener.open(base + path, timeout=5) as response:
                    if response.status != 200 or response.read() != content:
                        raise RuntimeError("embedded asset mismatch: " + path)
            body = json.dumps({"identifier": "platform", "password": password}).encode()
            request = urllib.request.Request(base + "/api/v1/auth/login", data=body,
                                             headers={"Content-Type": "application/json"})
            with opener.open(request, timeout=10) as response:
                login = json.load(response)
                if response.status != 200 or not login.get("raw_session_token"):
                    raise RuntimeError("fresh administrator login failed")
        return {"embedded_responses": len(expected), "login": "passed",
                "runtime_path_contains": ["git"], "source_assets_available": False}
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        hidden_assets.rename(assets.parent)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-downloads", action="store_true",
                        help="allow dependency downloads; default is offline")
    parser.add_argument("--store-dir", type=Path, help="existing pnpm cache, optional")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    evidence = Path(tempfile.mkdtemp(prefix="gitust-alpha-platform-"))
    os.chmod(evidence, 0o700)
    checkout = evidence / "source"
    checkout.mkdir()
    log = evidence / "commands.log"
    summary = {"status": "running", "platform": platform.platform(),
               "offline": not args.allow_downloads, "evidence_directory": str(evidence),
               "scope": "fresh source/output directories with existing host toolchains and caches"}
    print("Platform evidence: " + str(evidence), flush=True)
    env = {key: os.environ[key] for key in ["PATH", "HOME", "TMPDIR", "CARGO_HOME",
           "RUSTUP_HOME", "CC", "CXX", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET"]
           if key in os.environ}
    env.update({"CI": "true", "LANG": "C", "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0"})
    try:
        for executable in ["git", "cargo", "rustc", "node", "pnpm", "curl"]:
            if not shutil.which(executable, path=env["PATH"]):
                raise RuntimeError("missing prerequisite: " + executable)
        system = platform.system()
        backend = {"Darwin": Path("/usr/bin/sandbox-exec"),
                   "Linux": Path("/usr/bin/bwrap")}.get(system)
        if system == "Linux" and not backend.is_file():
            backend = Path("/usr/local/bin/bwrap")
        if backend is None or not os.access(backend, os.X_OK):
            raise RuntimeError("supported executable OS sandbox backend is unavailable")
        if not (Path(env["HOME"]) / ".rustup").is_dir():
            raise RuntimeError("~/.rustup required: toolchain sandbox test must not silently skip")
        summary["backend"] = str(backend)
        summary["snapshot"] = snapshot(root, checkout)
        for executable in ["git", "rustc", "cargo", "node", "pnpm"]:
            summary[executable] = subprocess.check_output(
                [executable, "--version"], env=env, text=True).strip()
        if not summary["pnpm"].startswith("10."):
            raise RuntimeError("pnpm 10 is required")
        install = ["pnpm", "install", "--frozen-lockfile"]
        cargo_flags = ["--locked"]
        if not args.allow_downloads:
            install.append("--offline")
            cargo_flags.append("--offline")
        if args.store_dir:
            install.extend(["--store-dir", str(args.store_dir.resolve())])
        timings = summary["duration_seconds"] = {}
        timings["dependency_install"] = run(install, checkout, env, log)
        timings["web_build"] = run(["pnpm", "--filter", "@gitust/design-system", "build"],
                                    checkout, env, log)
        # These checks compile into this snapshot's empty target directory, not the user's target.
        timings["release_build"] = run(["cargo", "build", "--release", "-p", "gitust-cli"]
                                        + cargo_flags, checkout, env, log)
        timings["sandbox_tests"] = run(["cargo", "test", "--release", "-p", "gitust-sandbox"]
                                        + cargo_flags + ["--", "--test-threads=1"], checkout, env, log,
                                        expected_tests=("gate_7_4_sandbox_isolation",
                                                        "gate_rust_toolchain_works_in_sandbox"))
        timings["http_git_roundtrip"] = run(
            ["cargo", "test", "--release", "-p", "gitust-server"] + cargo_flags
            + ["smart_http_gate_7_4_clone_commit_push_fetch_roundtrip_rbac_and_fsck",
               "--", "--test-threads=1"], checkout, env, log,
            expected_tests=("smart_http_gate_7_4_clone_commit_push_fetch_roundtrip_rbac_and_fsck",))
        runtime = evidence / "runtime"
        runtime.mkdir()
        summary["copied_binary_runtime"] = runtime_probe(
            checkout / "target/release/gitust", checkout / "packages/design-system/dist/assets",
            runtime, env, log)
        summary["status"] = "passed"
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        summary["status"] = "failed"
        summary["error"] = str(error)
    finally:
        (evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(json.dumps(summary, indent=2), flush=True)
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
