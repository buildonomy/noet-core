//! # ProtoIndex
//!
//! Pre-built filesystem index of every network directory in the repo, derived from a single
//! `WalkDir` pass at compiler startup.
//!
//! ## Motivation
//!
//! `NetworkCodec::proto` used to call `net_dir_children` (a `WalkDir` subtree scan) every time it
//! is asked to produce a proto for a network directory. In `initialize_stack`, this fires once per
//! ancestor directory per parsed document — O(networks × files) scans total.
//!
//! `ProtoIndex` replaces that pattern:
//!
//! 1. **Build once** (`ProtoIndex::build`) — one `WalkDir` from `repo_root` partitions every
//!    reachable file into its owning network directory via `net_dir_partition`.  The result is
//!    identical to running `net_dir_children` separately for each network, but costs O(files)
//!    instead of O(files × depth).
//!
//! 2. **Read cheaply** — `sort_key_for` and `proto_for` are pure read-only lookups after
//!    `build` returns.  No further filesystem access occurs during parsing.
//!
//! 3. **Share freely** — `ProtoIndex` wraps its inner map in `Arc<RwLock<...>>` and derives
//!    `Clone`.  Cloning produces a new handle to the same map (zero copy), matching the
//!    `BeliefSource + Clone` pattern used by `global_bb`.  Each parallel task in the Issue 57
//!    epoch architecture receives a cheap clone.
//!
//! ## What ProtoIndex is NOT
//!
//! - Not a `BeliefBase` or `BeliefGraph` — holds raw `Vec<PathBuf>` child lists, not
//!   resolved belief state.
//! - Not a `PathMap` — `PathMap` holds `BID → ordered position`; `ProtoIndex` holds
//!   `PathBuf → Vec<PathBuf>` (pre-belief-resolution filesystem structure).
//! - Not a holder of all relation types — `proto_for` only populates `upstream` with
//!   `WeightKind::Section` child-path relations (via `DocCodec::prepare_proto_relations`).
//!   Schema-derived and markdown-link edges are populated later by `MdCodec::parse`
//!   and `traverse_schema`.

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};
use walkdir::{DirEntry, Error as WalkDirError, WalkDir};

use toml_edit::value;

use crate::{
    codec::{
        is_network_index_file,
        network::{detect_network_file, NETWORK_NAME},
        IRNode, CODECS, WALK_CODECS,
    },
    error::BuildonomyError,
    paths::{os_path_to_string, string_to_os_path, AnchorPath},
    properties::BeliefKind,
};

#[cfg(feature = "git-tracking")]
use crate::codec::git::GitCache;

/// Filesystem-level index of every network directory in the repo.
///
/// Maps each absolute network directory path → its ordered list of direct children
/// (files with registered codec extensions, plus subnet directories).  The ordering
/// is lexicographic: subnet directories and plain files interleave alphabetically.
///
/// # Thread Safety
///
/// `Clone` on `ProtoIndex` clones the `Arc` handle only — the underlying map is shared.
/// After `build()` the map is read-only; the `RwLock` guards the initial population only.
/// Concurrent reads during parsing take the read lock, which is uncontended.
#[derive(Clone, Debug)]
pub struct ProtoIndex {
    /// `PathBuf` = absolute network directory
    /// `Vec<PathBuf>` = ordered direct children produced by the repo-wide scan
    inner: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>,
    /// Generic codec metadata cache.  Keyed by canonical network directory path,
    /// namespaced by a string key (e.g. `"git"`, `"cmake"`).  Each entry is a
    /// `serde_json::Value` that the producing codec serializes and the consuming
    /// codec deserializes into its own typed struct.
    ///
    /// Populated during `build()` (e.g. git metadata) or during Phase 1 network
    /// parsing via `set_meta()`.  Read-only after population; the `RwLock` guards
    /// concurrent access during parallel epoch tasks.
    codec_meta: Arc<RwLock<HashMap<PathBuf, HashMap<String, serde_json::Value>>>>,
}

/// Returns the direct children of a network directory: subnet directories (those containing
/// an `index.md`) and files with registered codec extensions, excluding files owned by
/// nested subnets.
///
/// Sort order is lexicographic: subnet directories and plain files interleave alphabetically
/// within each group.  Files under non-subnet subdirectories (plain dirs with no `index.md`)
/// are treated as peers of the plain files at the nearest subnet-ancestor level.
///
/// This is the single authoritative implementation.  `network::net_dir_children` delegates here.
pub(crate) fn net_dir_children<P: AsRef<Path>>(path: P) -> Vec<PathBuf> {
    // Normalize through the round-trip so that on Windows a \\?\-prefixed path
    // (from canonicalize()) is reduced to the plain C:\... form that WalkDir yields.
    // Without this, the equality checks inside net_dir_partition fire false because
    // the root passed in is \\?\C:\... while WalkDir entries are C:\....
    let call_root = string_to_os_path(&os_path_to_string(path.as_ref()));
    let by_group = net_dir_partition(&call_root);
    let mut result = Vec::with_capacity(by_group.values().map(|v| v.len()).sum());
    emit_group(&call_root, &by_group, &mut result);
    result
}

/// Partition the subtree rooted at `path` into a map of
/// `network_dir → ordered_direct_children`, in a single `WalkDir` pass.
///
/// Each key is a network directory (a directory containing `index.md`), including `path`
/// Each value is the list of direct children in lexicographic order — exactly
/// the list that `net_dir_children` would return for that key individually.
///
/// This allows `ProtoIndex::build` to build the complete index in O(files) rather than
/// O(files × depth) by avoiding one `WalkDir` call per network directory.
/// Log a `walkdir` error, distinguishing symlink cycles from plain I/O errors.
/// Called from both `net_dir_partition` and `discover_network_dirs`.
fn warn_walk_error(context: &str, e: &WalkDirError) {
    if e.loop_ancestor().is_some() {
        tracing::warn!(
            "{context}: symlink cycle detected at {:?}, skipping",
            e.path()
        );
    } else {
        tracing::warn!("{context}: I/O error during walk: {e}");
    }
}

