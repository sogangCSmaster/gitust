//! Bounded local-branch keysets; default HEAD and branch tips are independent identities.
use crate::{GitEngine, GitEngineError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use gitust_core::CommitSha;
use gix::prelude::HeaderExt;
use gix::refs::file::ReferenceExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const PREFIX: &str = "refs/heads/";
pub const MAX_BRANCH_NAME_BYTES: usize = 32 * 1024;
pub const MAX_BRANCH_CURSOR_BYTES: usize =
    3 + ((MAX_BRANCH_NAME_BYTES + PREFIX.len()) * 4).div_ceil(3);
pub const MAX_BRANCH_PAGE_BYTES: usize = 1024 * 1024;
const MAX_SCANNED_HEADS: usize = 10_000;
const EMPTY_PAGE_JSON_BYTES: usize = br#"{"branches":[],"next_cursor":null}"#.len();
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoBranch {
    pub name: String,
    pub ref_name: String,
    pub commit_sha: CommitSha,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoBranchPage {
    pub branches: Vec<RepoBranch>,
    pub next_cursor: Option<String>,
}
fn unavailable() -> GitEngineError {
    GitEngineError::RefTransaction(
        "local branch data is unavailable or exceeds its supported bounds".into(),
    )
}
fn invalid() -> GitEngineError {
    GitEngineError::InvalidRevision("invalid branch page limit or cursor".into())
}
fn canonical(full: &str) -> bool {
    full.strip_prefix(PREFIX)
        .is_some_and(|name| !name.is_empty() && name.len() <= MAX_BRANCH_NAME_BYTES)
        && <&gix::refs::FullNameRef>::try_from(full).is_ok()
}
fn encode(full: &str) -> String {
    format!("v1.{}", URL_SAFE_NO_PAD.encode(full.as_bytes()))
}
fn decode(cursor: &str) -> Result<String, GitEngineError> {
    if cursor.len() > MAX_BRANCH_CURSOR_BYTES {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor.strip_prefix("v1.").ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let full = String::from_utf8(bytes).map_err(|_| invalid())?;
    if !canonical(&full) || encode(&full) != cursor {
        return Err(invalid());
    }
    Ok(full)
}
impl GitEngine {
    /// A cursor is an exclusive UTF-8 byte-order key, not a snapshot or branch authority.
    /// Includes agent heads under the same repository read ACL; excludes tags/remotes.
    pub fn list_local_branches(
        &self,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<RepoBranchPage, GitEngineError> {
        if !(1..=200).contains(&limit) {
            return Err(invalid());
        }
        let after = cursor.map(decode).transpose()?;
        let repo = self.acquire_repo()?;
        let mut names: BTreeMap<String, gix::refs::Target> = BTreeMap::new();
        {
            let platform = repo.references().map_err(|_| unavailable())?;
            let references = platform.local_branches().map_err(|_| unavailable())?;
            // Do not assume the loose/packed iterator's ordering and never sort an unbounded map.
            for (index, reference) in references.enumerate() {
                if index >= MAX_SCANNED_HEADS {
                    return Err(unavailable());
                }
                let reference = reference.map_err(|_| unavailable())?;
                let full =
                    std::str::from_utf8(reference.name().as_bstr()).map_err(|_| unavailable())?;
                if !canonical(full) {
                    return Err(unavailable());
                }
                if after.as_ref().is_some_and(|key| full <= key.as_str()) {
                    continue;
                }
                if names.contains_key(full) {
                    continue;
                }
                if names.len() == limit + 1 {
                    if names
                        .last_key_value()
                        .is_some_and(|(last, _)| full >= last.as_str())
                    {
                        continue;
                    }
                    names.pop_last();
                }
                let full = full.to_owned();
                names.insert(full, reference.detach().target);
            }
        }
        // Release the iterator's packed-ref snapshot before resolving symbolic targets.
        let packed = repo
            .refs
            .cached_packed_buffer()
            .map_err(|_| unavailable())?;
        let mut rows = Vec::with_capacity(names.len().min(limit));
        let mut row_bytes = 0usize;
        let mut more = false;
        for (full, target) in names {
            if rows.len() == limit {
                more = true;
                break;
            }
            // Preserve the iterator's target: looking up a long packed name as a loose path
            // can hit filesystem component limits even though the packed ref is valid.
            let mut reference = gix::refs::Reference {
                name: full.as_str().try_into().map_err(|_| unavailable())?,
                target,
                peeled: None,
            };
            let oid = reference
                .follow_to_object_in_place_packed(&repo.refs, packed.as_ref().map(|p| &***p))
                .map_err(|_| unavailable())?;
            let header = repo.objects.header(oid).map_err(|_| unavailable())?;
            if header.kind() != gix::objs::Kind::Commit {
                return Err(unavailable());
            }
            let row = RepoBranch {
                name: full[PREFIX.len()..].to_owned(),
                ref_name: full,
                commit_sha: CommitSha(oid.to_string()),
            };
            let bytes = serde_json::to_vec(&row).map_err(|_| unavailable())?.len();
            let cursor = encode(&row.ref_name);
            // Include exact JSON encoding and a conservative next-cursor reservation before admitting a row.
            if row_bytes + bytes + rows.len() + EMPTY_PAGE_JSON_BYTES - 4
                + serde_json::to_vec(&cursor)
                    .map_err(|_| unavailable())?
                    .len()
                > MAX_BRANCH_PAGE_BYTES
            {
                more = true;
                break;
            }
            row_bytes += bytes;
            rows.push(row);
        }
        if more && rows.is_empty() {
            return Err(unavailable());
        }
        let next_cursor = if more {
            rows.last().map(|r| encode(&r.ref_name))
        } else {
            None
        };
        let page = RepoBranchPage {
            branches: rows,
            next_cursor,
        };
        if serde_json::to_vec(&page).map_err(|_| unavailable())?.len() > MAX_BRANCH_PAGE_BYTES {
            return Err(unavailable());
        }
        Ok(page)
    }
}
#[cfg(test)]
#[path = "tests_branches.rs"]
mod tests;
