//! Fast in-process commit ancestry classification (`FastForward`, `LocalAhead`, `Diverged`, `Indeterminate`).
//!
//! Combines Git's `commit-graph` topological generation numbers (`gen(parent) < gen(child)`) with
//! a two-color DAG merge-base traversal:
//! - Never cuts off search by commit timestamp (`parent.time > child.time` clock skew is 100% safe).
//! - When finite `commit-graph` generation numbers exist, immediately prunes any ancestor branch
//!   whose topological generation drops at or below the target generation (`gen(p) <= gen(target)`
//!   with `p != target`), and resolves equal-generation distinct commits as `Diverged` in $O(1)$.
//! - Strictly separates walk-limit / object-read failures (`Indeterminate`) from verified
//!   non-ancestry (`Diverged`).

use std::collections::{BinaryHeap, HashMap, HashSet};

pub(crate) const MAX_ANCESTRY_WALK_COMMITS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchRelation {
    /// `local_oid` is an ancestor of `upstream_oid` (or equal).
    FastForward,
    /// `upstream_oid` is a strict ancestor of `local_oid`.
    LocalAhead,
    /// Verified non-ancestor in both directions (`local_oid` and `upstream_oid` diverged).
    Diverged,
    /// Unable to verify ancestry (walk limit reached before target, or commit object read error).
    Indeterminate(String),
}

#[derive(Clone)]
struct CommitMeta {
    generation: u32,
    commit_time: i64,
    parents: Vec<gix::ObjectId>,
}

impl CommitMeta {
    #[inline]
    fn has_exact_generation(&self) -> bool {
        self.generation > 0 && self.generation < gix::commitgraph::GENERATION_NUMBER_MAX
    }
}

