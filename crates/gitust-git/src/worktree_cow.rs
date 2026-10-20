//! Copy-on-Write (`clonefile` / `FICLONE`) agent worktree materialization from a cached
//! per-commit template directory under `<common_git_dir>/gitust-cow-templates/<commit_sha>`.
//!
//! Falls back to `git worktree add` when the base tree contains symlinks, submodules,
//! `.gitattributes`, or case-insensitive sibling collisions.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gitust_core::{CommitSha, WorktreeId};
use gix::objs::tree::EntryKind;
use gix::prelude::HeaderExt;

use crate::{io_err, GitEngineError};

pub(crate) const COW_TEMPLATES_DIR_NAME: &str = "gitust-cow-templates";
const MAX_COW_TEMPLATE_FILES: usize = 50_000;
const MAX_COW_TEMPLATE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COW_TREE_DEPTH: usize = 64;
const MAX_CACHED_COW_TEMPLATES: usize = 8;

#[derive(Debug, Clone)]
struct IndexEntry {
    rel_path: String,
    stat_words: [u32; 10],
    oid: [u8; 20],
}

pub(crate) struct CowWorktreePaths {
    pub worktree_dir: PathBuf,
    pub worktree_admin_dir: PathBuf,
    pub worktrees_base: PathBuf,
}

/// Attempts to materialize the agent worktree using an in-process CoW template (`clonefile` on
/// macOS APFS, `FICLONE` / recursive copy fallback elsewhere). Returns `Ok(None)` if the commit
/// tree is ineligible for CoW templating (e.g., symlinks, submodules, `.gitattributes`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_materialize_cow_worktree(
    repo: &gix::Repository,
    common_git_dir: &Path,
    worktrees_base: &Path,
    worktree_id: WorktreeId,
    clean_branch: &str,
    clean_base: &str,
    base_id: gix::ObjectId,
    prompt_summary: &str,
) -> Result<Option<CowWorktreePaths>, GitEngineError> {
    let Some(template_dir) = ensure_cow_template(repo, common_git_dir, base_id)? else {
        return Ok(None);
    };

    fs::create_dir_all(worktrees_base).map_err(|e| io_err(worktrees_base, &e))?;
    let base_meta = fs::symlink_metadata(worktrees_base).map_err(|e| io_err(worktrees_base, &e))?;
    let canon_base = if base_meta.file_type().is_symlink() || !worktrees_base.is_absolute() {
        fs::canonicalize(worktrees_base).map_err(|e| io_err(worktrees_base, &e))?
    } else {
        worktrees_base.to_path_buf()
    };
    let id_str = worktree_id.0.to_string();
    let canon_worktree_dir = canon_base.join(&id_str);
    if canon_worktree_dir.exists() {
        return Err(GitEngineError::Io {
            path: canon_worktree_dir.display().to_string(),
            message: "worktree destination already exists".to_owned(),
        });
    }

    let worktrees_admin_root = common_git_dir.join("worktrees");
    fs::create_dir_all(&worktrees_admin_root).map_err(|e| io_err(&worktrees_admin_root, &e))?;
    let admin_root_meta = fs::symlink_metadata(&worktrees_admin_root)
        .map_err(|e| io_err(&worktrees_admin_root, &e))?;
    if admin_root_meta.file_type().is_symlink() {
        return Err(GitEngineError::VerificationInvalid(
            "worktrees administration root must not be a symlink".into(),
        ));
    }
    let admin_dir = worktrees_admin_root.join(&id_str);
    if admin_dir.exists() {
        return Err(GitEngineError::Io {
            path: admin_dir.display().to_string(),
            message: "worktree admin directory already exists".to_owned(),
        });
    }

    let materialize_res = (|| -> Result<(), GitEngineError> {
        clone_tree_cow(&template_dir.join("tree"), &canon_worktree_dir)?;
        clone_tree_cow(&template_dir.join("admin"), &admin_dir)?;
        let head_path = admin_dir.join("HEAD");
        fs::write(&head_path, format!("ref: refs/heads/{clean_branch}\n"))
            .map_err(|e| io_err(&head_path, &e))?;
        let dot_git_path = canon_worktree_dir.join(".git");
        let gitdir_path = admin_dir.join("gitdir");
        fs::write(&gitdir_path, format!("{}\n", dot_git_path.display()))
            .map_err(|e| io_err(&gitdir_path, &e))?;
        fs::write(&dot_git_path, format!("gitdir: {}\n", admin_dir.display()))
            .map_err(|e| io_err(&dot_git_path, &e))?;
        crate::worktree_origin::persist(
            &admin_dir,
            worktree_id,
            clean_branch,
            clean_base,
            &CommitSha(base_id.to_string()),
            Some(prompt_summary),
        )
    })();
    if let Err(err) = materialize_res {
        // fail-open-ok: best-effort cleanup of partially materialized CoW worktree before returning `Err(err)`.
        let _ = fs::remove_dir_all(&canon_worktree_dir);
        // fail-open-ok: best-effort cleanup of partially materialized worktree admin dir before returning `Err(err)`.
        let _ = fs::remove_dir_all(&admin_dir);
        return Err(err);
    }
    Ok(Some(CowWorktreePaths {
        worktree_dir: canon_worktree_dir,
        worktree_admin_dir: admin_dir,
        worktrees_base: canon_base,
    }))
}

