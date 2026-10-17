//! Bubblewrap mounts, including stable directory views for absent private paths.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{PathAccess, PathRule, SandboxError};

const MAX_PROJECTED_ENTRIES: usize = 4096;
const MAX_PROJECTION_DEPTH: usize = 256;
const MAX_MOUNT_ARGUMENTS: usize = 8000;

fn failure(path: &Path) -> SandboxError {
    SandboxError::Prepare {
        path: path.display().to_string(),
        message: "Linux sandbox mount sources could not be inspected safely".into(),
    }
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>, SandboxError> {
    match fs::symlink_metadata(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(failure(path)),
    }
}

fn access_at(path: &Path, rules: &[PathRule]) -> PathAccess {
    rules
        .iter()
        .filter(|rule| path.starts_with(&rule.path) && rule.path != path)
        .max_by_key(|rule| rule.path.components().count())
        .map_or(PathAccess::ReadOnly, |rule| rule.access)
}

struct ProjectionPlan {
    roots: BTreeSet<PathBuf>,
    hidden_types: BTreeMap<PathBuf, Option<fs::FileType>>,
}

fn projection_roots(rules: &[PathRule]) -> Result<ProjectionPlan, SandboxError> {
    let mut roots = BTreeSet::new();
    let mut hidden_types = BTreeMap::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.access == PathAccess::Hidden)
    {
        let value = metadata(&rule.path)?;
        hidden_types.insert(
            rule.path.clone(),
            value.as_ref().map(fs::Metadata::file_type),
        );
        if value.is_some() {
            continue;
        }
        match access_at(&rule.path, rules) {
            PathAccess::Hidden => continue,
            PathAccess::ReadWrite => {
                return Err(SandboxError::UnsupportedPath(
                    rule.path.display().to_string(),
                ));
            }
            PathAccess::ReadOnly => {}
        }
        for parent in rule.path.ancestors().skip(1) {
            if let Some(value) = metadata(parent)? {
                if !value.is_dir() {
                    return Err(failure(parent));
                }
                roots.insert(parent.to_path_buf());
                break;
            }
        }
    }
    Ok(ProjectionPlan {
        roots,
        hidden_types,
    })
}

fn check_hidden_type(
    path: &Path,
    expected: Option<fs::FileType>,
) -> Result<Option<fs::Metadata>, SandboxError> {
    let value = metadata(path)?;
    if value.as_ref().map(fs::Metadata::file_type) != expected {
        return Err(failure(path));
    }
    Ok(value)
}

fn check_argument_limit(args: &[OsString], workdir: &Path) -> Result<(), SandboxError> {
    if args.len() > MAX_MOUNT_ARGUMENTS {
        return Err(SandboxError::Prepare {
            path: workdir.display().to_string(),
            message: "Linux sandbox mount argument limit exceeded".into(),
        });
    }
    Ok(())
}

struct Source {
    path: PathBuf,
    kind: fs::FileType,
    link: Option<PathBuf>,
}

struct Mounts<'a> {
    args: Vec<OsString>,
    hidden: Vec<&'a Path>,
    seal: BTreeSet<PathBuf>,
    sources: Vec<Source>,
    entries: usize,
}

