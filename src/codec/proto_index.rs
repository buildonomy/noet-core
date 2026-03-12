//! # ProtoIndex
//!
//! Pre-built filesystem index of every network directory in the repo, derived from a single
//! `WalkDir` pass at compiler startup.
//!
//! ## Motivation
//!
//! `NetworkCodec::proto` calls `iter_net_docs` (a `WalkDir` subtree scan) every time it is
//! asked to produce a proto for a network directory.  In `initialize_stack`, this fires once
//! per ancestor directory per parsed document — O(networks × files) scans total.
//!
//! `ProtoIndex` replaces that pattern:
//!
//! 1. **Build once** (`ProtoIndex::build`) — one `WalkDir` from `repo_root` partitions every
//!    reachable file into its owning network directory.  The result is identical to running
//!    `iter_net_docs` separately for each network, but costs one filesystem pass instead of N.
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
//! - Not a full replacement for `NetworkCodec::proto` in all contexts — only replaces it in
//!   the `initialize_stack` call chain.  `NetworkCodec::proto` keeps its own `iter_net_docs`
//!   call for contexts where `ProtoIndex` is not available (e.g. `create_network_file`).
//! - Not a holder of all relation types — `proto_for` only populates `upstream` with
//!   `WeightKind::Section` child-path relations (the only type `NetworkCodec::proto` puts
//!   there).  Schema-derived and markdown-link edges are populated later by `MdCodec::parse`
//!   and `traverse_schema`.

use parking_lot::RwLock;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    codec::{
        belief_ir::IntermediateRelation,
        md::MdCodec,
        network::{detect_network_file, iter_net_docs, NETWORK_NAME},
        DocCodec, IRNode,
    },
    error::BuildonomyError,
    nodekey::NodeKey,
    paths::{os_path_to_string, string_to_os_path},
    properties::{BeliefKind, Bref, Weight, WeightKind},
};

/// Filesystem-level index of every network directory in the repo.
///
/// Maps each absolute network directory path → its lexically-ordered list of direct children
/// (files with registered codec extensions, plus subnet directories).  The ordering is
/// identical to `iter_net_docs` output: lexicographic by path components.
///
/// # Thread Safety
///
/// `Clone` on `ProtoIndex` clones the `Arc` handle only — the underlying map is shared.
/// After `build()` the map is read-only; the `RwLock` guards the initial population only.
/// Concurrent reads during parsing take the read lock, which is uncontended.
#[derive(Clone, Debug)]
pub struct ProtoIndex {
    /// `PathBuf` = absolute network directory
    /// `Vec<PathBuf>` = lexically-ordered direct children produced by the repo-wide scan
    inner: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>,
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
    /// Produces the same per-directory child lists that calling `iter_net_docs` separately
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

        // Delegate to iter_net_docs for each discovered network directory.
        //
        // Strategy: first collect all network directories in the repo by doing a lightweight
        // top-down walk looking only for index.md files; then call iter_net_docs once per
        // network dir to get the correctly-pruned, correctly-sorted child list for that dir.
        //
        // Why not a single custom partition walk?  iter_net_docs contains the authoritative
        // hidden-file filter, extensionless-file guard (new_file vs new AnchorPath), and
        // subnet-pruning logic.  Duplicating that inline risks drift.  Calling iter_net_docs
        // per directory is O(total_files) amortised across all calls from a single build()
        // invocation (each file is visited once by the top-level network-discovery walk, then
        // once by the iter_net_docs call for its owning network).  This is O(2 × files) total
        // — still one repo-wide scan's worth of work — rather than the O(networks × files)
        // that previously happened across the full parse session.
        //
        // All map keys and child paths are canonicalized so that lookup keys derived from
        // canonicalized paths (e.g. from Path::canonicalize() in the caller) always match.
        let mut map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

        // Discover all network directories via a lightweight walk.
        let network_dirs = Self::discover_network_dirs(repo_root);

