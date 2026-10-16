//! Step 2-4: Unified Diff Engine (`gitoxide` OID resolution + bounded `SIGPIPE`-safe `git diff`).

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Instant;

use gitust_core::CommitSha;
use serde::{Deserialize, Serialize};

use crate::{GitEngine, GitEngineError};

/// Maximum unified diff byte payload read from `git diff` (1 MiB).
pub const MAX_DIFF_BYTES: usize = 1_048_576;
/// Maximum parsed diff lines returned in a `RepoDiffSnapshot`.
pub const MAX_DIFF_LINES: usize = 10_000;

/// Git's well-known SHA-1 of the empty tree object.
const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Hunk,
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDiffLine {
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDiffFileSummary {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDiffSnapshot {
    pub base_sha: CommitSha,
    pub head_sha: CommitSha,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
    pub files: Vec<RepoDiffFileSummary>,
    pub lines: Vec<RepoDiffLine>,
    pub truncated: bool,
    pub lookup_micros: u64,
}

impl GitEngine {
    /// Computes a structured unified diff between `base_rev` and `head_rev` using in-process
    /// `gix` OID resolution and bounded `git diff` execution.
    pub fn inspect_diff(
        &self,
        base_rev: Option<&str>,
        head_rev: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<RepoDiffSnapshot, GitEngineError> {
        let start = Instant::now();
        let clean_path = match file_path {
            Some(p) if !p.trim().is_empty() => Some(Self::validate_repo_relative_path(p)?),
            _ => None,
        };

        let repo = self.acquire_repo()?;
        let (_, head_commit) = Self::resolve_commit(&repo, head_rev)?;
        let head_sha = head_commit.id.to_string();

        let base_sha = match base_rev.map(str::trim).filter(|s| !s.is_empty()) {
            Some(explicit_base) => {
                let (_, b_commit) = Self::resolve_commit(&repo, Some(explicit_base))?;
                b_commit.id.to_string()
            }
            None => head_commit
                .parent_ids()
                .next()
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| EMPTY_TREE_SHA1.to_owned()),
        };

        let mut cmd = Command::new("git");
        cmd.arg("--literal-pathspecs")
            .arg("-C")
            .arg(&self.repo_path)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                base_sha.as_str(),
                head_sha.as_str(),
                "--",
            ])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(ref p) = clean_path {
            cmd.arg(p.as_str());
        }

        let mut child = cmd.spawn().map_err(|e| GitEngineError::GitCommand {
            args: "diff".to_owned(),
            message: e.to_string(),
        })?;

        let mut raw_diff = Vec::new();
        let mut truncated = false;
        if let Some(stdout) = child.stdout.take() {
            let mut limited = stdout.take((MAX_DIFF_BYTES + 1) as u64);
            limited
                .read_to_end(&mut raw_diff)
                .map_err(|e| GitEngineError::GitCommand {
                    args: "diff".to_owned(),
                    message: e.to_string(),
                })?;
            if raw_diff.len() > MAX_DIFF_BYTES {
                raw_diff.truncate(MAX_DIFF_BYTES);
                truncated = true;
                let mut rest = limited.into_inner();
                // fail-open-ok: drains remaining `git diff` stdout to sink after 1 MiB cap to avoid SIGPIPE (exit 141); child exit status is verified below.
                let _ = std::io::copy(&mut rest, &mut std::io::sink());
            }
        }
        let status = child.wait().map_err(|e| GitEngineError::GitCommand {
            args: "diff".to_owned(),
            message: e.to_string(),
        })?;
        if !status.success() {
            return Err(GitEngineError::GitCommand {
                args: format!("diff {base_sha} {head_sha}"),
                message: format!("exit status {status}"),
            });
        }

        let diff_text = String::from_utf8_lossy(&raw_diff);
        let (files, lines, additions, deletions, line_truncated) =
            parse_unified_diff(&diff_text, truncated);
        let files_changed = u32::try_from(files.len()).unwrap_or(u32::MAX);
        let lookup_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);

        Ok(RepoDiffSnapshot {
            base_sha: CommitSha(base_sha),
            head_sha: CommitSha(head_sha),
            files_changed,
            additions,
            deletions,
            files,
            lines,
            truncated: line_truncated,
            lookup_micros,
        })
    }
}

