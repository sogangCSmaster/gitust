//! Sandbox path policy resolution and OS backend profile generation (macOS Seatbelt & Linux Bubblewrap).

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use gitust_core::SandboxRunId;

pub(crate) use crate::linux_policy::bwrap_args;
use crate::{PathAccess, PathRule, SandboxBackend, SandboxError, SandboxSpec};

/// Everything resolved on a blocking thread before spawning.
pub(crate) struct PreparedRun {
    pub(crate) backend: SandboxBackend,
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    pub(crate) workdir: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) tmp: PathBuf,
    pub(crate) run_dir: PathBuf,
    /// Toolchain variables (`RUSTUP_HOME`, `CARGO_HOME`, ...).
    pub(crate) toolchain_env: Vec<(&'static str, OsString)>,
}

fn prepare_err(path: &Path, e: &std::io::Error) -> SandboxError {
    SandboxError::Prepare {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

pub(crate) fn prepare_run(
    spec: &SandboxSpec,
    run_id: SandboxRunId,
    command_str: &str,
    allow_network: bool,
) -> Result<PreparedRun, SandboxError> {
    let (backend, program) = SandboxBackend::detect()?;

    let workdir = fs::canonicalize(&spec.workdir).map_err(|e| prepare_err(&spec.workdir, &e))?;
    let run_dir = spec.scratch_base.join(run_id.0.to_string());
    let home = run_dir.join("home");
    let tmp = run_dir.join("tmp");
    for dir in [&home, &tmp] {
        fs::create_dir_all(dir).map_err(|e| prepare_err(dir, &e))?;
    }
    let scratch_base =
        fs::canonicalize(&spec.scratch_base).map_err(|e| prepare_err(&spec.scratch_base, &e))?;
    let run_dir = fs::canonicalize(&run_dir).map_err(|e| prepare_err(&run_dir, &e))?;

    let mut toolchain_env = prepare_toolchain_env(&run_dir)?;

    let mut rules = host_rules();
    crate::isolation_tier::apply_isolation_tier_policy(
        spec.isolation_tier,
        &mut rules,
        &mut toolchain_env,
    );
    rules.extend(spec.rules.iter().cloned());
    rules.push(PathRule::new(&scratch_base, PathAccess::Hidden));
    rules.push(PathRule::new(&run_dir, PathAccess::ReadWrite));
    let rules = resolve_rules(rules)?;

    let args = match backend {
        SandboxBackend::MacosSeatbelt => {
            let profile = seatbelt_profile(&rules, allow_network)?;
            vec![
                OsString::from("-p"),
                OsString::from(profile),
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("--"),
                OsString::from(command_str),
            ]
        }
        SandboxBackend::LinuxBubblewrap => {
            bwrap_args(&rules, &workdir, command_str, allow_network)?
        }
    };

    Ok(PreparedRun {
        backend,
        program,
        args,
        workdir,
        home: run_dir.join("home"),
        tmp: run_dir.join("tmp"),
        run_dir,
        toolchain_env,
    })
}

/// Points rustup/cargo at the host toolchain without exposing the host home:
/// - `RUSTUP_HOME` is the host `~/.rustup` (read-only in the policy).
/// - `CARGO_HOME` is a per-run writable dir (cargo writes lock/cache files there) whose
///   `registry/` and `git/` link to the host's read-only copies.
/// - Network is denied, so cargo runs offline. Crates not already extracted in the host
///   registry cannot be extracted (read-only) and will fail to build.
fn prepare_toolchain_env(run_dir: &Path) -> Result<Vec<(&'static str, OsString)>, SandboxError> {
    let mut env = vec![("CARGO_NET_OFFLINE", OsString::from("true"))];
    let Some(host_home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(env);
    };
    let rustup_home = host_home.join(".rustup");
    if rustup_home.is_dir() {
        let canonical =
            fs::canonicalize(&rustup_home).map_err(|e| prepare_err(&rustup_home, &e))?;
        env.push(("RUSTUP_HOME", canonical.into_os_string()));
    }
    let cargo_home = run_dir.join("cargo");
    fs::create_dir_all(&cargo_home).map_err(|e| prepare_err(&cargo_home, &e))?;
    #[cfg(unix)]
    for sub in ["registry", "git"] {
        let host_dir = host_home.join(".cargo").join(sub);
        if host_dir.is_dir() {
            let link = cargo_home.join(sub);
            let canonical = fs::canonicalize(&host_dir).map_err(|e| prepare_err(&host_dir, &e))?;
            std::os::unix::fs::symlink(&canonical, &link).map_err(|e| prepare_err(&link, &e))?;
        }
    }
    env.push(("CARGO_HOME", cargo_home.into_os_string()));
    Ok(env)
}

/// Built-in host rules: hide user homes, temp dirs and external volumes; expose the Rust
/// toolchain read-only.
fn host_rules() -> Vec<PathRule> {
    #[cfg(target_os = "macos")]
    let hidden: &[&str] = &[
        "/Users",
        "/System/Volumes/Data/Users",
        "/Volumes",
        "/private/tmp",
        "/private/var/tmp",
        "/private/var/folders",
        "/private/etc/master.passwd",
    ];
    #[cfg(not(target_os = "macos"))]
    let hidden: &[&str] = &[
        "/home",
        "/root",
        "/tmp",
        "/var/tmp",
        "/run/user",
        "/mnt",
        "/media",
    ];

    let mut rules: Vec<PathRule> = hidden
        .iter()
        .map(|p| PathRule::new(*p, PathAccess::Hidden))
        .collect();

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        rules.push(PathRule::new(&home, PathAccess::Hidden));
        for sub in [".rustup", ".cargo/bin", ".cargo/registry", ".cargo/git"] {
            let path = home.join(sub);
            if path.is_dir() {
                rules.push(PathRule::new(path, PathAccess::ReadOnly));
            }
        }
    }
    rules
}

/// Canonicalizes rule paths and orders them shallow-to-deep so that deeper rules take
/// precedence. Equal depth keeps insertion order (later wins).
///
/// For `PathAccess::Hidden`, if the target leaf does not exist yet (for example
/// `<common_git_dir>/refs/stash` or `COMMIT_EDITMSG` before the host runs `git stash` or
/// `git commit`), we canonicalize the deepest existing non-root ancestor and append the
/// remaining normal path components so Seatbelt still emits a `(deny ...)` rule covering files
/// created while the sandbox is running.
pub(crate) fn resolve_rules(rules: Vec<PathRule>) -> Result<Vec<PathRule>, SandboxError> {
    let mut resolved = Vec::with_capacity(rules.len());
    for rule in rules {
        let Some(path) = canonicalize_rule_path(&rule.path, rule.access) else {
            // Dropping a rule can inherit a broader writable parent. Required paths must
            // resolve; optional host-toolchain paths are filtered before adding their rules.
            return Err(SandboxError::UnsupportedPath(
                rule.path.to_string_lossy().into_owned(),
            ));
        };
        let lossy = path.to_string_lossy();
        if path.to_str().is_none() || lossy.chars().any(char::is_control) {
            return Err(SandboxError::UnsupportedPath(lossy.into_owned()));
        }
        resolved.push(PathRule::new(path, rule.access));
    }
    resolved.sort_by_key(|r| r.path.components().count());
    Ok(resolved)
}

fn canonicalize_rule_path(path: &Path, access: PathAccess) -> Option<PathBuf> {
    // fail-open-ok: missing Hidden paths fall back to ancestor resolution; missing grants return None and resolve_rules rejects the run.
    if let Ok(canonical) = fs::canonicalize(path) {
        return Some(canonical);
    }
    if access != PathAccess::Hidden {
        return None;
    }
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if ancestor == Path::new("/") {
            // Nothing below `/` exists, so no component can be a symlink: the lexical path is
            // already canonical. Keep the deny rule (e.g. a non-existent `$HOME`) instead of
            // failing every sandbox spawn on hosts without `/mnt`, `/media`, or a real HOME.
            return lexical_absolute_path(path);
        }
        // fail-open-ok: probes each ancestor until the deepest existing directory is found.
        if let Ok(mut canonical_base) = fs::canonicalize(ancestor) {
            // fail-open-ok: `ancestor` is produced by `path.ancestors()`.
            let suffix = path.strip_prefix(ancestor).ok()?;
            for comp in suffix.components() {
                match comp {
                    Component::Normal(part) => canonical_base.push(part),
                    _ => return None,
                }
            }
            return Some(canonical_base);
        }
    }
    None
}