fn ensure_cow_template(
    repo: &gix::Repository,
    common_git_dir: &Path,
    base_id: gix::ObjectId,
) -> Result<Option<PathBuf>, GitEngineError> {
    if base_id.as_bytes().len() != 20 {
        return Ok(None);
    }
    let base_hex = base_id.to_string();
    let templates_root = common_git_dir.join(COW_TEMPLATES_DIR_NAME);
    fs::create_dir_all(&templates_root).map_err(|e| io_err(&templates_root, &e))?;
    if fs::symlink_metadata(&templates_root)
        .map_err(|e| io_err(&templates_root, &e))?
        .file_type()
        .is_symlink()
    {
        return Err(GitEngineError::VerificationInvalid(
            "gitust-cow-templates root must not be a symlink".into(),
        ));
    }
    let template_dir = templates_root.join(&base_hex);
    match fs::symlink_metadata(&template_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(GitEngineError::VerificationInvalid(
                    "CoW template directory must not be a symlink".into(),
                ));
            }
            if meta.is_dir()
                && template_dir.join("ready").is_file()
                && template_dir.join("tree").is_dir()
                && template_dir.join("admin").is_dir()
                && template_dir.join("index").is_file()
            {
                return Ok(Some(template_dir));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&template_dir, &e)),
    }

    let commit = repo
        .find_commit(base_id)
        .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
    let root_tree_id = commit
        .tree_id()
        .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
        .detach();

    let staging = templates_root.join(format!(".tmp-{base_hex}-{}", uuid::Uuid::new_v4()));
    let tree_dir = staging.join("tree");
    fs::create_dir_all(&tree_dir).map_err(|e| io_err(&tree_dir, &e))?;

    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;
    let populate_res = populate_template_tree(
        repo,
        root_tree_id,
        "",
        0,
        &tree_dir,
        &mut entries,
        &mut total_bytes,
    );

    match populate_res {
        Ok(true) => {}
        Ok(false) => {
            // fail-open-ok: removes staging directory when tree is ineligible for CoW; caller falls back to `git worktree add`.
            let _ = fs::remove_dir_all(&staging);
            return Ok(None);
        }
        Err(err) => {
            // fail-open-ok: removes staging directory after tree traversal error before propagating `Err(err)`.
            let _ = fs::remove_dir_all(&staging);
            return Err(err);
        }
    }

    entries.sort_by(|a, b| a.rel_path.as_bytes().cmp(b.rel_path.as_bytes()));
    let index_bytes = encode_git_index_v2(&entries);
    let index_path = staging.join("index");
    let admin_staging = staging.join("admin");
    let admin_index_path = admin_staging.join("index");
    let admin_commondir_path = admin_staging.join("commondir");
    let ready_path = staging.join("ready");
    if let Err(err) = fs::write(&index_path, &index_bytes)
        .map_err(|e| io_err(&index_path, &e))
        .and_then(|()| fs::create_dir_all(&admin_staging).map_err(|e| io_err(&admin_staging, &e)))
        .and_then(|()| {
            fs::write(&admin_index_path, &index_bytes).map_err(|e| io_err(&admin_index_path, &e))
        })
        .and_then(|()| {
            fs::write(&admin_commondir_path, b"../..\n")
                .map_err(|e| io_err(&admin_commondir_path, &e))
        })
        .and_then(|()| fs::write(&ready_path, b"1\n").map_err(|e| io_err(&ready_path, &e)))
    {
        // fail-open-ok: removes staging directory after write failure before propagating `Err(err)`.
        let _ = fs::remove_dir_all(&staging);
        return Err(err);
    }

    if template_dir.exists() {
        // fail-open-ok: removes stale/incomplete template directory under the worktree service lock before replacing it.
        let _ = fs::remove_dir_all(&template_dir);
    }
    if let Err(e) = fs::rename(&staging, &template_dir) {
        // fail-open-ok: removes staging directory after rename failure before propagating `Err`.
        let _ = fs::remove_dir_all(&staging);
        return Err(io_err(&template_dir, &e));
    }
    prune_excess_cow_templates(&templates_root, &base_hex);

    Ok(Some(template_dir))
}