impl Mounts<'_> {
    fn bind(&mut self, path: &Path, writable: bool) {
        self.seal.remove(path);
        self.args.extend([
            if writable { "--bind" } else { "--ro-bind" }.into(),
            path.as_os_str().to_owned(),
            path.as_os_str().to_owned(),
        ]);
    }

    fn empty_directory(&mut self, path: &Path) {
        self.args
            .extend(["--tmpfs".into(), path.as_os_str().to_owned()]);
        self.seal.insert(path.to_path_buf());
    }

    fn project(&mut self, path: &Path, depth: usize) -> Result<(), SandboxError> {
        if depth > MAX_PROJECTION_DEPTH {
            return Err(failure(path));
        }
        self.empty_directory(path);
        let listing = fs::read_dir(path).map_err(|_| failure(path))?;
        let mut children = Vec::new();
        for entry in listing {
            self.entries += 1;
            if self.entries > MAX_PROJECTED_ENTRIES {
                return Err(failure(path));
            }
            children.push(entry.map_err(|_| failure(path))?.path());
        }
        children.sort();
        for child in children {
            if self.hidden.iter().any(|hidden| **hidden == child) {
                continue;
            }
            let value = metadata(&child)?.ok_or_else(|| failure(&child))?;
            let kind = value.file_type();
            let link = if kind.is_symlink() {
                let target = fs::read_link(&child).map_err(|_| failure(&child))?;
                self.args.extend([
                    "--symlink".into(),
                    target.as_os_str().to_owned(),
                    child.as_os_str().to_owned(),
                ]);
                Some(target)
            } else {
                if value.is_dir() && self.hidden.iter().any(|hidden| hidden.starts_with(&child)) {
                    self.project(&child, depth + 1)?;
                } else {
                    // Existing sockets/devices/FIFOs retain the prior read-only root exposure.
                    self.bind(&child, false);
                }
                None
            };
            self.sources.push(Source {
                path: child,
                kind,
                link,
            });
        }
        Ok(())
    }

    fn validate_sources(&self) -> Result<(), SandboxError> {
        // This catches observed type changes. A separate hostile host writer is outside the
        // trusted single-user boundary; path inspection and the later mount are not atomic.
        for source in &self.sources {
            let value = metadata(&source.path)?.ok_or_else(|| failure(&source.path))?;
            if value.file_type() != source.kind {
                return Err(failure(&source.path));
            }
            if let Some(target) = &source.link {
                if fs::read_link(&source.path).map_err(|_| failure(&source.path))? != *target {
                    return Err(failure(&source.path));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn bwrap_args(
    rules: &[PathRule],
    workdir: &Path,
    command: &str,
    allow_network: bool,
) -> Result<Vec<OsString>, SandboxError> {
    // The last rule at an identical path wins, just as in the Seatbelt policy.
    let mut effective = BTreeMap::new();
    for rule in rules {
        effective.insert(rule.path.clone(), rule.clone());
    }
    let mut rules: Vec<_> = effective.into_values().collect();
    rules.sort_by_key(|rule| rule.path.components().count());
    let plan = projection_roots(&rules)?;
    let projections = &plan.roots;
    let mut mounts = Mounts {
        args: ["--die-with-parent", "--new-session", "--unshare-all"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        hidden: rules
            .iter()
            .filter(|rule| rule.access == PathAccess::Hidden)
            .map(|rule| rule.path.as_path())
            .collect(),
        seal: BTreeSet::new(),
        sources: Vec::new(),
        entries: 0,
    };
    if projections.contains(Path::new("/")) {
        mounts.project(Path::new("/"), 0)?;
    } else {
        mounts.bind(Path::new("/"), false);
    }
    // Root projection must precede these private devices and PID-namespace procfs mounts.
    mounts.args.extend(
        ["--dev", "/dev", "--proc", "/proc"]
            .into_iter()
            .map(OsString::from),
    );
    if allow_network {
        mounts.args.push("--share-net".into());
    }
    let mut steps: Vec<_> = rules
        .iter()
        .filter(|rule| !(rule.path == Path::new("/") && rule.access == PathAccess::ReadOnly))
        .cloned()
        .collect();
    for path in projections {
        if path != Path::new("/") && !steps.iter().any(|rule| rule.path == *path) {
            steps.push(PathRule::new(path, PathAccess::ReadOnly));
        }
    }
    steps.sort_by_key(|rule| rule.path.components().count());
    for rule in &steps {
        match rule.access {
            PathAccess::ReadOnly if projections.contains(&rule.path) => {
                mounts.project(&rule.path, 0)?;
            }
            PathAccess::ReadOnly => mounts.bind(&rule.path, false),
            PathAccess::ReadWrite => mounts.bind(&rule.path, true),
            PathAccess::Hidden => {
                let expected = plan
                    .hidden_types
                    .get(&rule.path)
                    .ok_or_else(|| failure(&rule.path))?;
                match check_hidden_type(&rule.path, *expected)? {
                    Some(value) if value.is_dir() => mounts.empty_directory(&rule.path),
                    Some(_) => {
                        mounts.seal.remove(&rule.path);
                        mounts.args.extend([
                            "--ro-bind".into(),
                            "/dev/null".into(),
                            rule.path.as_os_str().to_owned(),
                        ]);
                    }
                    // An absent deny name stays absent in the projected read-only parent or
                    // an existing hidden tmpfs. It never falls through to a live parent bind.
                    None => {}
                }
            }
        }
    }
    mounts.validate_sources()?;
    for (path, expected) in &plan.hidden_types {
        check_hidden_type(path, *expected)?;
    }
    // All nested mount targets must be created before their parent tmpfs becomes read-only.
    // Non-recursive remounts preserve explicitly granted child worktrees, admin and scratch.
    for path in mounts.seal {
        mounts
            .args
            .extend(["--remount-ro".into(), path.into_os_string()]);
    }
    mounts.args.extend([
        "--chdir".into(),
        workdir.as_os_str().to_owned(),
        "/bin/sh".into(),
        "-c".into(),
        "--".into(),
        command.into(),
    ]);
    check_argument_limit(&mounts.args, workdir)?;
    Ok(mounts.args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(rules: &[PathRule], workdir: &Path) -> Vec<OsString> {
        bwrap_args(rules, workdir, "true", false).expect("safe mount plan")
    }

    fn has_mount(args: &[OsString], option: &str, target: &Path) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == option && pair[1] == target.as_os_str())
    }

    #[test]
    fn absent_private_names_project_only_their_existing_readonly_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let objects = root.join("objects");
        fs::create_dir(&objects).unwrap();
        fs::write(root.join(".visible"), "public").unwrap();
        let missing_dir = root.join("private-history");
        let missing_file = root.join("private-file");
        let args = arguments(
            &[
                PathRule::new(root, PathAccess::ReadOnly),
                PathRule::new(&missing_dir, PathAccess::Hidden),
                PathRule::new(&missing_file, PathAccess::Hidden),
            ],
            root,
        );
        assert!(has_mount(&args, "--tmpfs", root));
        assert!(has_mount(&args, "--remount-ro", root));
        assert!(args.windows(3).any(|triple| triple[0] == "--ro-bind"
            && triple[1] == objects.as_os_str()
            && triple[2] == objects.as_os_str()));
        assert!(args
            .iter()
            .any(|arg| arg == root.join(".visible").as_os_str()));
        assert!(!args.iter().any(|arg| arg == missing_dir.as_os_str()));
        assert!(!args.iter().any(|arg| arg == missing_file.as_os_str()));
        assert!(!missing_dir.exists() && !missing_file.exists());
    }

    #[test]
    fn later_exact_write_grant_replaces_hidden_mount_and_missing_write_denies_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let args = arguments(
            &[
                PathRule::new(tmp.path(), PathAccess::Hidden),
                PathRule::new(tmp.path(), PathAccess::ReadWrite),
            ],
            tmp.path(),
        );
        assert!(!has_mount(&args, "--tmpfs", tmp.path()));
        assert!(!has_mount(&args, "--remount-ro", tmp.path()));
        assert!(args
            .windows(3)
            .any(|triple| triple[0] == "--bind" && triple[2] == tmp.path().as_os_str()));
        assert!(bwrap_args(
            &[
                PathRule::new(tmp.path(), PathAccess::ReadWrite),
                PathRule::new(tmp.path().join("missing"), PathAccess::Hidden),
            ],
            tmp.path(),
            "true",
            false
        )
        .is_err());
        fs::write(tmp.path().join("file"), "public").unwrap();
        assert!(bwrap_args(
            &[PathRule::new(
                tmp.path().join("file/child"),
                PathAccess::Hidden
            )],
            tmp.path(),
            "true",
            false
        )
        .is_err());
    }

    #[tokio::test]
    async fn later_exact_write_grant_writes_the_host_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let workdir = tmp.path().join("worktree");
        fs::create_dir(&workdir).unwrap();
        let spec = crate::SandboxSpec {
            workdir: workdir.clone(),
            scratch_base: tmp.path().join("scratch"),
            rules: vec![
                PathRule::new(&workdir, PathAccess::Hidden),
                PathRule::new(&workdir, PathAccess::ReadWrite),
            ],
        };
        let result = crate::LocalSandboxExecutor::new(1)
            .execute(
                gitust_core::WorktreeId::new(),
                &spec,
                "printf allowed > created",
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            fs::read_to_string(workdir.join("created")).unwrap(),
            "allowed"
        );
    }

    #[test]
    fn observed_hidden_changes_and_excessive_arguments_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let private = tmp.path().join("private");
        fs::create_dir(&private).unwrap();
        let kind = fs::symlink_metadata(&private).unwrap().file_type();
        fs::remove_dir(&private).unwrap();
        assert!(check_hidden_type(&private, Some(kind)).is_err());
        fs::write(&private, "new private type").unwrap();
        assert!(check_hidden_type(&private, Some(kind)).is_err());
        let args = vec![OsString::new(); MAX_MOUNT_ARGUMENTS + 1];
        assert!(check_argument_limit(&args, tmp.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn projected_symlinks_keep_their_namespace_targets_and_type_changes_fail() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let secret = root.join("private");
        fs::write(&secret, "private").unwrap();
        fs::write(root.join("public"), "public").unwrap();
        for (name, target) in [
            ("absolute", secret.clone()),
            ("relative", PathBuf::from("private")),
            ("dangling", PathBuf::from("future/item")),
            ("allowed", PathBuf::from("public")),
        ] {
            symlink(target, root.join(name)).unwrap();
        }
        let rules = [
            PathRule::new(root, PathAccess::ReadOnly),
            PathRule::new(&secret, PathAccess::Hidden),
            PathRule::new(root.join("future"), PathAccess::Hidden),
        ];
        let args = arguments(&rules, root);
        for name in ["absolute", "relative", "dangling", "allowed"] {
            let path = root.join(name);
            let target = fs::read_link(&path).unwrap();
            assert!(args.windows(3).any(|triple| triple[0] == "--symlink"
                && triple[1] == target.as_os_str()
                && triple[2] == path.as_os_str()));
            assert!(!args
                .windows(3)
                .any(|triple| triple[0] == "--ro-bind" && triple[1] == path.as_os_str()));
        }
        let mut mounts = Mounts {
            args: Vec::new(),
            hidden: vec![secret.as_path()],
            seal: BTreeSet::new(),
            sources: Vec::new(),
            entries: 0,
        };
        mounts.project(root, 0).unwrap();
        fs::remove_file(root.join("allowed")).unwrap();
        fs::write(root.join("allowed"), "changed type").unwrap();
        assert!(mounts.validate_sources().is_err());
        mounts.entries = MAX_PROJECTED_ENTRIES;
        assert!(mounts.project(root, 0).is_err());
    }
}