        // For each discovered network dir, get its direct children via iter_net_docs.
        // Canonicalize the dir key and each child path so lookups are always consistent.
        for net_dir in &network_dirs {
            let key = {
                let p = net_dir.canonicalize().unwrap_or_else(|_| net_dir.clone());
                string_to_os_path(&os_path_to_string(&p))
            };
            let children: Vec<PathBuf> = iter_net_docs(net_dir)
                .into_iter()
                .map(|p| {
                    let c = p.canonicalize().unwrap_or(p);
                    string_to_os_path(&os_path_to_string(&c))
                })
                .collect();
            map.insert(key, children);
        }

        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
        })
    }

    /// Discover all network directories under `root` (directories containing `index.md`),
    /// including `root` itself.  Returns them in lexicographic order (shallowest first).
    ///
    /// All returned paths are canonicalized so they match the canonicalized keys used in
    /// `build()` and expected by `children_of` / `sort_key_for` callers.
    pub(crate) fn discover_network_dirs(root: &Path) -> Vec<PathBuf> {
        use walkdir::WalkDir;
        // Canonicalize root so we can use it as the "allow root even if hidden" reference,
        // mirroring iter_net_docs's `!is_hidden(e) || e.path() == path.as_ref()` guard.
        let canonical_root = {
            let p = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            string_to_os_path(&os_path_to_string(&p))
        };
        let mut dirs: Vec<PathBuf> = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| {
                // Allow the root entry unconditionally (it may live in a hidden temp dir).
                // Skip all other hidden entries — same rule as iter_net_docs.
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

    /// Returns the 0-based sort key for `abs_path` within its owning network directory.
    ///
    /// This is the canonical, single source of truth used by **both** the fast path
    /// (`try_initialize_stack_from_session_cache`) and the slow path (`initialize_stack`),
    /// replacing the dual-source logic (session_bb Section edge scan + proto_cache fallback)
    /// that caused the BN-DB sort-key churn instability.
    ///
    /// The owning network may not be the immediate parent directory.  For files in a
    /// non-network subdirectory (e.g. `net1_dir1/hsml.md` where `net1_dir1/` has no
    /// `index.md`), `iter_net_docs` includes the file in the **ancestor** network's child
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
    /// `WeightKind::Section` entries (derived from `iter_net_docs`) into `upstream`; all
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
        let network_dir = network_filepath
            .parent()
            .expect("detect_network_file returns a path.is_file() path; parent() must succeed");

        // Read frontmatter only — no WalkDir.
        let Some(mut proto) = MdCodec::new().proto(network_filepath.as_ref())? else {
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
        // Mirrors exactly what NetworkCodec::proto does, but reads from self instead of
        // calling iter_net_docs again.
        let children = match self.children_of(network_dir) {
            Some(c) => c,
            // Directory is not in the index (e.g. built without this dir, or called on a
            // path that wasn't in the original repo_root scan) — fall back to iter_net_docs
            // so proto_for is still correct for out-of-index callers.
            None => iter_net_docs(network_dir)
                .into_iter()
                .map(|p| {
                    let c = p.canonicalize().unwrap_or(p);
                    string_to_os_path(&os_path_to_string(&c))
                })
                .collect(),
        };

        for child_path in &children {
            let relative_path = child_path
                .strip_prefix(network_dir)
                .expect("children are always under network_dir");
            let path_str = os_path_to_string(relative_path);
            if !path_str.is_empty() {
                let node_key = NodeKey::Path {
                    net: Bref::default(),
                    path: path_str.clone(),
                };
                let mut weight = Weight::default();
                weight.set_doc_paths(vec![path_str]).ok();
                proto.upstream.push(IntermediateRelation::new(
                    node_key,
                    WeightKind::Section,
                    Some(weight),
                ));
            }
        }

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

    /// Verify that ProtoIndex::build produces the same per-directory child list as calling
    /// iter_net_docs directly for each network directory.  This is the ground-truth parity test.
    #[test]
    fn test_build_matches_iter_net_docs_per_directory() {
        let tmp = build_fixture();
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

        let network_dirs = ProtoIndex::discover_network_dirs(&root);
        for net_dir in &network_dirs {
            let expected = iter_net_docs(net_dir);
            let actual = idx.children_of(net_dir).unwrap_or_default();
            assert_eq!(
                actual, expected,
                "children_of({net_dir:?}) should match iter_net_docs output"
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
            // iter_net_docs may also include in the root list due to ordering).
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
    /// the parent network's `iter_net_docs` returns `net1_dir1/hsml.md` in its child list.
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
        // where iter_net_docs includes nested.md in root's child list.
        let nested = plain_dir.join("nested.md");
        let sk = idx.sort_key_for(&nested);
        assert!(
            sk.is_some(),
            "sort_key_for should find nested.md in the ancestor root network's child list"
        );

        // The position must match where iter_net_docs places nested.md in the root list.
        let root_children = idx.children_of(&root).unwrap();
        let expected_idx = root_children
            .iter()
            .position(|p| p == &nested.canonicalize().unwrap_or_else(|_| nested.clone()));
        assert_eq!(
            sk,
            expected_idx.map(|i| i as u16),
            "sort_key should match the position in the root network's iter_net_docs output"
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
    #[test]
    fn test_proto_for_upstream_matches_network_codec_proto() {
        let tmp = build_fixture();
        let root = tmp.path().canonicalize().unwrap();
        let idx = ProtoIndex::build(&root).unwrap();

        let network_dirs = ProtoIndex::discover_network_dirs(&root);
        for net_dir in &network_dirs {
            let codec_proto = NetworkCodec::default()
                .proto(net_dir)
                .unwrap()
                .expect("fixture dirs all have index.md");
            let index_proto = idx
                .proto_for(net_dir)
                .unwrap()
                .expect("proto_for should succeed for known network dirs");

            // Compare upstream path strings — the canonical sort-key ordering.
            let codec_paths: Vec<String> = codec_proto
                .upstream
                .iter()
                .filter_map(|r| {
                    if let NodeKey::Path { path, .. } = &r.key {
                        Some(path.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let index_paths: Vec<String> = index_proto
                .upstream
                .iter()
                .filter_map(|r| {
                    if let NodeKey::Path { path, .. } = &r.key {
                        Some(path.clone())
                    } else {
                        None
                    }
                })
                .collect();

            assert_eq!(
                index_paths, codec_paths,
                "proto_for upstream paths should match NetworkCodec::proto for {net_dir:?}"
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
}