/// Returns `path` unchanged if it is absolute and made only of normal components.
fn lexical_absolute_path(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let mut out = PathBuf::from("/");
    for comp in components {
        match comp {
            Component::Normal(part) => out.push(part),
            _ => return None,
        }
    }
    Some(out)
}

/// Builds a Seatbelt profile. Seatbelt applies the last matching rule, so emitting rules in
/// shallow-to-deep order makes deeper rules win.
pub(crate) fn seatbelt_profile(
    rules: &[PathRule],
    allow_network: bool,
) -> Result<String, SandboxError> {
    let mut profile = String::with_capacity(256 + rules.len() * 96);
    profile.push_str("(version 1)\n(allow default)\n");
    if !allow_network {
        profile.push_str("(deny network*)\n");
    }
    profile.push_str("(deny file-write*)\n(allow file-write* (subpath \"/dev\"))\n");
    for rule in rules {
        let p = seatbelt_quote(&rule.path)?;
        match rule.access {
            PathAccess::Hidden => {
                profile.push_str(&format!("(deny file-read-data (subpath {p}))\n"));
                profile.push_str(&format!("(deny file-write* (subpath {p}))\n"));
            }
            PathAccess::ReadOnly => {
                profile.push_str(&format!("(allow file-read-data (subpath {p}))\n"));
                profile.push_str(&format!("(deny file-write* (subpath {p}))\n"));
            }
            PathAccess::ReadWrite => {
                profile.push_str(&format!("(allow file-read-data (subpath {p}))\n"));
                profile.push_str(&format!("(allow file-write* (subpath {p}))\n"));
            }
        }
    }
    Ok(profile)
}

pub(crate) fn seatbelt_quote(path: &Path) -> Result<String, SandboxError> {
    let s = path
        .to_str()
        .ok_or_else(|| SandboxError::UnsupportedPath(path.to_string_lossy().into_owned()))?;
    if s.chars().any(char::is_control) {
        return Err(SandboxError::UnsupportedPath(s.to_owned()));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub(crate) fn build_sanitized_path() -> String {
    let mut dirs = vec![
        "/usr/local/bin".to_owned(),
        "/opt/homebrew/bin".to_owned(),
        "/usr/bin".to_owned(),
        "/bin".to_owned(),
        "/usr/sbin".to_owned(),
        "/sbin".to_owned(),
    ];
    // fail-open-ok: optional host `~/.cargo/bin` addition when `HOME` is set; standard system `PATH` is always preserved.
    if let Ok(host_home) = std::env::var("HOME") {
        let cargo_bin = Path::new(&host_home).join(".cargo").join("bin");
        if cargo_bin.is_dir() {
            // fail-open-ok: an unavailable optional cargo bin is omitted from PATH.
            if let Ok(canonical) = fs::canonicalize(cargo_bin) {
                dirs.insert(0, canonical.display().to_string());
            }
        }
    }
    dirs.join(":")
}