pub(crate) fn net_dir_partition(path: &Path) -> BTreeMap<PathBuf, Vec<PathBuf>> {
    // Normalize through the round-trip so that on Windows a \\?\-prefixed path
    // (from canonicalize()) is reduced to the plain C:\... form that WalkDir yields.
    // All equality and prefix comparisons below use `path` as the reference; if it
    // carries the \\?\ extended-length prefix but WalkDir entries do not, every
    // `p.eq(path)` / `e.path() == path` check silently misfires.
    let path_buf = string_to_os_path(&os_path_to_string(path));
    let path = path_buf.as_path();

    fn is_hidden(entry: &DirEntry) -> bool {
        entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with('.'))
            .unwrap_or(false)
    }

    // ── Pass 1: discover all subnet directories ───────────────────────────────
    //
    // WalkDir does not guarantee that network files are yielded before sibling
    // files within the same directory — the order depends on the underlying
    // `readdir(2)` call, which is filesystem- and OS-dependent.  A single-pass
    // approach that populates `subnets` lazily as network files are encountered
    // therefore misclassifies sibling files that are yielded *before* their
    // directory's network file: the `subnets.iter().any(|s| p.starts_with(s))`
    // guard fires false, so those files are included in the root group instead
    // of being excluded (and later assigned to their correct subnet group).
    //
    // Pre-scanning for all subnet dirs in one pass makes Pass 2 order-independent.
    //
    // A file is a network file if WALK_CODECS.is_network_file(filename) returns
    // true — this checks NETWORK_NAME ("index.md") first, then any filenames
    // declared by registered WalkCodecs via network_filenames(). For non-
    // NETWORK_NAME candidates, this is a tentative classification; false
    // positives are culled later in ProtoIndex::build via DocCodec::proto().
    let subnet_dirs: std::collections::BTreeSet<PathBuf> = WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) || e.path() == path)
        .filter_map(|e| match e {
            Ok(e) => Some(e.into_path()),
            Err(ref err) => {
                warn_walk_error("net_dir_partition pass 1", err);
                None
            }
        })
        .filter_map(|mut p| {
            if p.is_file() {
                // Skip file-level symlinks (same rationale as Pass 2).
                if p.is_symlink() {
                    return None;
                }
                let p_str = os_path_to_string(&p);
                let p_ap = AnchorPath::new(&p_str);
                if WALK_CODECS.is_network_file(p_ap.filename()) {
                    p.pop(); // file → its containing directory
                    if !p.eq(path) {
                        return Some(p); // subnet dir
                    }
                }
            }
            None
        })
        .collect();

    // ── Pass 2: collect and assign files to their owning subnet ──────────────
    //
    // Now that `subnet_dirs` is complete, classify every file correctly regardless
    // of the order WalkDir yields entries within a directory.
    //
    // Files yielded by Pass 2 filter_map:
    //   • subnet network file → represented as the subnet directory path (is_dir());
    //     `group_of` routes it to its parent subnet (or root)
    //   • codec files owned by any subnet OR root → all included; `group_of` routes
    //     each file to its deepest owning subnet key in the by_group loop below.
    //     No per-file filtering is needed here because `group_of` uses the complete
    //     `subnet_dirs` set from Pass 1 and is therefore order-independent.
    //   • root's own network file → excluded entirely
    //   • non-codec files → excluded entirely
    //   • file-level symlinks → excluded (parsed under their canonical location;
    //     symlinks should produce epistemic/pragmatic edges, not duplicate section trees)
    let files = WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) || e.path() == path)
        .filter_map(|e| match e {
            Ok(e) => Some(e.into_path()),
            Err(ref err) => {
                warn_walk_error("net_dir_partition pass 2", err);
                None
            }
        })
        .filter_map(|mut p| {
            if p.is_file() {
                // Skip file-level symlinks.  A symlinked file (e.g.
                // `component/design_links/spec.md` → `docs/spec.md`) would
                // otherwise be parsed under BOTH the symlink's network and
                // the canonical location's network, producing duplicate nodes
                // with different BIDs that cause cache_fetch misses on
                // reparse.  The canonical copy is always discovered at its
                // real path; cross-tree references should use epistemic or
                // pragmatic edges (e.g. `resolve_design_links`) instead.
                if p.is_symlink() {
                    return None;
                }
                let p_str = os_path_to_string(&p);
                let p_ap = AnchorPath::new(&p_str);
                if WALK_CODECS.is_network_file(p_ap.filename()) {
                    // Subnet network file — represent the subnet as its directory path.
                    p.pop();
                    if !p.eq(path) {
                        return Some(p); // subnet dir entry (is_dir() in by_group loop)
                    } else {
                        return None; // root's own network file — exclude
                    }
                }
                // Use new_file: p.is_file() is confirmed, prevents extensionless files
                // (Gemfile, Makefile, …) from matching the (None, None) codec wildcard.
                let p_ap_file = AnchorPath::new_file(&p_str);
                if CODECS.get(&p_ap_file).is_some() || WALK_CODECS.should_track(&p) {
                    Some(p)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<PathBuf>>();

    // Partition into groups and sort lexicographically.
    //
    // Only subnet directories (is_dir() entries) form group boundaries.  Files under
    // non-subnet subdirectories belong to the nearest subnet-ancestor's group.
    //
    // Algorithm:
    //   1. `subnet_dirs` is the complete set of subnet dir paths from Pass 1.
    //   2. For each entry compute its "owning group": deepest subnet-dir ancestor,
    //      or `path` (the call root) if none.
    //   3. Sort each group lexicographically — subnets and files interleave alphabetically.
    //   4. emit_group recurses into each subnet at its natural sorted position.
    let group_of = |p: &PathBuf| -> PathBuf {
        // For a regular file: deepest subnet ancestor, or root.
        // For a subnet dir: deepest subnet ancestor that is a *strict* prefix of p
        // (i.e. the parent subnet), or root.  Using `starts_with` alone would match
        // the dir against itself; we exclude self-matches by requiring strict prefix.
        subnet_dirs
            .iter()
            .filter(|s| {
                // strict prefix: s must be an ancestor of p, not p itself
                p.starts_with(s.as_path()) && *s != p
            })
            .max_by_key(|s| s.components().count())
            .cloned()
            .unwrap_or_else(|| path.to_path_buf())
    };

    let mut by_group: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    // Ensure the root key is always present, even for a network with no children.
    by_group.entry(path.to_path_buf()).or_default();
    // Ensure every subnet has its own key, even if it has no files.
    for subnet in &subnet_dirs {
        by_group.entry(subnet.clone()).or_default();
    }
    for p in files {
        by_group.entry(group_of(&p)).or_default().push(p);
    }

    // Sort each group lexicographically — subnets and files interleave alphabetically.
    for entries in by_group.values_mut() {
        entries.sort();
    }

    by_group
}

fn emit_group(
    group_root: &PathBuf,
    by_group: &BTreeMap<PathBuf, Vec<PathBuf>>,
    result: &mut Vec<PathBuf>,
) {
    let Some(entries) = by_group.get(group_root) else {
        return;
    };
    // Entries are already sorted lexicographically by net_dir_partition.
    // Emit each entry; when it's a subnet dir, recurse immediately after so its
    // contents follow it in DFS order before the next sibling.
    for entry in entries.clone() {
        result.push(entry.clone());
        if entry.is_dir() {
            emit_group(&entry, by_group, result);
        }
    }
}

impl ProtoIndex {
    /// Create an empty `ProtoIndex` (useful for testing or deferred population).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            codec_meta: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build by scanning the entire repo tree once from `repo_root`.
    ///
    /// Produces the same per-directory child lists that calling `net_dir_children` separately
    /// on each network directory would produce, but in a single `WalkDir` pass.
    ///
    /// The scan partitions every discovered file into the child list of its *owning network
    /// directory* — the deepest ancestor directory that contains an `index.md` file.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `repo_root` is not a valid network root (no `index.md` found).
    pub fn build(repo_root: &Path, git_tracking: bool) -> Result<Self, BuildonomyError> {
        // Verify repo_root is actually a network root.
        if detect_network_file(repo_root).is_none() {
            return Err(BuildonomyError::Codec(format!(
                "ProtoIndex::build: repo_root {repo_root:?} contains no {NETWORK_NAME} file"
            )));
        }

        // Single-pass construction via net_dir_partition: one WalkDir from repo_root
        // partitions every reachable file into its owning network directory in O(files).
        // Each key in the partition is a network dir; each value is the ordered child list
        // identical to what net_dir_children would return for that dir individually.
        //
        // All map keys and child paths are canonicalized so that lookup keys derived from
        // canonicalized paths (e.g. from Path::canonicalize() in the caller) always match.
        // Normalize repo_root before passing to net_dir_partition so that on Windows a
        // \\?\-prefixed canonical path is reduced to the plain C:\... form that WalkDir
        // yields.  net_dir_partition also normalizes internally, but doing it here keeps
        // the partition keys consistent with the canonicalized+round-tripped values we
        // store in the map below.
        let repo_root_buf = string_to_os_path(&os_path_to_string(repo_root));
        let mut partition = net_dir_partition(&repo_root_buf);

        // ── Cull false-positive subnet candidates ────────────────────────────
        //
        // net_dir_partition tentatively treats any directory containing a file
        // matching WALK_CODECS.is_network_file() as a subnet. For non-NETWORK_NAME
        // files this is a superset — some may not be real network roots.
        //
        // For each candidate subnet whose network file is not NETWORK_NAME, call
        // DocCodec::proto(). If proto() returns None or a node without
        // BeliefKind::Network, demote: remove the subnet key and merge its children
        // back into the parent group.
        let candidate_dirs: Vec<PathBuf> = partition
            .keys()
            .filter(|dir| {
                // Skip the root — it's always valid (verified above).
                if dir.as_path() == repo_root_buf.as_path() {
                    return false;
                }
                // Skip NETWORK_NAME-based subnets — they are always valid.
                !dir.join(NETWORK_NAME).exists()
            })
            .cloned()
            .collect();

        for candidate_dir in candidate_dirs {
            // Find the network file in this directory.
            let network_file = detect_network_file(&candidate_dir);
            let is_valid_network = network_file.as_ref().is_some_and(|nf| {
                CODECS.path_get(nf).is_some_and(|factory| {
                    factory()
                        .proto(nf)
                        .ok()
                        .flatten()
                        .is_some_and(|proto| proto.kind.contains(BeliefKind::Network))
                })
            });

            if !is_valid_network {
                tracing::debug!(
                    "[ProtoIndex::build] Demoting false-positive subnet {:?} — \
                     proto() did not return BeliefKind::Network",
                    candidate_dir,
                );
                // Remove the subnet key and find its parent group.
                if let Some(children) = partition.remove(&candidate_dir) {
                    // Find the parent: deepest partition key that is a strict ancestor.
                    let parent_key = partition
                        .keys()
                        .filter(|k| candidate_dir.starts_with(k.as_path()) && *k != &candidate_dir)
                        .max_by_key(|k| k.components().count())
                        .cloned()
                        .unwrap_or_else(|| repo_root_buf.clone());
                    partition.entry(parent_key).or_default().extend(children);
                }
            }
        }

        // Re-sort groups that may have received merged children from demoted subnets.
        for entries in partition.values_mut() {
            entries.sort();
        }

        let map: HashMap<PathBuf, Vec<PathBuf>> = partition
            .into_iter()
            .map(|(net_dir, children)| {
                let key = crate::paths::canonicalize_path(&net_dir).unwrap_or(net_dir);
                let children = children
                    .into_iter()
                    .map(|p| crate::paths::canonicalize_path(&p).unwrap_or(p))
                    .collect();
                (key, children)
            })
            .collect();

        let codec_meta: HashMap<PathBuf, HashMap<String, serde_json::Value>> = HashMap::new();

        #[cfg(feature = "git-tracking")]
        let codec_meta = if git_tracking {
            let network_dirs: Vec<PathBuf> = map.keys().cloned().collect();
            let git_cache = GitCache::populate(&network_dirs);
            let mut codec_meta = codec_meta;
            for (dir, status) in git_cache.iter_networks() {
                match serde_json::to_value(status) {
                    Ok(val) => {
                        codec_meta
                            .entry(dir.clone())
                            .or_default()
                            .insert("git".to_string(), val);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ProtoIndex::build] Failed to serialize git status for {}: {}",
                            dir.display(),
                            e,
                        );
                    }
                }
            }
            codec_meta
        } else {
            codec_meta
        };
        // Suppress unused-variable warning when feature is disabled.
        let _ = git_tracking;

        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
            codec_meta: Arc::new(RwLock::new(codec_meta)),
        })
    }

    /// Discover all network directories under `root` (directories containing `index.md`),
    /// including `root` itself.  Returns them in lexicographic order (shallowest first).
    ///
    /// All returned paths are canonicalized so they match the canonicalized keys used by
    /// `children_of` / `sort_key_for` callers.
    ///
    /// Note: `build()` does not call this — it uses `net_dir_partition` for a single-pass
    /// O(files) construction.  This function is retained for tests and utilities that need
    /// the list of network directories independently of a built index.
    #[allow(dead_code)]
    pub(crate) fn discover_network_dirs(root: &Path) -> Vec<PathBuf> {
        // Canonicalize root so we can use it as the "allow root even if hidden" reference,
        // mirroring net_dir_children's `!is_hidden(e) || e.path() == path.as_ref()` guard.
        let canonical_root =
            crate::paths::canonicalize_path(root).unwrap_or_else(|_| root.to_path_buf());
        let mut dirs: Vec<PathBuf> = WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| {
                // Allow the root entry unconditionally (it may live in a hidden temp dir).
                // Skip all other hidden entries — same rule as net_dir_children.
                let entry_canonical = {
                    crate::paths::canonicalize_path(e.path())
                        .unwrap_or_else(|_| e.path().to_path_buf())
                };
                entry_canonical == canonical_root
                    || !e
                        .file_name()
                        .to_str()
                        .map(|s| s.starts_with('.'))
                        .unwrap_or(false)
            })
            .filter_map(|e| match e {
                Ok(e) => Some(e),
                Err(ref err) => {
                    warn_walk_error("discover_network_dirs", err);
                    None
                }
            })
            .filter_map(|e| {
                let p = e.into_path();
                if p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| WALK_CODECS.is_network_file(n))
                        .unwrap_or(false)
                {
                    // Return the canonicalized parent directory, not the network file itself.
                    p.parent().map(|d| {
                        crate::paths::canonicalize_path(d).unwrap_or_else(|_| d.to_path_buf())
                    })
                } else {
                    None
                }
            })
            .collect();

        dirs.sort_by(|a, b| a.components().cmp(b.components()));
        dirs.dedup();
        dirs
    }

    /// Returns all known network directories, sorted shallowest-first (by component count,
    /// then lexicographically).
    ///
    /// This is the same order as `discover_network_dirs` but
    /// reads from the already-built index rather than performing a new filesystem scan.
    ///
    /// Use this in `parse_all` to iterate epoch-0 batches in the correct network order
    /// without redundant `WalkDir` calls.
    /// Returns all known network directories sorted **shallowest-first**: primary
    /// key is component count (ascending), secondary key is lexicographic order
    /// within the same depth.
    ///
    /// This ordering guarantee is **load-bearing**: callers that group consecutive
    /// entries by component count (e.g. `parse_sequential` and `parse_all` phase 1)
    /// rely on all dirs at depth D being contiguous in the returned slice.  If the
    /// sort order ever changes, those grouping loops must be updated accordingly.
    pub fn network_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.inner.read().keys().cloned().collect();
        dirs.sort_by(|a, b| {
            a.components()
                .count()
                .cmp(&b.components().count())
                .then_with(|| a.components().cmp(b.components()))
        });
        // Verify the load-bearing invariant: component counts must be non-decreasing
        // so that depth-grouping by consecutive runs is correct.
        debug_assert!(
            dirs.windows(2)
                .all(|w| w[0].components().count() <= w[1].components().count()),
            "network_dirs: sort invariant violated — component counts not non-decreasing"
        );
        dirs
    }

    /// Returns network directories grouped by **subnet-tree depth**: the number of
    /// subnet-to-subnet hops from the repo root, *not* the OS path-component count.
    ///
    /// Group `k` contains every network dir whose parent network is in group `k-1`;
    /// group `0` holds the repo root (plus any dir with no indexed parent).
    ///
    /// # Why this differs from path-component depth
    ///
    /// `net_dir_partition` flattens plain (non-network) intervening directories: a
    /// subnet at `A/docs/parts/B/index.md` is a *direct* child of `A` in
    /// [`children_of`], exactly like a subnet at `A/B2/index.md`.  Grouping by
    /// `components().count()` would place those two true siblings in different
    /// groups — three apart, in that example — purely because one's path string is
    /// longer.  Since `parse_all` runs one epoch per group and drains between them,
    /// that spread serializes work that has no dependency relationship, and delays
    /// the deeper-pathed sibling past the round where it could have run.
    ///
    /// Grouping by tree depth reconverges true siblings into the same epoch and
    /// reduces the number of epochs to the real subnet-chain length.
    ///
    /// # Invariant relied on by callers
    ///
    /// Every dir in group `k` has its parent network in group `k-1`.  Callers that
    /// commit each group before starting the next (`parse_all` phase 1) therefore
    /// know a dir's ancestors are fully committed before it runs.  The converse is
    /// what makes merging groups unsafe: *every* member of group `k+1` has a parent
    /// in group `k`, so no member can be pulled forward.
    ///
    /// Within a group, dirs keep [`network_dirs`]'s ordering (component count, then
    /// lexicographic) so batch composition is deterministic across runs.
    ///
    /// [`children_of`]: ProtoIndex::children_of
    /// [`network_dirs`]: ProtoIndex::network_dirs
    pub fn network_dirs_by_tree_depth(&self) -> Vec<Vec<PathBuf>> {
        let dirs = self.network_dirs();
        if dirs.is_empty() {
            return Vec::new();
        }

        // Map each subnet dir to its parent network, read straight from the
        // in-memory child lists.  A child entry that is a directory is a subnet;
        // plain files are leaf documents and are not scheduled here.
        //
        // This deliberately does not use `owning_net_dir_for`, which walks the
        // filesystem on every call.
        let parent_of: HashMap<PathBuf, PathBuf> = {
            let inner = self.inner.read();
            inner
                .iter()
                .flat_map(|(net_dir, children)| {
                    children
                        .iter()
                        .filter(|child| child.is_dir())
                        .map(move |child| (child.clone(), net_dir.clone()))
                })
                .collect()
        };

        // Single left-to-right pass.  `dirs` is sorted by ascending component count,
        // and a subnet's path strictly contains its parent's, so the parent always
        // has a strictly smaller component count and has therefore already been
        // assigned a depth by the time we reach the child.
        let mut depth_of: HashMap<PathBuf, usize> = HashMap::with_capacity(dirs.len());
        let mut groups: Vec<Vec<PathBuf>> = Vec::new();
        for dir in dirs {
            let depth = parent_of
                .get(&dir)
                .and_then(|parent| depth_of.get(parent))
                .map(|parent_depth| parent_depth + 1)
                .unwrap_or(0);
            debug_assert!(
                depth_of.get(&dir).is_none_or(|existing| *existing == depth),
                "network_dirs_by_tree_depth: {dir:?} assigned two different depths"
            );
            depth_of.insert(dir.clone(), depth);
            if groups.len() <= depth {
                groups.resize_with(depth + 1, Vec::new);
            }
            groups[depth].push(dir);
        }
        groups
    }

    /// Returns the lexically-ordered direct children of `dir`, or `None` if `dir` is not a
    /// known network directory.
    ///
    /// This is a read-only lookup after `build()` completes.
    pub fn children_of(&self, dir: &Path) -> Option<Vec<PathBuf>> {
        // Canonicalize the lookup key so callers using raw or canonicalized paths both hit.
        let canonical = crate::paths::canonicalize_path(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.inner.read().get(&canonical).cloned()
    }

    /// Returns all document paths in depth-first, network-sorted order.
    ///
    /// Within each network directory, children are returned in the same lexicographic
    /// order produced by `net_dir_children` (shallow before deep, alphabetical within each
    /// depth).  Subnet directories appear at their natural alphabetical position among
    /// siblings — `dfs_ordered` distinguishes them by `child.is_dir()` and recurses into
    /// them, so their contents immediately follow the subnet dir entry in the output.
    ///
    /// The flat list returned here is used by `parse_sequential` for its initial pass.
    /// `parse_all` uses `network_dirs()` + `children_of()` directly for epoch batching.
    /// Returns a map from each known path to its global DFS position in `ordered_paths()`.
    ///
    /// Use this as a tiebreaker when sorting the remainder queue within a single `processed`
    /// count bucket, so that reparse order is deterministic regardless of the order paths
    /// were enqueued by `process_unresolved_reference`.
    pub fn ordered_path_index(&self) -> std::collections::HashMap<PathBuf, usize> {
        self.ordered_paths()
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p, i))
            .collect()
    }

    pub fn ordered_paths(&self) -> Vec<PathBuf> {
        // DFS from the repo root (first network_dir, shallowest).
        // network_dirs() returns all network dirs shallowest-first.
        // For each network dir, children_of() returns its direct children in net_dir_children order.
        // We emit: the network dir itself (represents index.md), then recurse into subnet children,
        // interleaved with plain file children in the order children_of() returns them.
        let dirs = self.network_dirs();
        let Some(root) = dirs.first().cloned() else {
            return Vec::new();
        };
        let mut result = Vec::new();
        self.dfs_ordered(&root, &mut result);
        result
    }

    fn dfs_ordered(&self, net_dir: &Path, result: &mut Vec<PathBuf>) {
        // Emit the network dir itself (parse_sequential will resolve to index.md)
        result.push(net_dir.to_path_buf());
        let Some(children) = self.children_of(net_dir) else {
            return;
        };
        for child in children {
            if child.is_dir() {
                // Subnet: recurse
                self.dfs_ordered(&child, result);
            } else {
                result.push(child);
            }
        }
    }

    /// Returns the absolute path of the network directory that owns `abs_path`.
    ///
    /// The owning network is the nearest ancestor directory that is a known network
    /// (i.e. present in the ProtoIndex) **and** whose `children_of` list contains
    /// `abs_path` (or, for a network index file, its parent directory) as a direct
    /// child entry.  This is a **membership** check — the file must appear in the
    /// parent's child list, not merely be somewhere beneath a known network.
    ///
    /// This differs from the walk-up in `try_initialize_stack_from_session_cache`,
    /// which uses an **existence** check (`children_of(dir).is_some()`) to find the
    /// nearest ancestor that IS a network at all.  The two checks agree for well-formed
    /// repos with no symlinked subnets, but can diverge in edge cases — each is correct
    /// for its own purpose:
    ///
    /// - `owning_net_dir_for` (membership): used to derive the submap path for the
    ///   pre-seed loop in `parse_epoch` and to drive `sort_key_for`, where the exact
    ///   PathMap child entry position is needed.
    /// - `children_of(dir).is_some()` (existence): used in
    ///   `try_initialize_stack_from_session_cache` to find the nearest network ancestor
    ///   whose PathMap can be queried for the entry document.
    ///
    /// Returns `None` if:
    /// - `abs_path` has no parent directory, or
    /// - no ancestor directory in the ProtoIndex contains `abs_path` in its child list.
    pub fn owning_net_dir_for(&self, abs_path: &Path) -> Option<PathBuf> {
        // For a network index file, the ProtoIndex child lists record the *directory*
        // path, not the index file itself.  Use the parent directory as the lookup
        // target so that `subnet1/index.md` matches the `subnet1/` entry in the root
        // network's child list.
        let lookup_path: std::borrow::Cow<Path> = if is_network_index_file(abs_path) {
            std::borrow::Cow::Owned(abs_path.parent()?.to_path_buf())
        } else {
            std::borrow::Cow::Borrowed(abs_path)
        };

        // Canonicalize once for all comparisons against canonicalized child entries.
        let canonical = crate::paths::canonicalize_path(lookup_path.as_ref())
            .unwrap_or_else(|_| lookup_path.to_path_buf());

        // Walk up the directory tree.  The first ancestor directory whose
        // children_of list contains `canonical` is the owning network directory.
        let mut dir = lookup_path.parent()?;
        loop {
            if let Some(children) = self.children_of(dir) {
                if children.iter().any(|child| child == &canonical) {
                    return Some(dir.to_path_buf());
                }
                // This dir is a known network but doesn't contain abs_path — keep walking up.
                // (Shouldn't happen in practice for well-formed repos, but be safe.)
            }
            dir = dir.parent()?;
        }
    }

    /// Returns the 0-based sort key for `abs_path` within its owning network directory.
    ///
    /// Delegates the directory walk to [`ProtoIndex::owning_net_dir_for`], then returns
    /// the position of `abs_path` (or its parent dir for network index files) within
    /// that network's `children_of` list.
    ///
    /// The owning network may not be the immediate parent directory.  For files in a
    /// non-network subdirectory (e.g. `net1_dir1/hsml.md` where `net1_dir1/` has no
    /// `index.md`), `net_dir_children` includes the file in the **ancestor** network's
    /// child list (flattened).
    ///
    /// Returns `None` if:
    /// - `abs_path` has no parent directory, or
    /// - no ancestor directory in the ProtoIndex contains `abs_path` in its child list.
    pub fn sort_key_for(&self, abs_path: &Path) -> Option<u16> {
        let lookup_path: std::borrow::Cow<Path> = if is_network_index_file(abs_path) {
            std::borrow::Cow::Owned(abs_path.parent()?.to_path_buf())
        } else {
            std::borrow::Cow::Borrowed(abs_path)
        };
        let canonical = crate::paths::canonicalize_path(lookup_path.as_ref())
            .unwrap_or_else(|_| lookup_path.to_path_buf());

        let net_dir = self.owning_net_dir_for(abs_path)?;
        let children = self.children_of(&net_dir)?;
        children
            .iter()
            .position(|child| child == &canonical)
            .map(|idx| idx as u16)
    }

    /// Build a complete network `IRNode` for `dir`.
    ///
    /// Reads frontmatter via `MdCodec::proto` (cheap file read, no `WalkDir`) and populates
    /// `upstream` with `WeightKind::Section` child-path relations from `self.children_of(dir)`.
    ///
    /// This is a drop-in replacement for `NetworkCodec::proto` in the `initialize_stack`
    /// ancestor push() loop.  It is correct because `NetworkCodec::proto` puts *only*
    /// `WeightKind::Section` entries (derived from `net_dir_children`) into `upstream`; all
    /// other relation types are populated later by `MdCodec::parse` and `traverse_schema`
    /// during Phase 1.
    ///
    /// Returns `Ok(None)` if:
    /// - `dir` has no `index.md` file, or
    /// - `dir` is not present in the `ProtoIndex` (not a known network directory).
    ///
    /// Returns `Err` if the `index.md` frontmatter cannot be parsed, or if the network node
    /// has no semantic ID (same invariant enforced by `NetworkCodec::proto`).
    ///
    /// Returns a tuple of `(IRNode, Option<serde_json::Value>)`.  The second element is
    /// the `"git"` namespace entry from `codec_meta` — `Some` when git tracking is
    /// enabled, `git_tracking` was `true` at `build` time, and the network directory
    /// resides inside a git repository.
    #[allow(clippy::type_complexity)]
    pub fn proto_for(
        &self,
        dir: &Path,
    ) -> Result<Option<(IRNode, Option<serde_json::Value>)>, BuildonomyError> {
        let Some(network_filepath) = detect_network_file(dir) else {
            return Ok(None);
        };
        let network_dir_raw = network_filepath
            .parent()
            .expect("detect_network_file returns a path.is_file() path; parent() must succeed");
        // Normalize through os_path_to_string/string_to_os_path so that on Windows the
        // \\?\ extended-length prefix is stripped, matching the form used by build() when
        // it stores child paths in the inner map.  Without this, strip_prefix inside
        // prepare_proto_relations fails because network_dir is still \\?\C:\... while
        // children are stored as C:/... after the round-trip.
        let network_dir_buf = string_to_os_path(&os_path_to_string(network_dir_raw));
        // Canonicalize so the path matches the canonicalized keys stored by ProtoIndex::build().
        // Without this, children_of() returns None on macOS (where /var/... symlinks to
        // /private/var/...) and the fallback net_dir_children() returns canonicalized children
        // that strip_prefix fails against the non-canonical network_dir.
        let network_dir_canonical =
            crate::paths::canonicalize_path(&network_dir_buf).unwrap_or(network_dir_buf);
        let network_dir = network_dir_canonical.as_path();

        // Read frontmatter via the registered codec for this file — honours any custom
        // network codec swapped in via CODECS rather than hardcoding MdCodec.
        let Some(codec_factory) = CODECS.path_get(network_filepath.as_ref()) else {
            return Ok(None);
        };
        let Some(mut proto) = codec_factory().proto(network_filepath.as_ref())? else {
            return Ok(None);
        };
        if proto.id().is_none() {
            return Err(BuildonomyError::Codec(format!(
                "ProtoIndex::proto_for: network node at {dir:?} has no semantic ID"
            )));
        }

        proto.path = os_path_to_string(network_dir);
        proto.kind.insert(BeliefKind::Network);
        // Ensure every Network node carries payload["codec"] — the filename that
        // round-trips through CODECS.get(AnchorPath::new(value)).  The upstream
        // codec_factory().proto() call may have already set this (e.g. NetworkCodec
        // always does); only fill it in when absent so custom codecs can override.
        if !proto.document.contains_key("codec") {
            let codec_filename = network_filepath
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(NETWORK_NAME);
            proto.document.insert("codec", value(codec_filename));
        }
        proto.heading = 1;

        // Populate upstream with Section child-path relations from the cached child list.
        // Falls back to net_dir_children for paths not in the index (e.g. out-of-repo callers).
        let children = match self.children_of(network_dir) {
            Some(c) => c,
            None => net_dir_children(network_dir)
                .into_iter()
                .map(|p| crate::paths::canonicalize_path(&p).unwrap_or(p))
                .collect(),
        };

        // Call prepare_proto_relations via the registered codec so custom network codec
        // implementations can express their own file-based child relations.
        codec_factory().prepare_proto_relations(&mut proto, network_dir, &children)?;

        // Look up cached git status from codec_meta (None when git tracking
        // is disabled or the directory is not inside any git repository).
        let git_status = self.get_meta(network_dir, "git");

        Ok(Some((proto, git_status)))
    }

    /// Store codec metadata for a network directory under a namespace key.
    ///
    /// The value is serialized by the producing codec via `serde_json::to_value()`.
    /// Consuming codecs read it back via [`Self::get_meta`] or [`Self::get_meta_as`].
    ///
    /// The path is canonicalized before insertion so lookups from different path
    /// representations (symlinks, `\\?\` prefixes on Windows) all hit the same entry.
    pub fn set_meta(&self, dir: &Path, namespace: &str, value: serde_json::Value) {
        let canonical = crate::paths::canonicalize_path(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.codec_meta
            .write()
            .entry(canonical)
            .or_default()
            .insert(namespace.to_string(), value);
    }

    /// Read raw codec metadata for a network directory under a namespace key.
    ///
    /// Returns `None` if no metadata was stored for this `(dir, namespace)` pair.
    pub fn get_meta(&self, dir: &Path, namespace: &str) -> Option<serde_json::Value> {
        let canonical = crate::paths::canonicalize_path(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.codec_meta
            .read()
            .get(&canonical)
            .and_then(|m| m.get(namespace))
            .cloned()
    }

    /// Iterate all `(dir, value)` pairs for a given namespace.
    ///
    /// Returns a `Vec` rather than an iterator to avoid holding the read lock
    /// across the caller's processing loop.  Values are cloned.
    pub fn iter_meta_as<T: DeserializeOwned>(&self, namespace: &str) -> Vec<(PathBuf, T)> {
        let guard = self.codec_meta.read();
        guard
            .iter()
            .filter_map(|(dir, namespaces)| {
                let val = namespaces.get(namespace)?;
                let typed: T = serde_json::from_value(val.clone()).ok()?;
                Some((dir.clone(), typed))
            })
            .collect()
    }

    /// Read and deserialize codec metadata into a typed struct.
    ///
    /// Convenience wrapper around [`Self::get_meta`] that calls
    /// `serde_json::from_value::<T>()`.  Returns `None` if no metadata exists or
    /// if deserialization fails (with a `tracing::warn` on failure).
    pub fn get_meta_as<T: DeserializeOwned>(&self, dir: &Path, namespace: &str) -> Option<T> {
        let val = self.get_meta(dir, namespace)?;
        match serde_json::from_value::<T>(val) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    "[ProtoIndex::get_meta_as] Failed to deserialize namespace \"{namespace}\" \
                     for {}: {e}",
                    dir.display(),
                );
                None
            }
        }
    }

    /// Walk parent directories of `path` looking for codec metadata under `namespace`.
    ///
    /// Returns the first `(directory, deserialized_value)` pair found by walking up
    /// from `path`'s parent directory, calling [`Self::get_meta_as`] at each level.
    /// Stops at the first hit.  Returns `None` if no ancestor has metadata for this
    /// namespace.
    ///
    /// This extracts the ancestor-walk pattern used by a downstream C++ codec and makes it
    /// reusable for any codec that needs to inherit config from an ancestor network
    /// (e.g. `alias-template` on a parent network applied to child documents).
    pub fn ancestor_meta_as<T: DeserializeOwned>(
        &self,
        path: &Path,
        namespace: &str,
    ) -> Option<(PathBuf, T)> {
        let mut candidate = path.parent().map(|p| p.to_path_buf());
        while let Some(dir) = candidate {
            let canonical = crate::paths::canonicalize_path(&dir).unwrap_or_else(|_| dir.clone());
            if let Some(value) = self.get_meta_as::<T>(&canonical, namespace) {
                return Some((canonical, value));
            }
            candidate = dir.parent().map(|p| p.to_path_buf());
        }
        None
    }
}

