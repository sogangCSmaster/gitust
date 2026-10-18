//! Commit-pinned trigram code and symbol search index engine (`docs/roadmap.md` §4.5).

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use gitust_core::CommitSha;
use gix::prelude::HeaderExt;
use serde::{Deserialize, Serialize};

use crate::{io_err, GitEngine, GitEngineError};

pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
pub const MAX_SEARCH_RESULTS: usize = 50;
pub const MAX_INDEXED_FILES: usize = 1_024;
pub const MAX_INDEXED_BLOB_BYTES: usize = 256 * 1024;
const MAX_TREE_DEPTH: usize = 32;
const MAX_CACHED_COMMIT_INDEXES: usize = 4;
const SEARCH_DIR_NAME: &str = "gitust-search";
const MAX_SNIPPET_CHARS: usize = 240;

static SEARCH_INDEX_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchKind {
    #[default]
    All,
    Code,
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Interface,
    Class,
    Type,
    Const,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySearchHit {
    pub path: String,
    pub line_number: usize,
    pub snippet: String,
    pub match_kind: SearchMatchKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<SymbolKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySearchReport {
    pub revision: String,
    pub commit_sha: CommitSha,
    pub query: String,
    pub kind: SearchMatchKind,
    pub indexed_file_count: usize,
    pub index_cache_hit: bool,
    pub hits: Vec<RepositorySearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedSymbol {
    line_number: usize,
    snippet: String,
    name: String,
    kind: SymbolKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedFileRecord {
    path: String,
    trigrams: Vec<u32>,
    symbols: Vec<IndexedSymbol>,
    lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCommitSearchIndex {
    version: u32,
    commit_sha: CommitSha,
    files: Vec<IndexedFileRecord>,
}

fn validate_search_query(raw: &str) -> Result<String, GitEngineError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_SEARCH_QUERY_BYTES
        || trimmed.chars().any(|c| c.is_control() && c != '\t')
    {
        return Err(GitEngineError::InvalidRevision(raw.to_owned()));
    }
    Ok(trimmed.to_owned())
}

fn pack_trigram(w: &[u8]) -> u32 {
    let [a, b, c] = [
        w[0].to_ascii_lowercase(),
        w[1].to_ascii_lowercase(),
        w[2].to_ascii_lowercase(),
    ];
    ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

fn extract_trigrams(lines: &[String]) -> Vec<u32> {
    let mut trigrams: Vec<u32> = lines
        .iter()
        .flat_map(|line| line.as_bytes().windows(3).map(pack_trigram))
        .collect();
    trigrams.sort_unstable();
    trigrams.dedup();
    trigrams
}

fn file_matches_query_trigrams(file_trigrams: &[u32], query_lower: &str) -> bool {
    query_lower
        .as_bytes()
        .windows(3)
        .all(|w| file_trigrams.binary_search(&pack_trigram(w)).is_ok())
}

fn truncate_snippet(line: &str) -> String {
    line.trim().chars().take(MAX_SNIPPET_CHARS).collect()
}

fn strip_leading_modifiers(mut s: &str) -> &str {
    loop {
        let trimmed = s.trim_start();
        if let Some((after_paren, close_idx)) = trimmed
            .strip_prefix("pub(")
            .and_then(|rest| Some((rest, rest.find(')')?)))
        {
            s = &after_paren[close_idx + 1..];
            continue;
        }
        if let Some(rest) = ["pub", "export", "default", "async", "unsafe"]
            .into_iter()
            .find_map(|kw| {
                trimmed
                    .strip_prefix(kw)
                    .filter(|r| r.starts_with(char::is_whitespace))
            })
        {
            s = rest;
            continue;
        }
        if let Some(after_const) = trimmed.strip_prefix("const").map(str::trim_start) {
            if after_const.starts_with("fn ") || after_const.starts_with("unsafe ") {
                s = after_const;
                continue;
            }
        }
        return trimmed;
    }
}

const SYMBOL_KEYWORDS: &[(&str, SymbolKind)] = &[
    ("function", SymbolKind::Function),
    ("interface", SymbolKind::Interface),
    ("struct", SymbolKind::Struct),
    ("static", SymbolKind::Const),
    ("const", SymbolKind::Const),
    ("trait", SymbolKind::Trait),
    ("class", SymbolKind::Class),
    ("enum", SymbolKind::Enum),
    ("type", SymbolKind::Type),
    ("func", SymbolKind::Function),
    ("def", SymbolKind::Function),
    ("fn", SymbolKind::Function),
];

fn extract_symbol_from_line(line: &str) -> Option<(String, SymbolKind)> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || ["//", "#", "/*", "*"]
            .iter()
            .any(|p| trimmed.starts_with(p))
    {
        return None;
    }
    let rest = strip_leading_modifiers(trimmed);
    for &(kw, kind) in SYMBOL_KEYWORDS {
        let Some(after_kw) = rest
            .strip_prefix(kw)
            .filter(|r| r.starts_with(char::is_whitespace))
        else {
            continue;
        };
        let mut candidate = after_kw.trim_start();
        if kw == "func" && candidate.starts_with('(') {
            candidate = candidate[candidate.find(')')? + 1..].trim_start();
        } else if kw == "function" && candidate.starts_with('*') {
            candidate = candidate[1..].trim_start();
        }
        let ident: String = candidate
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if ident
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Some((ident, kind));
        }
    }
    None
}

fn ensure_search_dir(common_git_dir: &Path) -> Result<PathBuf, GitEngineError> {
    let dir = common_git_dir.join(SEARCH_DIR_NAME);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(dir),
        Ok(_) => Err(GitEngineError::Io {
            path: dir.display().to_string(),
            message: "gitust-search must be a real directory".into(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&dir).map_err(|err| io_err(&dir, &err))?;
            Ok(dir)
        }
        Err(e) => Err(io_err(&dir, &e)),
    }
}

fn collect_indexed_files(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    depth: usize,
    out: &mut Vec<IndexedFileRecord>,
) -> Result<(), GitEngineError> {
    if depth > MAX_TREE_DEPTH || out.len() >= MAX_INDEXED_FILES {
        return Ok(());
    }
    for entry_res in tree.iter() {
        if out.len() >= MAX_INDEXED_FILES {
            break;
        }
        let entry = entry_res.map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        let filename = entry.filename().to_string();
        let path = if prefix.is_empty() {
            filename
        } else {
            format!("{prefix}/{filename}")
        };
        let mode = entry.mode();
        if mode.is_tree() {
            if depth < MAX_TREE_DEPTH {
                let child_tree = repo
                    .find_object(entry.oid())
                    .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
                    .try_into_tree()
                    .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
                collect_indexed_files(repo, &child_tree, &path, depth + 1, out)?;
            }
            continue;
        }
        if !mode.is_blob() || mode.is_link() {
            continue;
        }
        let header = repo
            .objects
            .header(entry.oid())
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        if header.kind() != gix::objs::Kind::Blob || header.size() > MAX_INDEXED_BLOB_BYTES as u64 {
            continue;
        }
        let obj = entry
            .object()
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        if obj.data.len() > MAX_INDEXED_BLOB_BYTES || obj.data.contains(&0) {
            continue;
        }
        // fail-open-ok: non-UTF-8 blobs are skipped during text indexing.
        let Ok(text) = std::str::from_utf8(&obj.data) else {
            continue;
        };
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let trigrams = extract_trigrams(&lines);
        let symbols = lines
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| {
                let (name, kind) = extract_symbol_from_line(line)?;
                Some(IndexedSymbol {
                    line_number: idx + 1,
                    snippet: truncate_snippet(line),
                    name,
                    kind,
                })
            })
            .collect();
        out.push(IndexedFileRecord {
            path,
            trigrams,
            symbols,
            lines,
        });
    }
    Ok(())
}

fn persist_and_prune_index(
    search_dir: &Path,
    index_path: &Path,
    index: &StoredCommitSearchIndex,
) -> Result<(), GitEngineError> {
    let tmp_path = search_dir.join(format!("index-{}.tmp", index.commit_sha.0));
    match fs::symlink_metadata(&tmp_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(GitEngineError::Io {
                path: tmp_path.display().to_string(),
                message: "refusing symlink temp search index".into(),
            });
        }
        Ok(_) => fs::remove_file(&tmp_path).map_err(|e| io_err(&tmp_path, &e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&tmp_path, &e)),
    }
    let payload = serde_json::to_vec(index)
        .map_err(|e| GitEngineError::TreeTraversal(format!("serialize search index: {e}")))?;
    {
        let mut file = File::create(&tmp_path).map_err(|e| io_err(&tmp_path, &e))?;
        file.write_all(&payload)
            .map_err(|e| io_err(&tmp_path, &e))?;
        file.sync_all().map_err(|e| io_err(&tmp_path, &e))?;
    }
    fs::rename(&tmp_path, index_path).map_err(|e| io_err(index_path, &e))?;

    let mut cached_files: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry_res in fs::read_dir(search_dir).map_err(|e| io_err(search_dir, &e))? {
        let entry = entry_res.map_err(|e| io_err(search_dir, &e))?;
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("index-") || !name.ends_with(".json") {
            continue;
        }
        let meta = fs::symlink_metadata(&p).map_err(|e| io_err(&p, &e))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        // fail-open-ok: mtime ordering fallback for cache eviction only; never affects authorization or git state.
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        cached_files.push((modified, p));
    }
    if cached_files.len() > MAX_CACHED_COMMIT_INDEXES {
        cached_files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let remove_count = cached_files.len() - MAX_CACHED_COMMIT_INDEXES;
        for (_, old_path) in cached_files.into_iter().take(remove_count) {
            if old_path != index_path {
                // fail-open-ok: best-effort old search index cache eviction.
                let _ = fs::remove_file(old_path);
            }
        }
    }
    Ok(())
}

fn is_agent_ref_name(raw_ref: &str) -> bool {
    let full = raw_ref.to_ascii_lowercase();
    if full.starts_with("refs/")
        && !full.starts_with("refs/heads/")
        && !full.starts_with("refs/tags/")
    {
        return true;
    }
    // fail-open-ok: normalizes optional refs/heads/ prefix before agent-branch rejection.
    let short = full.strip_prefix("refs/heads/").unwrap_or(&full);
    short.starts_with("agent/") || full.contains("/agent/")
}

fn is_head_disallowed(repo: &gix::Repository) -> Result<bool, GitEngineError> {
    let head = repo
        .head()
        .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
    Ok(head
        .referent_name()
        .is_none_or(|r| is_agent_ref_name(&String::from_utf8_lossy(r.as_bstr()))))
}

fn ensure_commit_reachable_from_non_agent_ref(
    repo: &gix::Repository,
    target_id: gix::ObjectId,
    clean_rev: &str,
) -> Result<(), GitEngineError> {
    let mut allowed_tips = Vec::new();
    let mut head = repo
        .head()
        .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
    if !is_head_disallowed(repo)? && !head.is_unborn() {
        let commit = head
            .peel_to_commit_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        if commit.id == target_id {
            return Ok(());
        }
        allowed_tips.push(commit.id);
    }
    let refs = repo
        .references()
        .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
    for r_res in refs
        .all()
        .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
    {
        let mut r = r_res.map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        if is_agent_ref_name(&String::from_utf8_lossy(r.name().as_bstr())) {
            continue;
        }
        // fail-open-ok: non-commit references (such as blob/tree tags) are skipped when collecting commit tips.
        if let Ok(commit) = r.peel_to_commit() {
            if commit.id == target_id {
                return Ok(());
            }
            allowed_tips.push(commit.id);
        }
    }
    if allowed_tips.is_empty() {
        return Err(GitEngineError::InvalidRevision(clean_rev.to_owned()));
    }
    for info_res in repo
        .rev_walk(allowed_tips)
        .all()
        .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
    {
        if info_res
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .id
            == target_id
        {
            return Ok(());
        }
    }
    Err(GitEngineError::InvalidRevision(clean_rev.to_owned()))
}

impl GitEngine {
    pub fn search_repository(
        &self,
        revision: Option<&str>,
        query: &str,
        kind: SearchMatchKind,
    ) -> Result<RepositorySearchReport, GitEngineError> {
        let clean_query = validate_search_query(query)?;
        let clean_rev = Self::validate_revision_spec(revision)?;
        // fail-open-ok: a revision without ancestry suffix is parsed as the whole validated anchor.
        let (anchor, _) = clean_rev.split_at(clean_rev.find(['~', '^']).unwrap_or(clean_rev.len()));
        if is_agent_ref_name(anchor) {
            return Err(GitEngineError::InvalidRevision(clean_rev));
        }
        let repo = self.acquire_repo()?;
        if anchor == "HEAD" && is_head_disallowed(&repo)? {
            return Err(GitEngineError::InvalidRevision(clean_rev));
        }
        let (resolved_rev, commit) = Self::resolve_commit(&repo, Some(&clean_rev))?;
        ensure_commit_reachable_from_non_agent_ref(&repo, commit.id, &clean_rev)?;
        let commit_sha = CommitSha(commit.id.to_string());
        let common_git_dir = Self::common_git_dir(&repo);

        let cached = {
            let _guard = SEARCH_INDEX_LOCK
                .lock()
                .map_err(|_| GitEngineError::TreeTraversal("search index lock poisoned".into()))?;
            let search_dir = ensure_search_dir(&common_git_dir)?;
            let index_path = search_dir.join(format!("index-{}.json", commit_sha.0));
            match fs::symlink_metadata(&index_path) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
                    return Err(GitEngineError::Io {
                        path: index_path.display().to_string(),
                        message: "search index must be a regular file".into(),
                    });
                }
                Ok(_) => {
                    let bytes = fs::read(&index_path).map_err(|e| io_err(&index_path, &e))?;
                    let stored: StoredCommitSearchIndex = serde_json::from_slice(&bytes)
                        .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
                    if stored.version != 1 || stored.commit_sha != commit_sha {
                        return Err(GitEngineError::TreeTraversal(
                            "search index mismatch".into(),
                        ));
                    }
                    Ok(stored)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err((search_dir, index_path)),
                Err(e) => return Err(io_err(&index_path, &e)),
            }
        };
        let (index, index_cache_hit) = match cached {
            Ok(stored) => (stored, true),
            Err((search_dir, index_path)) => {
                let root = commit
                    .tree()
                    .map_err(|err| GitEngineError::TreeTraversal(err.to_string()))?;
                let mut files = Vec::new();
                collect_indexed_files(&repo, &root, "", 0, &mut files)?;
                files.sort_by(|a, b| a.path.cmp(&b.path));
                let stored = StoredCommitSearchIndex {
                    version: 1,
                    commit_sha: commit_sha.clone(),
                    files,
                };
                let _guard = SEARCH_INDEX_LOCK.lock().map_err(|_| {
                    GitEngineError::TreeTraversal("search index lock poisoned".into())
                })?;
                persist_and_prune_index(&search_dir, &index_path, &stored)?;
                (stored, false)
            }
        };

        let query_lower = clean_query.to_ascii_lowercase();
        let mut hits = Vec::new();
        let mut emitted = BTreeSet::new();
        if matches!(kind, SearchMatchKind::All | SearchMatchKind::Symbol) {
            let mut ranked = Vec::new();
            for file in &index.files {
                for sym in &file.symbols {
                    let s = sym.name.to_ascii_lowercase();
                    let Some(rank) = (s == query_lower)
                        .then_some(0_u8)
                        .or_else(|| s.starts_with(&query_lower).then_some(1))
                        .or_else(|| s.contains(&query_lower).then_some(2))
                    else {
                        continue;
                    };
                    ranked.push((rank, file.path.clone(), sym.line_number, sym));
                }
            }
            ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            for (_, path, line_number, sym) in ranked.into_iter().take(MAX_SEARCH_RESULTS) {
                emitted.insert((path.clone(), line_number));
                hits.push(RepositorySearchHit {
                    path,
                    line_number,
                    snippet: sym.snippet.clone(),
                    match_kind: SearchMatchKind::Symbol,
                    symbol_name: Some(sym.name.clone()),
                    symbol_kind: Some(sym.kind),
                });
            }
        }
        if matches!(kind, SearchMatchKind::All | SearchMatchKind::Code) {
            'outer: for file in &index.files {
                if query_lower.len() >= 3
                    && !file_matches_query_trigrams(&file.trigrams, &query_lower)
                {
                    continue;
                }
                for (idx, line) in file.lines.iter().enumerate() {
                    if hits.len() >= MAX_SEARCH_RESULTS {
                        break 'outer;
                    }
                    let line_number = idx + 1;
                    if !emitted.contains(&(file.path.clone(), line_number))
                        && line.to_ascii_lowercase().contains(&query_lower)
                    {
                        hits.push(RepositorySearchHit {
                            path: file.path.clone(),
                            line_number,
                            snippet: truncate_snippet(line),
                            match_kind: SearchMatchKind::Code,
                            symbol_name: None,
                            symbol_kind: None,
                        });
                    }
                }
            }
        }
        Ok(RepositorySearchReport {
            revision: resolved_rev,
            commit_sha,
            query: clean_query,
            kind,
            indexed_file_count: index.files.len(),
            index_cache_hit,
            hits,
        })
    }
}
