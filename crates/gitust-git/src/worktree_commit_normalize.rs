//! Git's builtin content conversions in private metadata with no executable filter configuration.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::worktree_commit_paths::{self as sources, invalid};
use crate::{git_command, io_err, GitEngineError};

const MAX_METADATA_BYTES: usize = 1024 * 1024;
const ATTRIBUTES: [&str; 6] = [
    "text",
    "eol",
    "working-tree-encoding",
    "ident",
    "crlf",
    "filter",
];

struct Input {
    root: PathBuf,
    path: String,
    bytes: Option<Vec<u8>>,
}

/// All configuration/attribute inputs are immutable private copies, then rechecked before CAS.
pub(crate) struct Normalization {
    inputs: Vec<Input>,
    scratch: tempfile::TempDir,
    index: PathBuf,
    objects: PathBuf,
    settings: Vec<(String, String)>,
    conversions: HashMap<String, bool>,
    normalized: HashMap<String, (u32, gix::ObjectId)>,
    metadata_bytes: usize,
}

impl Normalization {
    pub fn new(
        root: &Path,
        common: &Path,
        admin: &Path,
        paths: &[&str],
        deleted: &[&str],
        index: &Path,
        parent_tree: &gix::Tree<'_>,
    ) -> Result<Self, GitEngineError> {
        let scratch = tempfile::Builder::new()
            .prefix("gitust-convert-")
            .tempdir_in(admin)
            .map_err(|e| io_err(admin, &e))?;
        let mut state = Self {
            inputs: Vec::new(),
            scratch,
            index: index.into(),
            objects: common.join("objects"),
            settings: Vec::new(),
            conversions: HashMap::new(),
            normalized: HashMap::new(),
            metadata_bytes: 0,
        };
        let metadata = state.scratch.path().join("git");
        for directory in [
            metadata.join("refs"),
            metadata.join("objects"),
            metadata.join("info"),
            state.scratch.path().join("worktree"),
        ] {
            fs::create_dir_all(&directory).map_err(|e| io_err(&directory, &e))?;
        }
        fs::write(
            metadata.join("HEAD"),
            b"ref: refs/heads/private-conversion\n",
        )
        .map_err(|e| io_err(&metadata, &e))?;
        let config = state.capture(common, "config")?;
        let worktree_config = state.load_settings(config.as_deref())?;
        // Linked checkouts read their admin/config.worktree only when this extension is enabled.
        let config = state.capture(admin, "config.worktree")?;
        if worktree_config {
            state.load_settings(config.as_deref())?;
        }
        let attributes = state.capture(common, "info/attributes")?;
        if let Some(bytes) = attributes {
            fs::write(metadata.join("info/attributes"), bytes)
                .map_err(|e| io_err(&metadata, &e))?;
        }
        // Git add drops selected attribute deletions before converting other selected paths.
        // Without this, Git's index fallback would resurrect a deliberately removed policy.
        let deleted_attributes: HashSet<_> = deleted
            .iter()
            .copied()
            .filter(|path| path.rsplit('/').next() == Some(".gitattributes"))
            .collect();
        state.remove_attributes(&deleted_attributes, parent_tree.id.kind())?;
        let mut names = HashSet::new();
        names.insert(".gitattributes".to_owned());
        let mut attribute_path_bytes = ".gitattributes".len();
        for path in paths {
            for (offset, _) in path.match_indices('/') {
                let name = format!("{}/.gitattributes", &path[..offset]);
                let length = name.len();
                if names.insert(name) {
                    attribute_path_bytes += length;
                }
                if names.len() > 512 || attribute_path_bytes > 256 * 1024 {
                    return Err(invalid("too many ancestor attribute paths for one commit"));
                }
            }
        }
        let mut names: Vec<_> = names.into_iter().collect();
        names.sort();
        for name in names {
            let bytes = match state.capture(root, &name)? {
                Some(bytes) => Some(bytes),
                None if deleted_attributes.contains(name.as_str()) => None,
                None => state.committed_attributes(parent_tree, &name)?,
            };
            if let Some(bytes) = bytes {
                let target = state.scratch.path().join("worktree").join(name);
                let parent = target
                    .parent()
                    .ok_or_else(|| invalid("attribute path has no parent"))?;
                fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
                fs::write(&target, bytes).map_err(|e| io_err(&target, &e))?;
            }
        }
        state.inspect(paths)?;
        Ok(state)
    }