/// Classifies the ancestry relationship between `local_oid` and `upstream_oid`.
pub(crate) fn classify_branch_relation(
    repo: &gix::Repository,
    local_oid: gix::ObjectId,
    upstream_oid: gix::ObjectId,
) -> BranchRelation {
    if local_oid == upstream_oid {
        return BranchRelation::FastForward;
    }
    const FROM_LOCAL: u8 = 0b01;
    const FROM_UPSTREAM: u8 = 0b10;
    const BOTH: u8 = FROM_LOCAL | FROM_UPSTREAM;

    let commit_graph = repo.commit_graph_if_enabled().ok().flatten();
    let mut meta_cache: HashMap<gix::ObjectId, CommitMeta> = HashMap::new();

    let read_meta = |oid: gix::ObjectId,
                     cache: &mut HashMap<gix::ObjectId, CommitMeta>|
     -> Result<CommitMeta, String> {
        if let Some(cached) = cache.get(&oid) {
            return Ok(cached.clone());
        }
        if let Some(cg) = commit_graph.as_ref() {
            if let Some(cg_commit) = cg.commit_by_id(oid) {
                let mut parents = Vec::new();
                let mut parent_ok = true;
                for p_pos_res in cg_commit.iter_parents() {
                    match p_pos_res {
                        Ok(p_pos) => parents.push(cg.id_at(p_pos).to_owned()),
                        Err(_) => {
                            parent_ok = false;
                            break;
                        }
                    }
                }
                if parent_ok {
                    let meta = CommitMeta {
                        generation: cg_commit.generation(),
                        commit_time: cg_commit.committer_timestamp() as i64,
                        parents,
                    };
                    cache.insert(oid, meta.clone());
                    return Ok(meta);
                }
            }
        }
        let commit = repo.find_commit(oid).map_err(|e| {
            format!("unable to read commit object {oid} during ancestry check: {e}")
        })?;
        let secs = commit.time().map(|t| t.seconds).unwrap_or(0);
        let parents: Vec<gix::ObjectId> = commit.parent_ids().map(|id| id.detach()).collect();
        let meta = CommitMeta {
            generation: gix::commitgraph::GENERATION_NUMBER_INFINITY,
            commit_time: secs,
            parents,
        };
        cache.insert(oid, meta.clone());
        Ok(meta)
    };

    let local_meta = match read_meta(local_oid, &mut meta_cache) {
        Ok(m) => m,
        Err(reason) => return BranchRelation::Indeterminate(reason),
    };
    let upstream_meta = match read_meta(upstream_oid, &mut meta_cache) {
        Ok(m) => m,
        Err(reason) => return BranchRelation::Indeterminate(reason),
    };

    // O(1) topological generation check when both commits have exact commit-graph generations:
    // Every strict ancestor A of B satisfies gen(A) < gen(B).
    let both_exact_gen = local_meta.has_exact_generation() && upstream_meta.has_exact_generation();
    if both_exact_gen && local_meta.generation == upstream_meta.generation {
        return BranchRelation::Diverged;
    }

    let mut flags: HashMap<gix::ObjectId, u8> = HashMap::new();
    let mut expanded_mask: HashMap<gix::ObjectId, u8> = HashMap::new();
    let mut in_heap_single: HashSet<gix::ObjectId> = HashSet::new();
    let mut heap: BinaryHeap<(u32, i64, gix::ObjectId)> = BinaryHeap::new();

    // Seed only the direction(s) that are topologically possible.
    let can_be_fast_forward = !both_exact_gen || local_meta.generation < upstream_meta.generation;
    let can_be_local_ahead = !both_exact_gen || upstream_meta.generation < local_meta.generation;

    if can_be_local_ahead {
        flags.insert(local_oid, FROM_LOCAL);
        in_heap_single.insert(local_oid);
        heap.push((local_meta.generation, local_meta.commit_time, local_oid));
    }
    if can_be_fast_forward {
        flags.insert(upstream_oid, FROM_UPSTREAM);
        in_heap_single.insert(upstream_oid);
        heap.push((
            upstream_meta.generation,
            upstream_meta.commit_time,
            upstream_oid,
        ));
    }
    // If both are seeded (no exact generation numbers), we run the 2-color merge-base walk.
    // If only one direction is seeded (exact generation numbers exist), we also prune any parent
    // whose generation drops <= target generation.

    let mut steps = 0usize;
    while let Some((_, _, oid)) = heap.pop() {
        let color = flags.get(&oid).copied().unwrap_or(0);
        if color != BOTH {
            in_heap_single.remove(&oid);
        }
        let prev_expanded = expanded_mask.get(&oid).copied().unwrap_or(0);
        if (color & !prev_expanded) == 0 {
            if in_heap_single.is_empty() {
                return BranchRelation::Diverged;
            }
            continue;
        }
        expanded_mask.insert(oid, prev_expanded | color);

        steps += 1;
        if steps > MAX_ANCESTRY_WALK_COMMITS {
            return BranchRelation::Indeterminate(format!(
                "ancestry walk exceeded limit ({MAX_ANCESTRY_WALK_COMMITS} commits); unable to verify fast-forward"
            ));
        }

        let meta = match read_meta(oid, &mut meta_cache) {
            Ok(m) => m,
            Err(reason) => return BranchRelation::Indeterminate(reason),
        };
        for p_oid in meta.parents {
            if p_oid == local_oid && (color & FROM_UPSTREAM) != 0 {
                return BranchRelation::FastForward;
            }
            if p_oid == upstream_oid && (color & FROM_LOCAL) != 0 {
                return BranchRelation::LocalAhead;
            }
            let p_meta = match read_meta(p_oid, &mut meta_cache) {
                Ok(m) => m,
                Err(reason) => return BranchRelation::Indeterminate(reason),
            };

            // Safe topological generation pruning:
            // - A purely FROM_UPSTREAM walk looking for `local_oid` cannot reach `local_oid`
            //   through `p_oid` if both have exact generations and `gen(p_oid) <= gen(local_oid)`.
            // - A purely FROM_LOCAL walk looking for `upstream_oid` cannot reach `upstream_oid`
            //   through `p_oid` if both have exact generations and `gen(p_oid) <= gen(upstream_oid)`.
            let mut propagate_color = color;
            if (propagate_color & FROM_UPSTREAM) != 0
                && !can_be_local_ahead
                && p_meta.has_exact_generation()
                && local_meta.has_exact_generation()
                && p_meta.generation <= local_meta.generation
            {
                propagate_color &= !FROM_UPSTREAM;
            }
            if (propagate_color & FROM_LOCAL) != 0
                && !can_be_fast_forward
                && p_meta.has_exact_generation()
                && upstream_meta.has_exact_generation()
                && p_meta.generation <= upstream_meta.generation
            {
                propagate_color &= !FROM_LOCAL;
            }
            if propagate_color == 0 {
                continue;
            }

            let old_p = flags.get(&p_oid).copied().unwrap_or(0);
            let new_p = old_p | propagate_color;
            if new_p != old_p {
                flags.insert(p_oid, new_p);
                if new_p == BOTH {
                    in_heap_single.remove(&p_oid);
                } else {
                    in_heap_single.insert(p_oid);
                }
                heap.push((p_meta.generation, p_meta.commit_time, p_oid));
            }
        }
        if in_heap_single.is_empty() {
            return BranchRelation::Diverged;
        }
    }
    BranchRelation::Diverged
}

pub(crate) fn is_agent_branch_short(short_branch: &str) -> bool {
    let lower = short_branch.trim_matches('/').to_ascii_lowercase();
    let prefix = crate::AGENT_BRANCH_PREFIX
        .trim_matches('/')
        .to_ascii_lowercase();
    lower == prefix
        || lower
            .strip_prefix(&prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(crate) fn list_checked_out_branches(
    repo_path: &std::path::Path,
    has_main_workdir: bool,
) -> Result<(Option<String>, HashSet<String>), crate::GitEngineError> {
    use gix::bstr::ByteSlice;
    let stdout = crate::run_git(repo_path, ["worktree", "list", "--porcelain", "-z"])?;
    let mut main_checked_out: Option<String> = None;
    let mut linked_checked_out: HashSet<String> = HashSet::new();
    let mut is_first_worktree = true;

    for record in stdout.split(|b| *b == 0) {
        let field = record.to_str_lossy();
        if let Some(short) = field.strip_prefix("branch refs/heads/") {
            if is_first_worktree {
                if has_main_workdir {
                    main_checked_out = Some(short.to_owned());
                }
            } else {
                linked_checked_out.insert(short.to_owned());
            }
        } else if field.is_empty() {
            is_first_worktree = false;
        }
    }
    Ok((main_checked_out, linked_checked_out))
}