impl Default for ProtoIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::network::NetworkCodec;
    use crate::codec::{DocCodec, WalkCodec};
    use crate::nodekey::NodeKey;
    use std::fs;
    use tempfile::TempDir;

    /// Write a minimal index.md with the given id into `dir`.
    fn write_index(dir: &Path, id: &str) {
        let content = format!("---\nid = \"{id}\"\ntitle = \"{id}\"\n---\n");
        fs::write(dir.join(NETWORK_NAME), content).unwrap();
    }

    /// Build a test fixture with the following structure:
    ///
    /// ```text
    /// root/
    ///   index.md          (id = "root")
    ///   alpha.md
    ///   beta.md
    ///   subnet/
    ///     index.md        (id = "subnet")
    ///     gamma.md
    ///     delta.md
    ///   .hidden/
    ///     index.md        (id = "hidden-net")  -- should be excluded
    ///     epsilon.md
    /// ```
    fn build_fixture() -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        write_index(root, "root");
        fs::write(root.join("alpha.md"), "# Alpha\n").unwrap();
        fs::write(root.join("beta.md"), "# Beta\n").unwrap();

        let subnet = root.join("subnet");
        fs::create_dir_all(&subnet).unwrap();
        write_index(&subnet, "subnet");
        fs::write(subnet.join("gamma.md"), "# Gamma\n").unwrap();
        fs::write(subnet.join("delta.md"), "# Delta\n").unwrap();

        let hidden = root.join(".hidden");
        fs::create_dir_all(&hidden).unwrap();
        write_index(&hidden, "hidden-net");
        fs::write(hidden.join("epsilon.md"), "# Epsilon\n").unwrap();

        tmp
    }

    // -------------------------------------------------------------------------
    // build() / children_of()
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_discovers_correct_network_dirs() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        // Root and subnet should be in the index; .hidden should not.
        assert!(
            idx.children_of(&root).is_some(),
            "repo root should be indexed"
        );
        let subnet = root.join("subnet");
        assert!(
            idx.children_of(&subnet).is_some(),
            "subnet dir should be indexed"
        );
        let hidden = root.join(".hidden");
        assert!(
            idx.children_of(&hidden).is_none(),
            ".hidden dir should be excluded"
        );
    }

    /// Verify that ProtoIndex::build produces the same per-directory child list as
    /// net_dir_partition for each network directory.
    ///
    /// Note: `children_of` returns the *direct group* for each network dir (subnet dir
    /// entries + that network's own plain files), matching one key from `net_dir_partition`.
    /// This differs from `net_dir_children`, which returns the full recursive DFS output
    /// (all descendants across all nested subnets in a single flat list).
    #[test]
    fn test_build_matches_net_dir_children_per_directory() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        let mut partition = net_dir_partition(&root);
        // Canonicalize partition keys and values to match ProtoIndex's stored paths.
        let partition: BTreeMap<_, _> = partition
            .iter_mut()
            .map(|(net_dir, children)| {
                let key =
                    crate::paths::canonicalize_path(net_dir).unwrap_or_else(|_| net_dir.clone());
                let vals: Vec<PathBuf> = children
                    .iter()
                    .map(|p| crate::paths::canonicalize_path(p).unwrap_or_else(|_| p.clone()))
                    .collect();
                (key, vals)
            })
            .collect();

        let network_dirs = ProtoIndex::discover_network_dirs(&root);
        for net_dir in &network_dirs {
            let expected = partition.get(net_dir).cloned().unwrap_or_default();
            let actual = idx.children_of(net_dir).unwrap_or_default();
            assert_eq!(
                actual, expected,
                "children_of({net_dir:?}) should match net_dir_partition direct group"
            );
        }
    }

    #[test]
    fn test_root_children_contains_alpha_and_beta() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        let root_children = idx.children_of(&root).unwrap();

        // alpha.md and beta.md must appear in root's child list.
        let names: Vec<_> = root_children
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(
            names.contains(&"alpha.md"),
            "alpha.md should be in root; names={names:?}"
        );
        assert!(
            names.contains(&"beta.md"),
            "beta.md should be in root; names={names:?}"
        );

        // The subnet directory itself must appear (as a dir entry).
        assert!(
            names.contains(&"subnet"),
            "subnet dir entry should be in root; names={names:?}"
        );
    }

    #[test]
    fn test_subnet_children_correct() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let subnet = root.join("subnet");
        let idx = ProtoIndex::build(&root, false).unwrap();

        let subnet_children = idx.children_of(&subnet).unwrap();
        let names: Vec<_> = subnet_children
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(names.contains(&"gamma.md"));
        assert!(names.contains(&"delta.md"));
        // subnet's own index.md must not appear as a child of itself
        assert!(!names.contains(&NETWORK_NAME));
    }

    // -------------------------------------------------------------------------
    // sort_key_for()
    // -------------------------------------------------------------------------

    #[test]
    fn test_sort_key_matches_position_in_immediate_parent_list() {
        // sort_key_for(p) must return the index of p in its *owning* network's child list.
        // For direct children of root (alpha.md, beta.md) this is their root-list index.
        // For children of subnet (gamma.md, delta.md) this is their subnet-list index.
        // sort_key_for walks up the directory tree to find the owning network, so files
        // inside non-network subdirectories are also handled (see test below).
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let subnet = root.join("subnet");
        let idx = ProtoIndex::build(&root, false).unwrap();

        // Verify direct root children (alpha.md, beta.md) get their root-list position.
        let root_children = idx.children_of(&root).unwrap();
        for (expected_idx, child) in root_children.iter().enumerate() {
            // Only test files whose immediate parent IS root (not subnet files that
            // net_dir_children may also include in the root list due to ordering).
            if child.parent() != Some(root.as_path()) {
                continue;
            }
            let sk = idx.sort_key_for(child);
            assert_eq!(
                sk,
                Some(expected_idx as u16),
                "sort_key_for({child:?}) should be {expected_idx}"
            );
        }

        // Verify subnet children (gamma.md, delta.md) get their subnet-list position.
        let subnet_children = idx.children_of(&subnet).unwrap();
        for (expected_idx, child) in subnet_children.iter().enumerate() {
            let sk = idx.sort_key_for(child);
            assert_eq!(
                sk,
                Some(expected_idx as u16),
                "sort_key_for({child:?}) should be {expected_idx} in subnet"
            );
        }
    }

    /// Files inside a non-network subdirectory (one without an index.md) must be resolved
    /// against their ancestor network's child list.  This is the case that caused the
    /// BN-DB sort-key churn: `net1_dir1/hsml.md` where `net1_dir1/` has no index.md but
    /// the parent network's `net_dir_children` returns `net1_dir1/hsml.md` in its child list.
    #[test]
    fn test_sort_key_for_file_in_non_network_subdir() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();

        // Add a non-network subdirectory with a file directly under root.
        let plain_dir = root.join("plain_dir");
        fs::create_dir_all(&plain_dir).unwrap();
        // No index.md in plain_dir — it is NOT a network directory.
        fs::write(plain_dir.join("nested.md"), "# Nested\n").unwrap();

        // Rebuild the index after adding the new file.
        let idx = ProtoIndex::build(&root, false).unwrap();

        // plain_dir is NOT a network dir, so children_of(plain_dir) returns None.
        assert!(
            idx.children_of(&plain_dir).is_none(),
            "plain_dir has no index.md and should not be a known network dir"
        );

        // But sort_key_for(plain_dir/nested.md) must still succeed by walking up to root,
        // where net_dir_children includes nested.md in root's child list.
        let nested = plain_dir.join("nested.md");
        let sk = idx.sort_key_for(&nested);
        assert!(
            sk.is_some(),
            "sort_key_for should find nested.md in the ancestor root network's child list"
        );

        // The position must match where net_dir_children places nested.md in the root list.
        let root_children = idx.children_of(&root).unwrap();
        // Normalize nested's canonical form the same way build() normalizes child paths
        // (os_path_to_string + string_to_os_path strips the \\?\ prefix on Windows),
        // so the comparison against root_children entries is apples-to-apples.
        let nested_canonical =
            crate::paths::canonicalize_path(&nested).unwrap_or_else(|_| nested.clone());
        let expected_idx = root_children.iter().position(|p| p == &nested_canonical);
        assert_eq!(
            sk,
            expected_idx.map(|i| i as u16),
            "sort_key should match the position in the root network's net_dir_children output"
        );
    }

    #[test]
    fn test_sort_key_unknown_path_returns_none() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        let nonexistent = root.join("does_not_exist.md");
        assert_eq!(idx.sort_key_for(&nonexistent), None);
    }

    #[test]
    fn test_sort_key_index_md_itself_returns_none() {
        // The network's own index.md is not a child of itself.
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        let index_path = root.join(NETWORK_NAME);
        // index.md's parent is root; root's child list should not contain index.md.
        assert_eq!(
            idx.sort_key_for(&index_path),
            None,
            "index.md should not appear in its own parent's child list"
        );
    }

    // -------------------------------------------------------------------------
    // proto_for() vs NetworkCodec::proto() parity
    // -------------------------------------------------------------------------

    /// Core parity test: proto_for must produce the same upstream relation list as
    /// NetworkCodec::proto for every network directory in the fixture.
    /// Core parity test: `proto_for` must produce the same upstream relation list as
    /// calling `NetworkCodec::proto` + `NetworkCodec::prepare_proto_relations` directly,
    /// and both must agree with the raw `net_dir_children` output for the same directory.
    #[test]
    fn test_proto_for_upstream_matches_network_codec_proto() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        // Use net_dir_partition for the codec side: proto_for uses children_of(), which
        // returns the direct group from the partition (not the full DFS net_dir_children list).
        // The two must agree, so feed the same direct-group children to prepare_proto_relations.
        let mut partition = net_dir_partition(&root);
        let partition: BTreeMap<PathBuf, Vec<PathBuf>> = partition
            .iter_mut()
            .map(|(net_dir, children)| {
                let key =
                    crate::paths::canonicalize_path(net_dir).unwrap_or_else(|_| net_dir.clone());
                let vals: Vec<PathBuf> = children
                    .iter()
                    .map(|p| crate::paths::canonicalize_path(p).unwrap_or_else(|_| p.clone()))
                    .collect();
                (key, vals)
            })
            .collect();

        let network_dirs = ProtoIndex::discover_network_dirs(&root);
        for net_dir in &network_dirs {
            // Build the full codec proto the new way: proto() gives frontmatter only;
            // prepare_proto_relations adds the upstream child entries.
            let codec = NetworkCodec::default();
            let mut codec_proto = codec
                .proto(net_dir)
                .unwrap()
                .expect("fixture dirs all have index.md");
            let children = partition.get(net_dir).cloned().unwrap_or_default();
            codec
                .prepare_proto_relations(&mut codec_proto, net_dir, &children)
                .unwrap();

            let (index_proto, _git_status) = idx
                .proto_for(net_dir)
                .unwrap()
                .expect("proto_for should succeed for known network dirs");

            // Compare upstream path strings — the canonical sort-key ordering.
            let extract_paths = |proto: &IRNode| -> Vec<String> {
                proto
                    .upstream
                    .iter()
                    .filter_map(|r| {
                        if let NodeKey::Path { path, .. } = &r.key {
                            Some(path.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            };

            let codec_paths = extract_paths(&codec_proto);
            let index_paths = extract_paths(&index_proto);

            assert_eq!(
                index_paths, codec_paths,
                "proto_for upstream paths should match proto+prepare_proto_relations for {net_dir:?}"
            );
        }
    }

    #[test]
    fn test_proto_for_sets_network_kind_and_heading() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        let (proto, _git_status) = idx.proto_for(&root).unwrap().unwrap();
        assert!(proto.kind.contains(BeliefKind::Network));
        assert_eq!(proto.heading, 1);
    }

    #[test]
    fn test_proto_for_unknown_dir_returns_none() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        // A directory with no index.md.
        let no_net = root.join("subnet").join("subsubdir");
        fs::create_dir_all(&no_net).unwrap();
        let result = idx.proto_for(&no_net).unwrap();
        assert!(
            result.is_none(),
            "directory without index.md should return None"
        );
    }

    // -------------------------------------------------------------------------
    // Clone / Arc sharing
    // -------------------------------------------------------------------------

    #[test]
    fn test_clone_shares_inner_map() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();
        let clone = idx.clone();

        // Both handles should see the same data.
        let root_children_orig = idx.children_of(&root).unwrap();
        let root_children_clone = clone.children_of(&root).unwrap();
        assert_eq!(root_children_orig, root_children_clone);

        // They point to the same Arc.
        assert!(
            Arc::ptr_eq(&idx.inner, &clone.inner),
            "clone should share the same Arc"
        );
    }

    // -------------------------------------------------------------------------
    // build() error case
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_fails_without_index_md() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        // No index.md in root.
        let result = ProtoIndex::build(&root, false);
        assert!(
            result.is_err(),
            "build() without index.md should return Err"
        );
    }

    // -------------------------------------------------------------------------
    // ordered_paths()
    // -------------------------------------------------------------------------

    /// Verify that `ordered_paths()` returns paths in depth-first, network-sorted order:
    /// - The root network dir appears first.
    /// - Children of the root appear in lexicographic order (matching `net_dir_children`).
    /// - Subnet dir contents appear immediately after the subnet dir entry (DFS recursion).
    ///
    /// Structure (reuses build_fixture):
    /// ```text
    /// root/
    ///   index.md          (id = "root")
    ///   alpha.md
    ///   beta.md
    ///   subnet/           ← alphabetically AFTER alpha.md and beta.md
    ///     index.md        (id = "subnet")
    ///     delta.md
    ///     gamma.md
    /// ```
    ///
    /// Expected ordered_paths():
    ///   [root/, alpha.md, beta.md, subnet/, delta.md, gamma.md]
    ///   (root dir first, then root's children in lexicographic order — alpha.md and
    ///    beta.md before subnet/ — with subnet's own children immediately following it
    ///    DFS)
    #[test]
    fn test_ordered_paths_dfs_network_before_children_lex_order() {
        let tmp = build_fixture();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        let idx = ProtoIndex::build(&root, false).unwrap();

        let paths = idx.ordered_paths();

        // Extract file_name strings for readability
        let names: Vec<&str> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();

        // Root dir must be first
        assert_eq!(
            names.first().copied(),
            Some(root.file_name().unwrap().to_str().unwrap()),
            "root network dir must be first; names={names:?}"
        );

        // Pure lex order: alpha.md < beta.md < subnet (s > b).  DFS: root dir first, then
        // root's children in lexicographic order.  When dfs_ordered hits subnet/, it
        // recurses immediately — subnet's contents follow the subnet dir entry.
        let alpha_pos = names
            .iter()
            .position(|&n| n == "alpha.md")
            .expect("alpha.md must be present");
        let beta_pos = names
            .iter()
            .position(|&n| n == "beta.md")
            .expect("beta.md must be present");
        let subnet_pos = names
            .iter()
            .position(|&n| n == "subnet")
            .expect("subnet dir must be present");
        let gamma_pos = names
            .iter()
            .position(|&n| n == "gamma.md")
            .expect("gamma.md must be present");
        let delta_pos = names
            .iter()
            .position(|&n| n == "delta.md")
            .expect("delta.md must be present");

        // Pure lex order: alpha.md < beta.md < subnet/ (s > b).
        assert!(
            alpha_pos < subnet_pos,
            "alpha.md should sort before subnet/; names={names:?}"
        );
        assert!(
            beta_pos < subnet_pos,
            "beta.md should sort before subnet/; names={names:?}"
        );
        assert!(
            alpha_pos < beta_pos,
            "alpha.md should sort before beta.md; names={names:?}"
        );

        // DFS: subnet's children must immediately follow subnet/.
        assert!(
            delta_pos > subnet_pos,
            "delta.md should appear after subnet/; names={names:?}"
        );
        assert!(
            gamma_pos > subnet_pos,
            "gamma.md should appear after subnet/; names={names:?}"
        );
        // DFS: subnet children follow subnet/ immediately, before any remaining root-level
        // siblings that sort after subnet/ lexicographically.  alpha.md and beta.md sort
        // before subnet/ so they appear earlier; no root-level plain files sort after subnet/
        // in this fixture (z > s would, but none exist here).
        assert!(
            delta_pos > beta_pos,
            "delta.md (subnet child) should appear after beta.md; names={names:?}"
        );
        assert!(
            gamma_pos > beta_pos,
            "gamma.md (subnet child) should appear after beta.md; names={names:?}"
        );
    }

    /// Verify that `ordered_paths()` on an empty ProtoIndex returns an empty vec.
    #[test]
    fn test_ordered_paths_empty_index_returns_empty() {
        let idx = ProtoIndex::new();
        assert!(
            idx.ordered_paths().is_empty(),
            "ordered_paths on empty index must return empty vec"
        );
    }

    // -------------------------------------------------------------------------
    // network_dirs_by_tree_depth()
    // -------------------------------------------------------------------------

    /// Relative-path view of the grouping, for readable assertions.
    fn tree_depth_names(idx: &ProtoIndex, root: &Path) -> Vec<Vec<String>> {
        idx.network_dirs_by_tree_depth()
            .into_iter()
            .map(|group| {
                group
                    .iter()
                    .map(|p| {
                        p.strip_prefix(root)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .replace('\\', "/")
                    })
                    .collect()
            })
            .collect()
    }

    /// The motivating case: two subnets that are true tree-siblings (both direct
    /// children of the root in `children_of`) but whose paths differ in component
    /// count because one sits behind plain intervening directories.
    ///
    /// Component-count grouping put these in different epochs — serializing work
    /// with no dependency between it — and delayed the deeper-pathed one.
    #[test]
    fn test_tree_depth_groups_siblings_split_by_path_length() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        write_index(&root, "root");

        // Shallow sibling: root/near/
        let near = root.join("near");
        fs::create_dir_all(&near).unwrap();
        write_index(&near, "near");

        // Deep-pathed sibling: root/docs/parts/far/ — three extra components, but
        // `docs` and `parts` hold no index.md, so `far` is still a direct child of
        // root in the ProtoIndex.
        let far = root.join("docs").join("parts").join("far");
        fs::create_dir_all(&far).unwrap();
        write_index(&far, "far");

        let idx = ProtoIndex::build(&root, false).unwrap();

        // Precondition: both really are direct children of root.
        let root_children = idx.children_of(&root).unwrap();
        assert!(
            root_children.contains(&near) && root_children.contains(&far),
            "fixture invalid — both subnets should be direct children of root: {root_children:?}"
        );

        // Within a group, dirs keep network_dirs()'s ordering — component count
        // first, then lexicographic — so `near` precedes the deeper-pathed `far`.
        assert_eq!(
            tree_depth_names(&idx, &root),
            vec![vec![""], vec!["near", "docs/parts/far"]],
            "true siblings must share one group regardless of path length"
        );
    }

    /// A real subnet chain still serializes: tree depth increases by one per subnet
    /// hop, so no batching is manufactured where a genuine dependency exists.
    #[test]
    fn test_tree_depth_chain_stays_serial() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        write_index(&root, "root");

        let b = root.join("b");
        let c = b.join("c");
        let d = c.join("d");
        for (dir, id) in [(&b, "b"), (&c, "c"), (&d, "d")] {
            fs::create_dir_all(dir).unwrap();
            write_index(dir, id);
        }

        let idx = ProtoIndex::build(&root, false).unwrap();
        assert_eq!(
            tree_depth_names(&idx, &root),
            vec![vec![""], vec!["b"], vec!["b/c"], vec!["b/c/d"]],
            "a genuine subnet chain must remain one dir per group"
        );
    }

    /// The scheduling invariant `parse_all` phase 1 depends on: every dir in group
    /// D has its parent network in group D-1, so draining each group before the
    /// next guarantees ancestors are committed before their children parse.
    #[test]
    fn test_tree_depth_parent_is_always_in_previous_group() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::paths::canonicalize_path(tmp.path()).unwrap();
        write_index(&root, "root");

        // Mixed shapes: a branch point, plain-dir indirection at two levels, and
        // branches of differing depth that would drift apart under component count.
        for rel in ["b", "b/c", "b/wrapper/f", "b/c/deep/nested/g", "h", "h/i"] {
            let dir = root.join(rel);
            fs::create_dir_all(&dir).unwrap();
            write_index(&dir, &rel.replace('/', "-"));
        }

        let idx = ProtoIndex::build(&root, false).unwrap();
        let groups = idx.network_dirs_by_tree_depth();

        // Every indexed dir appears exactly once.
        let scheduled: Vec<PathBuf> = groups.iter().flatten().cloned().collect();
        let mut sorted = scheduled.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            scheduled.len(),
            "a network dir was scheduled more than once"
        );
        assert_eq!(
            sorted.len(),
            idx.network_dirs().len(),
            "grouping dropped or invented network dirs"
        );

        // Group 0 is exactly the repo root.
        assert_eq!(groups[0], vec![root.clone()], "group 0 must be the root");

        // Parent-in-previous-group holds for every non-root dir.
        for (depth, group) in groups.iter().enumerate().skip(1) {
            for dir in group {
                let parent = idx
                    .owning_net_dir_for(&dir.join(NETWORK_NAME))
                    .unwrap_or_else(|| panic!("{dir:?} has no owning network"));
                assert!(
                    groups[depth - 1].contains(&parent),
                    "{dir:?} is in group {depth} but its parent {parent:?} is not in group {}",
                    depth - 1
                );
            }
        }
    }

    // ── net_dir_children sort order ───────────────────────────────────

    /// Sort order for `net_dir_children`: subnet directories and plain files interleave
    /// lexicographically within each group.  The sort is hierarchical (DFS): all entries
    /// at the root level come before entries inside a subnet, with each subnet's contents
    /// immediately following it in DFS order.
    ///
    /// Corpus (from Issue 58 spec):
    /// ```text
    /// root/
    ///   index.md             ← root network (excluded)
    ///   a.md
    ///   b_dir/               ← subnet → sorts after a.md (b > a), before c.md
    ///     index.md
    ///     a_sub.md
    ///     aaaa.md
    ///     c_dir/             ← plain dir (no index.md) → files included flat
    ///       file.md
    ///     x.md
    ///     y_repo/            ← subnet → sorts after x.md (y > x)
    ///       index.md
    ///       abc.md
    ///   c.md
    ///   d_dir/               ← plain dir (no index.md) → files included flat
    ///     a.md
    ///     aaaa.md
    ///   z.md
    /// ```
    ///
    /// Expected flat order:
    ///   a.md, b_dir,
    ///   b_dir/a_sub.md, b_dir/aaaa.md, b_dir/c_dir/file.md, b_dir/x.md, b_dir/y_repo,
    ///   b_dir/y_repo/abc.md,
    ///   c.md, d_dir/a.md, d_dir/aaaa.md, z.md
    #[test]
    fn test_net_dir_children_sort_lex_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let write_md = |p: PathBuf, body: &str| {
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&p, body).unwrap();
        };

        write_md(root.join("index.md"), "---\ntitle = \"Root\"\n---\n");
        write_md(root.join("a.md"), "# A\n");
        write_md(root.join("c.md"), "# C\n");
        write_md(root.join("z.md"), "# Z\n");
        write_md(root.join("d_dir").join("a.md"), "# D/A\n");
        write_md(root.join("d_dir").join("aaaa.md"), "# D/AAAA\n");
        write_md(
            root.join("b_dir").join("index.md"),
            "---\ntitle = \"B\"\n---\n",
        );
        write_md(root.join("b_dir").join("a_sub.md"), "# A_sub\n");
        write_md(root.join("b_dir").join("aaaa.md"), "# AAAA\n");
        write_md(root.join("b_dir").join("x.md"), "# X\n");
        write_md(root.join("b_dir").join("c_dir").join("file.md"), "# File\n");
        write_md(
            root.join("b_dir").join("y_repo").join("index.md"),
            "---\ntitle = \"Y\"\n---\n",
        );
        write_md(root.join("b_dir").join("y_repo").join("abc.md"), "# ABC\n");

        let results = net_dir_children(root);
        let names: Vec<String> = results
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        let expected: Vec<&str> = vec![
            "a.md",
            "b_dir",
            "b_dir/a_sub.md",
            "b_dir/aaaa.md",
            "b_dir/c_dir/file.md",
            "b_dir/x.md",
            "b_dir/y_repo",
            "b_dir/y_repo/abc.md",
            "c.md",
            "d_dir/a.md",
            "d_dir/aaaa.md",
            "z.md",
        ];
        assert_eq!(
            names, expected,
            "net_dir_children sort order mismatch\ngot:      {names:?}\nexpected: {expected:?}"
        );
    }

    // ── symlink support ───────────────────────────────────────────────────────

    /// A symlinked directory that contains an `index.md` must be walked so its children
    /// appear in the index.  Because `build()` canonicalizes all keys, a symlink resolves
    /// to the same canonical path as the real directory — they share one index entry.
    /// The important invariant is that the content is reachable, not that the symlink
    /// gets a separate entry.
    ///
    /// Layout:
    /// ```text
    /// root/
    ///   index.md          (id = "root")
    ///   real_subnet/
    ///     index.md        (id = "real")
    ///     doc.md
    ///   link_subnet  ->   real_subnet   (symlink)
    /// ```
    /// Expected: `real_subnet` is indexed and its children include `doc.md`.
    /// `link_subnet` canonicalizes to the same path as `real_subnet`, so only one
    /// network-dir entry exists for that canonical path.
    #[test]
    #[cfg(unix)] // std::os::unix::fs::symlink is unix-only
    fn test_symlinked_subnet_is_discovered() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        write_index(root, "root");

        let real_subnet = root.join("real_subnet");
        fs::create_dir_all(&real_subnet).unwrap();
        write_index(&real_subnet, "real");
        fs::write(real_subnet.join("doc.md"), "# Doc\n").unwrap();

        // Create a symlink: root/link_subnet -> root/real_subnet
        let link_subnet = root.join("link_subnet");
        symlink(&real_subnet, &link_subnet).unwrap();

        let idx = ProtoIndex::build(root, false).unwrap();
        let root_canon = crate::paths::canonicalize_path(root).unwrap();

        // The real subnet must be present in the index.
        let net_dirs = idx.network_dirs();
        let names: Vec<String> = net_dirs
            .iter()
            .map(|p| {
                p.strip_prefix(&root_canon)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert!(
            names.iter().any(|n| n == "real_subnet"),
            "real_subnet must appear as a network dir; got: {names:?}"
        );

        // link_subnet canonicalizes to the same path as real_subnet.  Lookup by
        // canonical path must succeed and expose the subnet's children.
        let real_canon = crate::paths::canonicalize_path(&real_subnet).unwrap();
        let link_canon = crate::paths::canonicalize_path(&link_subnet).unwrap();
        assert_eq!(
            real_canon, link_canon,
            "sanity: symlink and target must resolve to the same canonical path"
        );

        let children = idx.children_of(&real_canon);
        assert!(
            children.is_some(),
            "children_of(real_subnet canonical) must return Some; network dirs: {names:?}"
        );
        let child_names: Vec<String> = children
            .unwrap()
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            child_names.iter().any(|n| n == "doc.md"),
            "doc.md must appear as child of real_subnet; got: {child_names:?}"
        );
    }

    /// A symlink cycle must not cause an infinite loop.  `WalkDir` with
    /// `follow_links(true)` detects cycles and emits an error; our code must
    /// handle that gracefully and continue building the rest of the index.
    ///
    /// Layout:
    /// ```text
    /// root/
    ///   index.md          (id = "root")
    ///   subnet/
    ///     index.md        (id = "subnet")
    ///     doc.md
    ///     loop  ->  root/subnet   (symlink back to parent — a cycle)
    /// ```
    #[test]
    #[cfg(unix)]
    fn test_symlink_cycle_does_not_hang() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        write_index(root, "root");

        let subnet = root.join("subnet");
        fs::create_dir_all(&subnet).unwrap();
        write_index(&subnet, "subnet");
        fs::write(subnet.join("doc.md"), "# Doc\n").unwrap();

        // Create a cycle: subnet/loop -> subnet (points back to itself)
        let loop_link = subnet.join("loop");
        symlink(&subnet, &loop_link).unwrap();

        // Must complete without hanging or panicking.
        let idx = ProtoIndex::build(root, false).unwrap();
        let root_canon = crate::paths::canonicalize_path(root).unwrap();

        // The non-cyclic parts of the tree must still be indexed correctly.
        let net_dirs = idx.network_dirs();
        let names: Vec<String> = net_dirs
            .iter()
            .map(|p| {
                p.strip_prefix(&root_canon)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert!(
            names.iter().any(|n| n == "subnet"),
            "subnet must still be discovered despite cycle; got: {names:?}"
        );

        let subnet_canon = crate::paths::canonicalize_path(&subnet).unwrap();
        let children = idx.children_of(&subnet_canon).unwrap_or_default();
        let child_names: Vec<String> = children
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            child_names.iter().any(|n| n == "doc.md"),
            "doc.md must appear despite cycle link; got: {child_names:?}"
        );
    }

    #[test]
    fn test_net_dir_partition_includes_yaml_files() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // create a minimal network
        fs::write(
            root.join("index.md"),
            "---\nid: test-net\ntitle: Test\n---\n",
        )
        .unwrap();
        fs::write(root.join("data.yaml"), "key: value\n").unwrap();
        fs::write(root.join("doc.md"), "# Doc\n").unwrap();

        let partition = net_dir_partition(root);
        let children = partition.get(root).unwrap();
        let names: Vec<&str> = children
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert!(
            names.contains(&"data.yaml"),
            "yaml should be included: {names:?}"
        );
        assert!(
            names.contains(&"doc.md"),
            "md should still be included: {names:?}"
        );
    }

    /// Walk codec for testing: declares "Manifest.test" as a network filename.
    struct TestManifestWalkCodec;

    impl WalkCodec for TestManifestWalkCodec {
        fn should_track(&self, path: &Path) -> bool {
            path.file_name().and_then(|n| n.to_str()) == Some("Manifest.test")
        }

        fn tracked_extensions(&self) -> Vec<&'static str> {
            vec!["test"]
        }

        fn network_filenames(&self) -> Vec<&'static str> {
            vec!["Manifest.test"]
        }
    }

    /// Register the test walk codec (idempotent — duplicates are harmless).
    fn register_test_walk_codec() {
        WALK_CODECS.register(Box::new(TestManifestWalkCodec));
    }

    #[test]
    fn test_net_dir_partition_custom_network_filename() {
        register_test_walk_codec();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Root has index.md as usual.
        write_index(root, "root");
        fs::write(root.join("doc.md"), "# Doc\n").unwrap();

        // Create a subdirectory with a Manifest.test file.
        let component = root.join("component");
        fs::create_dir_all(&component).unwrap();
        fs::write(component.join("Manifest.test"), "name = \"comp\"\n").unwrap();
        fs::write(component.join("data.yaml"), "key: value\n").unwrap();

        let partition = net_dir_partition(root);

        // The component directory should be a partition key (candidate subnet).
        assert!(
            partition.contains_key(&component),
            "component dir should be a candidate subnet in the partition; keys: {:?}",
            partition.keys().collect::<Vec<_>>()
        );

        // data.yaml should be a child of the component group, not the root.
        let component_children = partition.get(&component).unwrap();
        let child_names: Vec<&str> = component_children
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert!(
            child_names.contains(&"data.yaml"),
            "data.yaml should be child of component; got: {child_names:?}"
        );

        // Root should NOT contain data.yaml.
        let root_children = partition.get(root).unwrap();
        let root_child_names: Vec<&str> = root_children
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert!(
            !root_child_names.contains(&"data.yaml"),
            "data.yaml should NOT be in root group; got: {root_child_names:?}"
        );
    }

    #[test]
    fn test_proto_index_build_culls_false_positive_subnet() {
        register_test_walk_codec();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Root has index.md as usual.
        write_index(root, "root");
        fs::write(root.join("doc.md"), "# Doc\n").unwrap();

        // Create a subdirectory with Manifest.test but no registered CODECS
        // entry for it, so proto() will fail (no codec found → no
        // BeliefKind::Network). This is a false positive.
        let plumbing = root.join("plumbing");
        fs::create_dir_all(&plumbing).unwrap();
        fs::write(plumbing.join("Manifest.test"), "not a real network\n").unwrap();
        fs::write(plumbing.join("helper.yaml"), "x: 1\n").unwrap();

        // net_dir_partition should tentatively treat plumbing as a subnet.
        let partition = net_dir_partition(root);
        assert!(
            partition.contains_key(&plumbing),
            "plumbing should be a candidate subnet before culling"
        );

        // ProtoIndex::build should cull it (no codec registered for Manifest.test
        // → CODECS.path_get returns None → not a valid network).
        let idx = ProtoIndex::build(root, false).unwrap();
        let root_canon = crate::paths::canonicalize_path(root).unwrap();
        let plumbing_canon = crate::paths::canonicalize_path(&plumbing).unwrap();

        // Plumbing should NOT be a network dir after culling.
        let net_dirs = idx.network_dirs();
        assert!(
            !net_dirs.contains(&plumbing_canon),
            "plumbing should be culled from network dirs; got: {net_dirs:?}"
        );

        // But its children (helper.yaml) should be merged into root's child list.
        let root_children = idx.children_of(&root_canon).unwrap_or_default();
        let child_names: Vec<String> = root_children
            .iter()
            .filter_map(|p| Some(p.file_name()?.to_string_lossy().into_owned()))
            .collect();
        assert!(
            child_names.contains(&"helper.yaml".to_string()),
            "helper.yaml should be merged into root after culling; got: {child_names:?}"
        );
    }

    // -------------------------------------------------------------------------
    // codec_meta: set_meta / get_meta / get_meta_as
    // -------------------------------------------------------------------------

    #[test]
    fn test_set_meta_get_meta_round_trip() {
        let idx = ProtoIndex::new();
        let dir = PathBuf::from("/tmp/test_net");

        let val = serde_json::json!({"include_dirs": ["include/", "src/"]});
        idx.set_meta(&dir, "cmake", val.clone());

        let retrieved = idx.get_meta(&dir, "cmake");
        assert_eq!(retrieved, Some(val));
    }

    #[test]
    fn test_get_meta_unknown_namespace_returns_none() {
        let idx = ProtoIndex::new();
        let dir = PathBuf::from("/tmp/test_net");

        idx.set_meta(&dir, "cmake", serde_json::json!({"x": 1}));

        assert!(idx.get_meta(&dir, "git").is_none());
    }

    #[test]
    fn test_get_meta_unknown_dir_returns_none() {
        let idx = ProtoIndex::new();
        let dir = PathBuf::from("/tmp/test_net");

        idx.set_meta(&dir, "cmake", serde_json::json!({"x": 1}));

        let other = PathBuf::from("/tmp/other_net");
        assert!(idx.get_meta(&other, "cmake").is_none());
    }

    #[test]
    fn test_get_meta_as_deserializes_typed_struct() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct MockMeta {
            include_dirs: Vec<String>,
            target_name: String,
        }

        let idx = ProtoIndex::new();
        let dir = PathBuf::from("/tmp/test_net");

        let original = MockMeta {
            include_dirs: vec!["include/".to_string(), "src/".to_string()],
            target_name: "my_component".to_string(),
        };
        idx.set_meta(&dir, "test", serde_json::to_value(&original).unwrap());

        let retrieved: Option<MockMeta> = idx.get_meta_as(&dir, "test");
        assert_eq!(retrieved, Some(original));
    }

    #[test]
    fn test_get_meta_as_returns_none_on_type_mismatch() {
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct WrongShape {
            nonexistent_field: u64,
        }

        let idx = ProtoIndex::new();
        let dir = PathBuf::from("/tmp/test_net");

        idx.set_meta(&dir, "test", serde_json::json!({"x": "hello"}));

        let retrieved: Option<WrongShape> = idx.get_meta_as(&dir, "test");
        assert!(
            retrieved.is_none(),
            "Deserialization of mismatched type should return None"
        );
    }

    #[test]
    fn test_set_meta_multiple_namespaces() {
        let idx = ProtoIndex::new();
        let dir = PathBuf::from("/tmp/test_net");

        idx.set_meta(&dir, "git", serde_json::json!({"branch": "main"}));
        idx.set_meta(&dir, "cmake", serde_json::json!({"target": "lib"}));

        assert_eq!(
            idx.get_meta(&dir, "git"),
            Some(serde_json::json!({"branch": "main"}))
        );
        assert_eq!(
            idx.get_meta(&dir, "cmake"),
            Some(serde_json::json!({"target": "lib"}))
        );
    }

    #[test]
    fn test_ancestor_meta_as_finds_parent() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct AliasConfig {
            template: String,
        }

        let idx = ProtoIndex::new();
        let parent_dir = PathBuf::from("/tmp/test_net");
        let child_path = PathBuf::from("/tmp/test_net/child.md");

        let config = AliasConfig {
            template: "/docs/{{ slug }}".to_string(),
        };
        idx.set_meta(
            &parent_dir,
            "url_alias",
            serde_json::to_value(&config).unwrap(),
        );

        let result = idx.ancestor_meta_as::<AliasConfig>(&child_path, "url_alias");
        assert!(result.is_some(), "Should find parent's metadata");
        let (found_dir, found_config) = result.unwrap();
        assert_eq!(found_config.template, "/docs/{{ slug }}");
        // The found_dir should be the canonical form of parent_dir
        assert!(
            found_dir.ends_with("test_net"),
            "Found dir should end with test_net, got {:?}",
            found_dir
        );
    }

    #[test]
    fn test_ancestor_meta_as_returns_none_when_absent() {
        #[derive(Debug, serde::Serialize, serde::Deserialize)]
        struct AliasConfig {
            template: String,
        }

        let idx = ProtoIndex::new();
        let child_path = PathBuf::from("/tmp/test_net/child.md");

        let result = idx.ancestor_meta_as::<AliasConfig>(&child_path, "url_alias");
        assert!(
            result.is_none(),
            "Should return None when no ancestor has metadata"
        );
    }

    #[test]
    fn test_ancestor_meta_as_finds_grandparent() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct AliasConfig {
            template: String,
        }

        let idx = ProtoIndex::new();
        let grandparent_dir = PathBuf::from("/tmp/test_net");
        let child_path = PathBuf::from("/tmp/test_net/subdir/child.md");

        let config = AliasConfig {
            template: "/docs/{{ slug }}".to_string(),
        };
        idx.set_meta(
            &grandparent_dir,
            "url_alias",
            serde_json::to_value(&config).unwrap(),
        );

        let result = idx.ancestor_meta_as::<AliasConfig>(&child_path, "url_alias");
        assert!(result.is_some(), "Should find grandparent's metadata");
        let (_found_dir, found_config) = result.unwrap();
        assert_eq!(found_config.template, "/docs/{{ slug }}");
    }
}
