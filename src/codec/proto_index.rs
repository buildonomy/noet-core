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
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};
use walkdir::{DirEntry, WalkDir};

use crate::{
    codec::{
        network::{detect_network_file, NETWORK_NAME},
        IRNode, CODECS,
    },
    error::BuildonomyError,
    paths::{os_path_to_string, string_to_os_path, AnchorPath},
    properties::BeliefKind,
};

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
    // WalkDir does not guarantee that `index.md` is yielded before sibling files
    // within the same directory — the order depends on the underlying `readdir(2)`
    // call, which is filesystem- and OS-dependent.  A single-pass approach that
    // populates `subnets` lazily as `index.md` files are encountered therefore
    // misclassifies sibling files that are yielded *before* their directory's
    // `index.md`: the `subnets.iter().any(|s| p.starts_with(s))` guard fires
    // false, so those files are included in the root group instead of being
    // excluded (and later assigned to their correct subnet group).
    //
    // Pre-scanning for all subnet dirs in one pass makes Pass 2 order-independent.
    let subnet_dirs: std::collections::BTreeSet<PathBuf> = WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) || e.path() == path)
        .filter_map(|e| e.ok().map(|e| e.into_path()))
        .filter_map(|mut p| {
            if p.is_file() {
                let p_str = os_path_to_string(&p);
                let p_ap = AnchorPath::new(&p_str);
                if NETWORK_NAME == p_ap.filename() {
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
    //   • subnet `index.md`  → represented as the subnet directory path (is_dir());
    //     `group_of` routes it to its parent subnet (or root)
    //   • codec files owned by any subnet OR root → all included; `group_of` routes
    //     each file to its deepest owning subnet key in the by_group loop below.
    //     No per-file filtering is needed here because `group_of` uses the complete
    //     `subnet_dirs` set from Pass 1 and is therefore order-independent.
    //   • root `index.md` → excluded entirely
    //   • non-codec files → excluded entirely
    let files = WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) || e.path() == path)
        .filter_map(|e| e.ok().map(|e| e.into_path()))
        .filter_map(|mut p| {
            if p.is_file() {
                let p_str = os_path_to_string(&p);
                let p_ap = AnchorPath::new(&p_str);
                if NETWORK_NAME == p_ap.filename() {
                    // Subnet index.md — represent the subnet as its directory path.
                    p.pop();
                    if !p.eq(path) {
                        return Some(p); // subnet dir entry (is_dir() in by_group loop)
                    } else {
                        return None; // root's own index.md — exclude
                    }
                }
                // Use new_file: p.is_file() is confirmed, prevents extensionless files
                // (Gemfile, Makefile, …) from matching the (None, None) codec wildcard.
                let p_ap_file = AnchorPath::new_file(&p_str);
                if CODECS.get(&p_ap_file).is_some() {
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
    pub fn build(repo_root: &Path) -> Result<Self, BuildonomyError> {
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
        let partition = net_dir_partition(&repo_root_buf);
        let map: HashMap<PathBuf, Vec<PathBuf>> = partition
            .into_iter()
            .map(|(net_dir, children)| {
                let key = {
                    let p = net_dir.canonicalize().unwrap_or(net_dir);
                    string_to_os_path(&os_path_to_string(&p))
                };
                let children = children
                    .into_iter()
                    .map(|p| {
                        let c = p.canonicalize().unwrap_or(p);
                        string_to_os_path(&os_path_to_string(&c))
                    })
                    .collect();
                (key, children)
            })
            .collect();

        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
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
        let canonical_root = {
            let p = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            string_to_os_path(&os_path_to_string(&p))
        };
        let mut dirs: Vec<PathBuf> = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| {
                // Allow the root entry unconditionally (it may live in a hidden temp dir).
                // Skip all other hidden entries — same rule as net_dir_children.
                let entry_canonical = {
                    let p = e
                        .path()
                        .canonicalize()
                        .unwrap_or_else(|_| e.path().to_path_buf());
                    string_to_os_path(&os_path_to_string(&p))
                };
                entry_canonical == canonical_root
                    || !e
                        .file_name()
                        .to_str()
                        .map(|s| s.starts_with('.'))
                        .unwrap_or(false)
            })
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.into_path();
                if p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n == NETWORK_NAME)
                        .unwrap_or(false)
                {
                    // Return the canonicalized parent directory, not the index.md file itself.
                    p.parent().map(|d| {
                        let c = d.canonicalize().unwrap_or_else(|_| d.to_path_buf());
                        string_to_os_path(&os_path_to_string(&c))
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

    /// Returns the lexically-ordered direct children of `dir`, or `None` if `dir` is not a
    /// known network directory.
    ///
    /// This is a read-only lookup after `build()` completes.
    pub fn children_of(&self, dir: &Path) -> Option<Vec<PathBuf>> {
        // Canonicalize the lookup key so callers using raw or canonicalized paths both hit.
        let canonical = {
            let p = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            string_to_os_path(&os_path_to_string(&p))
        };
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

    /// Returns the 0-based sort key for `abs_path` within its owning network directory.
    ///
    /// This is the canonical, single source of truth used by **both** the fast path
    /// (`try_initialize_stack_from_session_cache`) and the slow path (`initialize_stack`),
    /// replacing the dual-source logic (session_bb Section edge scan + proto_cache fallback)
    /// that caused the BN-DB sort-key churn instability.
    ///
    /// The owning network may not be the immediate parent directory.  For files in a
    /// non-network subdirectory (e.g. `net1_dir1/hsml.md` where `net1_dir1/` has no
    /// `index.md`), `net_dir_children` includes the file in the **ancestor** network's child
    /// list (flattened).  `sort_key_for` walks up the directory tree until it finds a
    /// network directory whose child list contains `abs_path`, then returns that position.
    ///
    /// Returns `None` if:
    /// - `abs_path` has no parent directory, or
    /// - no ancestor directory in the ProtoIndex contains `abs_path` in its child list.
    pub fn sort_key_for(&self, abs_path: &Path) -> Option<u16> {
        // If abs_path is a network index file (ends with NETWORK_NAME / "index.md"), the
        // ProtoIndex child lists record the *directory* path, not the index file itself.
        // Use the parent directory as the canonical lookup target so that, e.g.,
        // `subnet1/index.md` matches the `subnet1/` entry in the root network's child list.
        let lookup_path: std::borrow::Cow<Path> =
            if abs_path.file_name().and_then(|n| n.to_str()) == Some(NETWORK_NAME) {
                std::borrow::Cow::Owned(abs_path.parent()?.to_path_buf())
            } else {
                std::borrow::Cow::Borrowed(abs_path)
            };

        // Canonicalize once for all comparisons against canonicalized child entries.
        let canonical = {
            let p = lookup_path
                .canonicalize()
                .unwrap_or_else(|_| lookup_path.to_path_buf());
            string_to_os_path(&os_path_to_string(&p))
        };

        // Walk up the directory tree, checking each ancestor directory that is a known
        // network dir (i.e. present in the ProtoIndex).  The first hit that contains
        // `canonical` in its child list is the owning network.
        let mut dir = lookup_path.parent()?;
        loop {
            if let Some(children) = self.children_of(dir) {
                if let Some(idx) = children.iter().position(|child| child == &canonical) {
                    return Some(idx as u16);
                }
                // This dir is a known network but doesn't contain abs_path — keep walking up.
                // (Shouldn't happen in practice, but be safe.)
            }
            dir = dir.parent()?;
        }
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
    pub fn proto_for(&self, dir: &Path) -> Result<Option<IRNode>, BuildonomyError> {
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
        let network_dir = network_dir_buf.as_path();

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
        proto.heading = 1;

        // Populate upstream with Section child-path relations from the cached child list.
        // Falls back to net_dir_children for paths not in the index (e.g. out-of-repo callers).
        let children = match self.children_of(network_dir) {
            Some(c) => c,
            None => net_dir_children(network_dir)
                .into_iter()
                .map(|p| {
                    let c = p.canonicalize().unwrap_or(p);
                    string_to_os_path(&os_path_to_string(&c))
                })
                .collect(),
        };

        // Call prepare_proto_relations via the registered codec so custom network codec
        // implementations can express their own file-based child relations.
        codec_factory().prepare_proto_relations(&mut proto, network_dir, &children)?;

        Ok(Some(proto))
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
    use crate::codec::DocCodec;
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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

        let mut partition = net_dir_partition(&root);
        // Canonicalize partition keys and values to match ProtoIndex's stored paths.
        let partition: BTreeMap<_, _> = partition
            .iter_mut()
            .map(|(net_dir, children)| {
                let key = {
                    let p = net_dir.canonicalize().unwrap_or_else(|_| net_dir.clone());
                    string_to_os_path(&os_path_to_string(&p))
                };
                let vals: Vec<PathBuf> = children
                    .iter()
                    .map(|p| {
                        let c = p.canonicalize().unwrap_or_else(|_| p.clone());
                        string_to_os_path(&os_path_to_string(&c))
                    })
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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

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
        let root = tmp.path().canonicalize().unwrap();
        let subnet = root.join("subnet");
        let idx = ProtoIndex::build(&root).unwrap();

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
        let root = tmp.path().canonicalize().unwrap();
        let subnet = root.join("subnet");
        let idx = ProtoIndex::build(&root).unwrap();

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
        let root = tmp.path().canonicalize().unwrap();

        // Add a non-network subdirectory with a file directly under root.
        let plain_dir = root.join("plain_dir");
        fs::create_dir_all(&plain_dir).unwrap();
        // No index.md in plain_dir — it is NOT a network directory.
        fs::write(plain_dir.join("nested.md"), "# Nested\n").unwrap();

        // Rebuild the index after adding the new file.
        let idx = ProtoIndex::build(&root).unwrap();

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
        let nested_canonical = {
            let c = nested.canonicalize().unwrap_or_else(|_| nested.clone());
            string_to_os_path(&os_path_to_string(&c))
        };
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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

        let nonexistent = root.join("does_not_exist.md");
        assert_eq!(idx.sort_key_for(&nonexistent), None);
    }

    #[test]
    fn test_sort_key_index_md_itself_returns_none() {
        // The network's own index.md is not a child of itself.
        let tmp = build_fixture();
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

        // Use net_dir_partition for the codec side: proto_for uses children_of(), which
        // returns the direct group from the partition (not the full DFS net_dir_children list).
        // The two must agree, so feed the same direct-group children to prepare_proto_relations.
        let mut partition = net_dir_partition(&root);
        let partition: BTreeMap<PathBuf, Vec<PathBuf>> = partition
            .iter_mut()
            .map(|(net_dir, children)| {
                let key = {
                    let p = net_dir.canonicalize().unwrap_or_else(|_| net_dir.clone());
                    string_to_os_path(&os_path_to_string(&p))
                };
                let vals: Vec<PathBuf> = children
                    .iter()
                    .map(|p| {
                        let c = p.canonicalize().unwrap_or_else(|_| p.clone());
                        string_to_os_path(&os_path_to_string(&c))
                    })
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

            let index_proto = idx
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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

        let proto = idx.proto_for(&root).unwrap().unwrap();
        assert!(proto.kind.contains(BeliefKind::Network));
        assert_eq!(proto.heading, 1);
    }

    #[test]
    fn test_proto_for_unknown_dir_returns_none() {
        let tmp = build_fixture();
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();
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
        let root = tmp.path().canonicalize().unwrap();
        // No index.md in root.
        let result = ProtoIndex::build(&root);
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
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

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

    // ── net_dir_children sort order ───────────────────────────────────────────

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
}