    fn remove_attributes(
        &self,
        paths: &HashSet<&str>,
        hash: gix::hash::Kind,
    ) -> Result<(), GitEngineError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut input = Vec::new();
        for path in paths {
            input
                .extend_from_slice(format!("0 {}\t{path}\0", gix::ObjectId::null(hash)).as_bytes());
        }
        let mut command = self.command();
        command.args(["update-index", "-z", "--index-info"]);
        let (_, truncated) = crate::process::run_capped_with_input(
            command,
            Duration::from_secs(10),
            0,
            Some(input),
        )?;
        if truncated {
            return Err(invalid("unexpected attribute deletion output"));
        }
        Ok(())
    }

    fn capture(&mut self, root: &Path, path: &str) -> Result<Option<Vec<u8>>, GitEngineError> {
        let bytes = read_metadata(root, path)?;
        self.reserve_metadata(bytes.as_ref().map_or(0, Vec::len))?;
        self.inputs.push(Input {
            root: root.into(),
            path: path.into(),
            bytes: bytes.clone(),
        });
        Ok(bytes)
    }

    fn reserve_metadata(&mut self, size: usize) -> Result<(), GitEngineError> {
        self.metadata_bytes += size;
        if self.metadata_bytes > MAX_METADATA_BYTES {
            return Err(invalid(
                "commit attribute/configuration snapshots exceed 1 MiB",
            ));
        }
        Ok(())
    }

    fn committed_attributes(
        &mut self,
        tree: &gix::Tree<'_>,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitEngineError> {
        let Some(entry) = tree
            .lookup_entry_by_path(path)
            .map_err(|e| invalid(e.to_string()))?
        else {
            return Ok(None);
        };
        if !entry.mode().is_blob() || entry.mode().is_link() {
            return Err(invalid("committed attribute files must be ordinary blobs"));
        }
        let header = tree
            .repo
            .find_header(entry.oid())
            .map_err(|e| invalid(e.to_string()))?;
        if header.kind() != gix::objs::Kind::Blob || header.size() > MAX_METADATA_BYTES as u64 {
            return Err(invalid("committed attribute file exceeds 1 MiB"));
        }
        self.reserve_metadata(header.size() as usize)?;
        let object = entry.object().map_err(|e| invalid(e.to_string()))?;
        Ok(Some(object.data.to_vec()))
    }

    pub fn validate_selected_attribute(
        &self,
        root: &Path,
        path: &str,
        bytes: Option<&[u8]>,
    ) -> Result<(), GitEngineError> {
        for input in &self.inputs {
            if input.root == root && input.path == path && input.bytes.as_deref() != bytes {
                return Err(invalid(
                    "selected attribute file changed while preparing the commit; retry",
                ));
            }
        }
        Ok(())
    }

    fn load_settings(&mut self, config: Option<&[u8]>) -> Result<bool, GitEngineError> {
        let Some(config) = config else {
            return Ok(false);
        };
        let text = std::str::from_utf8(config)
            .map_err(|_| invalid("commit configuration is not UTF-8"))?;
        let config = gix::config::File::try_from(text)
            .map_err(|_| invalid("invalid commit configuration"))?;
        let worktree_config = match config.boolean("extensions.worktreeConfig") {
            Some(value) => value.map_err(|_| invalid("invalid extensions.worktreeConfig value"))?,
            None => false,
        };
        for section in config.sections() {
            if section.header().subsection_name().is_some() {
                continue;
            }
            if section.header().name().to_ascii_lowercase().as_slice() != b"core" {
                continue;
            }
            for key in section.body().value_names() {
                let key_name: &str = key.as_ref();
                let key_lower = key_name.to_ascii_lowercase();
                if !matches!(
                    key_lower.as_str(),
                    "attributesfile"
                        | "autocrlf"
                        | "eol"
                        | "safecrlf"
                        | "checkroundtripencoding"
                        | "ignorecase"
                        | "precomposeunicode"
                        | "filemode"
                        | "symlinks"
                ) {
                    continue;
                }
                let value = match section.body().value_implicit(key_name) {
                    Some(Some(value)) => std::str::from_utf8(value.as_ref())
                        .map_err(|_| invalid("commit configuration value is not UTF-8"))?
                        .to_owned(),
                    Some(None) => "true".to_owned(),
                    None => return Err(invalid("commit configuration value disappeared")),
                };
                if key_lower == "attributesfile" {
                    if value != "/dev/null" {
                        return Err(invalid("external core.attributesFile is unsupported for managed commits; use repository .gitattributes or info/attributes"));
                    }
                    continue;
                }
                let key = format!("core.{key_lower}");
                if let Some((_, current)) = self.settings.iter_mut().find(|(name, _)| name == &key)
                {
                    *current = value;
                } else {
                    self.settings.push((key, value));
                }
            }
        }
        Ok(worktree_config)
    }

    fn command(&self) -> Command {
        let mut command = git_command(&self.scratch.path().join("worktree"));
        command
            .env("GIT_DIR", self.scratch.path().join("git"))
            .env("GIT_WORK_TREE", self.scratch.path().join("worktree"))
            .env("GIT_INDEX_FILE", &self.index)
            .env("GIT_OBJECT_DIRECTORY", &self.objects)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["-c", "core.attributesFile=/dev/null"]);
        for (key, value) in &self.settings {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        command
    }

    fn inspect(&mut self, paths: &[&str]) -> Result<(), GitEngineError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut command = self.command();
        command
            .args(["check-attr", "-z", "--stdin"])
            .args(ATTRIBUTES);
        let input = paths
            .iter()
            .flat_map(|path| path.bytes().chain([0]))
            .collect();
        let (output, truncated) = crate::process::run_capped_with_input(
            command,
            Duration::from_secs(10),
            MAX_METADATA_BYTES,
            Some(input),
        )?;
        if truncated {
            return Err(invalid("commit attribute output exceeds 1 MiB"));
        }
        let body = output
            .strip_suffix(&[0])
            .ok_or_else(|| invalid("incomplete commit attributes"))?;
        let fields: Vec<_> = body.split(|byte| *byte == 0).collect();
        if fields.len() != paths.len() * ATTRIBUTES.len() * 3 {
            return Err(invalid("invalid commit attribute output"));
        }
        let auto = self
            .settings
            .iter()
            .rev()
            .find(|(key, _)| key == "core.autocrlf")
            .is_some_and(|(_, value)| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "false" | "no" | "off" | "0"
                )
            });
        let mut preserve_index_mode = false;
        for (key, value) in &self.settings {
            if matches!(key.as_str(), "core.filemode" | "core.symlinks") {
                let value = gix::config::Boolean::try_from(std::ffi::OsString::from(value))
                    .map_err(|_| invalid("invalid Git file mode policy"))?;
                preserve_index_mode |= !value.is_true();
            }
        }
        for (path, fields) in paths.iter().zip(fields.chunks_exact(ATTRIBUTES.len() * 3)) {
            let mut values = Vec::new();
            for (name, triple) in ATTRIBUTES.iter().zip(fields.chunks_exact(3)) {
                if triple[0] != path.as_bytes() || triple[1] != name.as_bytes() {
                    return Err(invalid("mismatched commit attribute identity"));
                }
                values.push(
                    std::str::from_utf8(triple[2])
                        .map_err(|_| invalid("invalid commit attribute value"))?,
                );
            }
            if !matches!(values[5], "unspecified" | "unset") {
                return Err(invalid(
                    "custom Git clean filters are forbidden for managed commits",
                ));
            }
            let explicit = values[..5]
                .iter()
                .any(|value| !matches!(*value, "unspecified" | "unset"));
            self.conversions.insert(
                (*path).into(),
                explicit || (auto && values[0] != "unset") || preserve_index_mode,
            );
        }
        Ok(())
    }

    /// Git add consults the existing blob for text=auto/autocrlf, unlike hash-object.
    /// Its inputs and configuration live only in this private snapshot; no agent path is reopened.
    pub fn prepare_blobs(
        &mut self,
        repo: &gix::Repository,
        selected: &[(&String, Option<sources::Source>)],
    ) -> Result<(), GitEngineError> {
        let mut paths = Vec::new();
        for (path, source) in selected {
            let Some(source) = source else {
                continue;
            };
            if source.mode == 0o120000 || self.conversions.get(path.as_str()) != Some(&true) {
                continue;
            }
            let target = self.scratch.path().join("worktree").join(path);
            let parent = target
                .parent()
                .ok_or_else(|| invalid("conversion path has no parent"))?;
            fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
            fs::write(&target, &source.bytes).map_err(|e| io_err(&target, &e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // Git add must observe the captured executable bit, then apply its mode policy.
                let mode = if source.mode == 0o100755 {
                    0o700
                } else {
                    0o600
                };
                fs::set_permissions(&target, fs::Permissions::from_mode(mode))
                    .map_err(|e| io_err(&target, &e))?;
            }
            paths.push(path.as_str());
        }
        if paths.is_empty() {
            return Ok(());
        }
        let input = paths
            .iter()
            .flat_map(|path| path.bytes().chain([0]))
            .collect();
        let mut command = self.command();
        command.args([
            "--literal-pathspecs",
            "add",
            "--force",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ]);
        let (_, truncated) = crate::process::run_capped_with_input(
            command,
            Duration::from_secs(10),
            0,
            Some(input),
        )?;
        if truncated {
            return Err(invalid("unexpected Git add output during normalization"));
        }
        let mut command = self.command();
        command
            .args(["--literal-pathspecs", "ls-files", "--stage", "-z", "--"])
            .args(&paths);
        let (output, truncated) =
            crate::process::run_capped(command, Duration::from_secs(10), MAX_METADATA_BYTES)?;
        if truncated {
            return Err(invalid("normalized Git index output exceeded limit"));
        }
        let output = output
            .strip_suffix(&[0])
            .ok_or_else(|| invalid("incomplete normalized index entries"))?;
        let expected: HashSet<_> = paths.into_iter().collect();
        for entry in output.split(|byte| *byte == 0) {
            let separator = entry
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| invalid("invalid normalized index entry"))?;
            let fields: Vec<_> = entry[..separator].split(|byte| *byte == b' ').collect();
            if fields.len() != 3
                || !matches!(fields[0], b"100644" | b"100755" | b"120000")
                || fields[2] != b"0"
            {
                return Err(invalid(
                    "normalized Git index entry is not a supported stage-zero file",
                ));
            }
            let path = std::str::from_utf8(&entry[separator + 1..])
                .map_err(|_| invalid("invalid normalized path"))?;
            if !expected.contains(path) || self.normalized.contains_key(path) {
                return Err(invalid("unexpected normalized Git path"));
            }
            let oid = gix::ObjectId::from_hex(fields[1])
                .map_err(|_| invalid("invalid normalized Git blob SHA"))?;
            let header = repo.find_header(oid).map_err(|e| invalid(e.to_string()))?;
            if header.kind() != gix::objs::Kind::Blob
                || header.size() > sources::MAX_FILE_BYTES as u64
            {
                return Err(invalid("normalized Git blob exceeds the 8 MiB file limit"));
            }
            let mode = if fields[0] == b"120000" {
                0o120000
            } else if fields[0] == b"100755" {
                0o100755
            } else {
                0o100644
            };
            self.normalized.insert(path.into(), (mode, oid));
        }
        if self.normalized.len() != expected.len() {
            return Err(invalid("Git omitted a path during normalization"));
        }
        Ok(())
    }

    pub fn write_entry(
        &self,
        repo: &gix::Repository,
        path: &str,
        mode: u32,
        bytes: Vec<u8>,
    ) -> Result<(u32, gix::ObjectId), GitEngineError> {
        match self.conversions.get(path) {
            Some(true) => self
                .normalized
                .get(path)
                .copied()
                .ok_or_else(|| invalid("missing normalized blob")),
            Some(false) => repo
                .write_blob(bytes)
                .map(|id| (mode, id.detach()))
                .map_err(|e| invalid(e.to_string())),
            None => Err(invalid("missing commit attribute decision")),
        }
    }

    pub fn validate(&self) -> Result<(), GitEngineError> {
        for input in &self.inputs {
            if read_metadata(&input.root, &input.path)? != input.bytes {
                return Err(invalid(
                    "Git attributes or configuration changed while preparing the commit; retry",
                ));
            }
        }
        Ok(())
    }
}

fn read_metadata(root: &Path, path: &str) -> Result<Option<Vec<u8>>, GitEngineError> {
    match sources::read(root, path)? {
        Some(source) if source.mode == 0o120000 => Err(invalid(
            "Git attribute/configuration files must not be symbolic links",
        )),
        Some(source) if source.bytes.len() > MAX_METADATA_BYTES => {
            Err(invalid("Git attribute/configuration file exceeds 1 MiB"))
        }
        Some(source) => Ok(Some(source.bytes)),
        None => Ok(None),
    }
}