fn prune_excess_cow_templates(templates_root: &Path, keep_hex: &str) {
    // fail-open-ok: bounded template cache eviction is an optimization under common_git_dir; worktree creation already succeeded.
    let Ok(rd) = fs::read_dir(templates_root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n != keep_hex && !n.starts_with('.'))
        })
        .collect();
    if dirs.len() >= MAX_CACHED_COW_TEMPLATES {
        dirs.sort();
        let drop_count = dirs.len() + 1 - MAX_CACHED_COW_TEMPLATES;
        for old in dirs.into_iter().take(drop_count) {
            // fail-open-ok: best-effort eviction of excess cached CoW template directory.
            let _ = fs::remove_dir_all(old);
        }
    }
}

fn populate_template_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    prefix: &str,
    depth: usize,
    fs_dir: &Path,
    entries: &mut Vec<IndexEntry>,
    total_bytes: &mut u64,
) -> Result<bool, GitEngineError> {
    if depth > MAX_COW_TREE_DEPTH {
        return Ok(false);
    }
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
    let mut seen_casefold = HashSet::new();

    for entry_res in tree.iter() {
        let entry = entry_res.map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        // fail-open-ok: returning Ok(false) rejects the fast-path template and falls back to git worktree checkout.
        let Ok(name) = std::str::from_utf8(entry.filename().as_ref()) else {
            return Ok(false);
        };
        if !is_safe_entry_name(name) || !seen_casefold.insert(name.to_ascii_lowercase()) {
            return Ok(false);
        }

        let rel_path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if rel_path.len() >= 0x0fff {
            return Ok(false);
        }

        let child_path = fs_dir.join(name);
        let entry_oid = entry.oid().to_owned();
        match entry.mode().kind() {
            EntryKind::Tree => {
                fs::create_dir(&child_path).map_err(|e| io_err(&child_path, &e))?;
                if !populate_template_tree(
                    repo,
                    entry_oid,
                    &rel_path,
                    depth + 1,
                    &child_path,
                    entries,
                    total_bytes,
                )? {
                    return Ok(false);
                }
            }
            EntryKind::Blob | EntryKind::BlobExecutable => {
                if entries.len() >= MAX_COW_TEMPLATE_FILES {
                    return Ok(false);
                }
                let executable = matches!(entry.mode().kind(), EntryKind::BlobExecutable);
                let header = repo
                    .objects
                    .header(entry_oid)
                    .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
                let hdr_size = header.size();
                if hdr_size > MAX_COW_TEMPLATE_BYTES
                    || hdr_size > u64::from(u32::MAX)
                    || total_bytes.saturating_add(hdr_size) > MAX_COW_TEMPLATE_BYTES
                {
                    return Ok(false);
                }
                let object = repo
                    .find_object(entry_oid)
                    .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
                let data = &object.data;
                let data_len = data.len() as u64;
                *total_bytes = total_bytes.saturating_add(data_len);
                if *total_bytes > MAX_COW_TEMPLATE_BYTES || data.len() > u32::MAX as usize {
                    return Ok(false);
                }
                fs::write(&child_path, data).map_err(|e| io_err(&child_path, &e))?;
                #[cfg(unix)]
                if executable {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&child_path, fs::Permissions::from_mode(0o755))
                        .map_err(|e| io_err(&child_path, &e))?;
                }
                let meta =
                    fs::symlink_metadata(&child_path).map_err(|e| io_err(&child_path, &e))?;
                let mut oid_bytes = [0_u8; 20];
                let raw_oid = entry_oid.as_bytes();
                if raw_oid.len() != 20 {
                    return Ok(false);
                }
                oid_bytes.copy_from_slice(raw_oid);
                entries.push(build_index_entry(
                    rel_path,
                    if executable { 0o100755 } else { 0o100644 },
                    data.len() as u32,
                    oid_bytes,
                    &meta,
                ));
            }
            EntryKind::Link | EntryKind::Commit => {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn is_safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case(".git")
        && !name.eq_ignore_ascii_case(".gitattributes")
        && !name.eq_ignore_ascii_case(".gitmodules")
        && !name.contains(['/', '\\', '\0'])
}

#[cfg(unix)]
fn build_index_entry(
    rel_path: String,
    mode: u32,
    size: u32,
    oid: [u8; 20],
    meta: &fs::Metadata,
) -> IndexEntry {
    use std::os::unix::fs::MetadataExt;
    IndexEntry {
        rel_path,
        stat_words: [
            meta.ctime() as u32,
            meta.ctime_nsec() as u32,
            meta.mtime() as u32,
            meta.mtime_nsec() as u32,
            meta.dev() as u32,
            meta.ino() as u32,
            mode,
            meta.uid(),
            meta.gid(),
            size,
        ],
        oid,
    }
}

#[cfg(not(unix))]
fn build_index_entry(
    rel_path: String,
    mode: u32,
    size: u32,
    oid: [u8; 20],
    _meta: &fs::Metadata,
) -> IndexEntry {
    IndexEntry {
        rel_path,
        stat_words: [0, 0, 0, 0, 0, 0, mode, 0, 0, size],
        oid,
    }
}

fn encode_git_index_v2(entries: &[IndexEntry]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(12 + entries.len() * 80 + 20);
    buf.extend_from_slice(b"DIRC");
    buf.extend_from_slice(&2_u32.to_be_bytes());
    buf.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for entry in entries {
        for word in entry.stat_words {
            buf.extend_from_slice(&word.to_be_bytes());
        }
        buf.extend_from_slice(&entry.oid);
        let path_bytes = entry.rel_path.as_bytes();
        let flags = (path_bytes.len().min(0x0fff)) as u16;
        buf.extend_from_slice(&flags.to_be_bytes());
        buf.extend_from_slice(path_bytes);
        let pad_len = 8 - ((62 + path_bytes.len()) % 8);
        buf.resize(buf.len() + pad_len, 0);
    }
    let checksum = sha1_digest(&buf);
    buf.extend_from_slice(&checksum);
    buf
}

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h = [
        0x6745_2301_u32,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 72) & !63);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0_u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, temp);
        }
        for (hi, add) in h.iter_mut().zip([a, b, c, d, e]) {
            *hi = hi.wrapping_add(add);
        }
    }
    let mut out = [0_u8; 20];
    for (idx, word) in h.into_iter().enumerate() {
        out[idx * 4..(idx + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn clone_tree_cow(src: &Path, dst: &Path) -> Result<(), GitEngineError> {
    #[cfg(target_os = "macos")]
    {
        if try_macos_clonefile(src, dst)? {
            return Ok(());
        }
    }
    copy_dir_recursive(src, dst)
}

#[cfg(target_os = "macos")]
fn try_macos_clonefile(src: &Path, dst: &Path) -> Result<bool, GitEngineError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_src = CString::new(src.as_os_str().as_bytes()).map_err(|_| GitEngineError::Io {
        path: src.display().to_string(),
        message: "interior NUL in source path".to_owned(),
    })?;
    let c_dst = CString::new(dst.as_os_str().as_bytes()).map_err(|_| GitEngineError::Io {
        path: dst.display().to_string(),
        message: "interior NUL in destination path".to_owned(),
    })?;
    let rc = unsafe { libc::clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if matches!(err.raw_os_error(), Some(libc::ENOTSUP | libc::EXDEV)) {
        return Ok(false);
    }
    Err(io_err(dst, &err))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), GitEngineError> {
    fs::create_dir(dst).map_err(|e| io_err(dst, &e))?;
    let read_dir = fs::read_dir(src).map_err(|e| io_err(src, &e))?;
    for entry_res in read_dir {
        let entry = entry_res.map_err(|e| io_err(src, &e))?;
        let file_type = entry.file_type().map_err(|e| io_err(&entry.path(), &e))?;
        let src_child = entry.path();
        let dst_child = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_child, &dst_child)?;
        } else if file_type.is_file() {
            copy_file_cow_or_fallback(&src_child, &dst_child)?;
        } else {
            return Err(GitEngineError::Io {
                path: src_child.display().to_string(),
                message: "unexpected non-regular file in CoW template".to_owned(),
            });
        }
    }
    Ok(())
}

fn copy_file_cow_or_fallback(src: &Path, dst: &Path) -> Result<(), GitEngineError> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let src_file = fs::File::open(src).map_err(|e| io_err(src, &e))?;
        let dst_file = fs::File::create(dst).map_err(|e| io_err(dst, &e))?;
        // fail-open-ok: FICLONE is an opportunistic reflink optimization; regular copy below preserves identical file contents and permissions when reflink is unsupported.
        let rc = unsafe { libc::ioctl(dst_file.as_raw_fd(), libc::FICLONE, src_file.as_raw_fd()) };
        if rc == 0 {
            let meta = src_file.metadata().map_err(|e| io_err(src, &e))?;
            dst_file
                .set_permissions(meta.permissions())
                .map_err(|e| io_err(dst, &e))?;
            return Ok(());
        }
    }
    fs::copy(src, dst).map_err(|e| io_err(dst, &e))?;
    Ok(())
}