fn parse_unified_diff(
    text: &str,
    byte_truncated: bool,
) -> (Vec<RepoDiffFileSummary>, Vec<RepoDiffLine>, u32, u32, bool) {
    let mut files: Vec<RepoDiffFileSummary> = Vec::new();
    let mut lines: Vec<RepoDiffLine> = Vec::new();
    let mut additions: u32 = 0;
    let mut deletions: u32 = 0;
    let mut old_ln: u32 = 0;
    let mut new_ln: u32 = 0;
    let mut rem_old: u32 = 0;
    let mut rem_new: u32 = 0;
    let mut truncated = byte_truncated;

    let mut push_line = |old_line, new_line, kind, content: &str| {
        if lines.len() < MAX_DIFF_LINES {
            lines.push(RepoDiffLine {
                old_line,
                new_line,
                kind,
                content: content.to_owned(),
            });
        } else {
            truncated = true;
        }
    };

    for raw_line in text.lines() {
        let in_hunk = rem_old > 0 || rem_new > 0;
        if in_hunk {
            if let Some(added) = raw_line.strip_prefix('+') {
                if rem_new > 0 {
                    rem_new -= 1;
                    additions = additions.saturating_add(1);
                    if let Some(last_file) = files.last_mut() {
                        last_file.additions = last_file.additions.saturating_add(1);
                    }
                    let cur_new = new_ln;
                    new_ln = new_ln.saturating_add(1);
                    push_line(None, Some(cur_new), DiffLineKind::Add, added);
                    continue;
                }
            } else if let Some(removed) = raw_line.strip_prefix('-') {
                if rem_old > 0 {
                    rem_old -= 1;
                    deletions = deletions.saturating_add(1);
                    if let Some(last_file) = files.last_mut() {
                        last_file.deletions = last_file.deletions.saturating_add(1);
                    }
                    let cur_old = old_ln;
                    old_ln = old_ln.saturating_add(1);
                    push_line(Some(cur_old), None, DiffLineKind::Del, removed);
                    continue;
                }
            } else if let Some(ctx) = raw_line.strip_prefix(' ') {
                rem_old = rem_old.saturating_sub(1);
                rem_new = rem_new.saturating_sub(1);
                let (cur_old, cur_new) = (old_ln, new_ln);
                old_ln = old_ln.saturating_add(1);
                new_ln = new_ln.saturating_add(1);
                push_line(Some(cur_old), Some(cur_new), DiffLineKind::Context, ctx);
                continue;
            } else if raw_line.starts_with('\\') {
                continue;
            }
            rem_old = 0;
            rem_new = 0;
        }

        if let Some(rest) = raw_line.strip_prefix("diff --git ") {
            let path = parse_diff_git_b_path(rest);
            files.push(RepoDiffFileSummary {
                path,
                additions: 0,
                deletions: 0,
            });
        } else if let Some(rhs) = raw_line.strip_prefix("+++ ") {
            if let Some(b_path) = parse_plus_header_b_path(rhs) {
                if let Some(last_file) = files.last_mut() {
                    last_file.path = b_path;
                }
            }
        } else if raw_line.starts_with("--- ")
            || raw_line.starts_with("index ")
            || raw_line.starts_with("new file mode ")
            || raw_line.starts_with("deleted file mode ")
        {
            continue;
        } else if raw_line.starts_with("@@ ") {
            (old_ln, rem_old, new_ln, rem_new) = parse_hunk_header(raw_line);
            push_line(None, None, DiffLineKind::Hunk, raw_line);
        }
    }
    (files, lines, additions, deletions, truncated)
}

fn parse_diff_git_b_path(rest: &str) -> String {
    let trimmed = rest.trim_end_matches('\t');
    if trimmed.ends_with('"') {
        let bytes = trimmed.as_bytes();
        let mut idx = bytes.len() - 1;
        while idx > 0 {
            idx -= 1;
            if bytes[idx] == b'"' {
                let mut backslashes = 0usize;
                let mut k = idx;
                while k > 0 && bytes[k - 1] == b'\\' {
                    backslashes += 1;
                    k -= 1;
                }
                if backslashes.is_multiple_of(2) {
                    let unquoted = unquote_git_c_path(&trimmed[idx..]);
                    return unquoted.strip_prefix("b/").unwrap_or(&unquoted).to_owned();
                }
            }
        }
    }
    trimmed
        .rsplit_once(" b/")
        .map(|(_, b)| b)
        .or_else(|| {
            trimmed
                .split_whitespace()
                .last()
                .and_then(|b| b.strip_prefix("b/").or(Some(b)))
        })
        .unwrap_or(trimmed)
        .to_owned()
}

fn parse_plus_header_b_path(rhs: &str) -> Option<String> {
    let trimmed = rhs.trim_end_matches('\t');
    if trimmed == "/dev/null" {
        return None;
    }
    let unquoted = unquote_git_c_path(trimmed);
    unquoted.strip_prefix("b/").map(str::to_owned)
}

fn unquote_git_c_path(raw: &str) -> String {
    let Some(inner) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return raw.to_owned();
    };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'"' => out.push(b'"'),
                b'\\' => out.push(b'\\'),
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'a' => out.push(0x07),
                b'b' => out.push(0x08),
                b'f' => out.push(0x0c),
                b'v' => out.push(0x0b),
                d0 @ b'0'..=b'7' => {
                    let mut val = u32::from(d0 - b'0');
                    for _ in 0..2 {
                        if i + 1 < bytes.len() && (b'0'..=b'7').contains(&bytes[i + 1]) {
                            i += 1;
                            val = (val << 3) | u32::from(bytes[i] - b'0');
                        } else {
                            break;
                        }
                    }
                    out.push((val & 0xff) as u8);
                }
                other => out.push(other),
            }
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_hunk_header(header: &str) -> (u32, u32, u32, u32) {
    let (mut old_start, mut old_count, mut new_start, mut new_count) = (1, 0, 1, 0);
    for token in header.split_whitespace() {
        if let Some(rest) = token.strip_prefix('-') {
            let mut parts = rest.split(',');
            // fail-open-ok: unified diff hunk header display line numbers default to 1 when count is omitted (`@@ -1 +1 @@`).
            old_start = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            // fail-open-ok: unified diff hunk header omits `,count` when count is 1.
            old_count = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        } else if let Some(rest) = token.strip_prefix('+') {
            let mut parts = rest.split(',');
            // fail-open-ok: unified diff hunk header display line numbers default to 1.
            new_start = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            // fail-open-ok: unified diff hunk header omits `,count` when count is 1.
            new_count = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            break;
        }
    }
    (old_start, old_count, new_start, new_count)
}
