//! Git repository metadata for BeliefNetwork nodes.
//!
//! Provides [`GitCache`], which is built once during [`ProtoIndex::build`] (when the
//! `git-tracking` feature is enabled) and shared across all parallel parse tasks via
//! [`ProtoIndex`]'s existing `Arc<RwLock<...>>` pattern.
//!
//! ## Design
//!
//! - One [`git2::Repository`] open per repo workdir root (keyed by canonicalized workdir).
//! - Per-network dirty/untracked/modified counts are path-local: only files under the
//!   network's subdirectory are considered.
//! - All fields that may be absent (detached HEAD, no upstream) use `Option`.
//! - Any `git2` error logs a warning and returns `None`; the parse continues without
//!   git metadata rather than failing.
//!
//! ## Feature gate
//!
//! All types and functions in this module are gated on `#[cfg(feature = "git-tracking")]`.
//! Code outside this module that conditionally uses git metadata should use the same gate.

#[cfg(feature = "git-tracking")]
pub use inner::*;

#[cfg(feature = "git-tracking")]
mod inner {
    use git2::{Repository, StatusOptions};
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::Arc,
    };

    // -------------------------------------------------------------------------
    // Data types
    // -------------------------------------------------------------------------

    /// Repo-level git status shared by every network that lives inside the same
    /// git repository.  Populated once per workdir root during [`GitCache::populate`].
    #[derive(Debug, Clone)]
    pub struct RepoGitStatus {
        /// Full HEAD SHA.
        pub commit: String,
        /// Short SHA (7 hex chars).
        pub commit_short: String,
        /// Current branch name.  `None` when HEAD is detached.
        pub branch: Option<String>,
        /// Upstream tracking branch (e.g. `"origin/main"`).  `None` when none is configured.
        pub upstream: Option<String>,
        /// Commits ahead of upstream (0 when no upstream).
        pub ahead: usize,
        /// Commits behind upstream (0 when no upstream).
        pub behind: usize,
        /// RFC 3339 timestamp of the HEAD commit.
        pub last_commit_date: String,
        /// RFC 3339 timestamp of when this status was computed.
        pub checked_at: String,
        /// Normalized HTTPS base URL for the `origin` remote, e.g.
        /// `"https://github.com/org/repo"`.  `None` when no remote is configured
        /// or the remote URL is unrecognized.
        pub remote_url: Option<String>,
    }

    /// Network-level git status: path-local dirty flags plus a reference to the
    /// shared [`RepoGitStatus`] for the containing repository.
    #[derive(Debug, Clone)]
    pub struct NetworkGitStatus {
        /// Path from the network directory to the `.git` directory (relative).
        pub repo_root: PathBuf,
        /// Path from the git workdir root to this network directory (e.g. `tests/network_1`).
        /// Empty when the network directory IS the git workdir root.
        /// Used by `compute_source_url` to build repo-root-relative source URLs.
        pub network_prefix: PathBuf,
        /// `true` if any tracked file under the network directory has uncommitted changes.
        pub dirty: bool,
        /// Count of untracked files under the network directory.
        pub untracked: usize,
        /// Count of modified (but not staged) files under the network directory.
        pub modified: usize,
        /// Shared repo-level status (commit, branch, upstream, …).
        pub repo: Arc<RepoGitStatus>,
    }

    impl NetworkGitStatus {
        /// Convert this status into a `toml::value::Table` suitable for storing in
        /// `BeliefNode.metadata["git"]`.
        ///
        /// All fields are present except optional ones (`branch`, `upstream`,
        /// `remote_url`) which are omitted when `None`.
        pub fn to_metadata_table(&self) -> toml::value::Table {
            use toml::value::{Table, Value};
            let mut t = Table::new();

            // Network-local fields
            t.insert(
                "repo_root".to_string(),
                Value::String(self.repo_root.to_string_lossy().into_owned()),
            );
            // Forward-slash form so compute_source_url can join it with root_path directly.
            let network_prefix_fwd = self
                .network_prefix
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            t.insert(
                "network_prefix".to_string(),
                Value::String(network_prefix_fwd),
            );
            t.insert("dirty".to_string(), Value::Boolean(self.dirty));
            t.insert(
                "untracked".to_string(),
                Value::Integer(self.untracked as i64),
            );
            t.insert("modified".to_string(), Value::Integer(self.modified as i64));

            // Repo-level fields
            let r = &self.repo;
            t.insert("commit".to_string(), Value::String(r.commit.clone()));
            t.insert(
                "commit_short".to_string(),
                Value::String(r.commit_short.clone()),
            );
            if let Some(branch) = &r.branch {
                t.insert("branch".to_string(), Value::String(branch.clone()));
            }
            if let Some(upstream) = &r.upstream {
                t.insert("upstream".to_string(), Value::String(upstream.clone()));
            }
            t.insert("ahead".to_string(), Value::Integer(r.ahead as i64));
            t.insert("behind".to_string(), Value::Integer(r.behind as i64));
            t.insert(
                "last_commit_date".to_string(),
                Value::String(r.last_commit_date.clone()),
            );
            t.insert(
                "checked_at".to_string(),
                Value::String(r.checked_at.clone()),
            );
            if let Some(remote_url) = &r.remote_url {
                t.insert("remote_url".to_string(), Value::String(remote_url.clone()));
            }

            t
        }
    }

    // -------------------------------------------------------------------------
    // GitCache
    // -------------------------------------------------------------------------

    /// Cache of git status, keyed by canonicalized git workdir root.
    ///
    /// Build via [`GitCache::populate`], then query via [`GitCache::get`].
    /// Stored inside `ProtoIndex` so all parallel epoch tasks share a single instance.
    #[derive(Debug, Default)]
    pub struct GitCache {
        /// Keyed by canonicalized workdir root returned by `repo.workdir()`.
        by_repo: HashMap<PathBuf, Arc<RepoGitStatus>>,
        /// Keyed by canonicalized network directory path.
        by_network: HashMap<PathBuf, NetworkGitStatus>,
    }

    impl GitCache {
        /// Compute git status for every network directory in `network_dirs`.
        ///
        /// Opens each repository at most once (shared across networks in the same repo).
        /// Errors from `git2` are logged as warnings and skipped — a network directory
        /// that is not inside any git repository simply receives no git metadata.
        pub fn populate(network_dirs: &[PathBuf]) -> Self {
            let mut cache = GitCache::default();
            for dir in network_dirs {
                if let Err(e) = cache.compute_for_network(dir) {
                    tracing::warn!(
                        "[GitCache] failed to compute git status for {}: {}",
                        dir.display(),
                        e
                    );
                }
            }
            cache
        }

        /// Look up the cached [`NetworkGitStatus`] for a network directory.
        ///
        /// Returns `None` if the directory was not inside any git repository at populate time.
        pub fn get(&self, network_dir: &Path) -> Option<&NetworkGitStatus> {
            let canonical = network_dir
                .canonicalize()
                .unwrap_or_else(|_| network_dir.to_path_buf());
            self.by_network.get(&canonical)
        }

        // ------------------------------------------------------------------
        // Private helpers
        // ------------------------------------------------------------------

        fn compute_for_network(&mut self, network_dir: &Path) -> Result<(), git2::Error> {
            let canonical_dir = network_dir
                .canonicalize()
                .unwrap_or_else(|_| network_dir.to_path_buf());

            let repo = Repository::discover(&canonical_dir)?;
            let workdir = match repo.workdir() {
                Some(w) => w.canonicalize().unwrap_or_else(|_| w.to_path_buf()),
                None => {
                    // Bare repository — skip.
                    tracing::debug!(
                        "[GitCache] skipping bare repository for {}",
                        canonical_dir.display()
                    );
                    return Ok(());
                }
            };

            // Open the Repository once per workdir root and cache the repo-level status.
            let repo_status = if let Some(existing) = self.by_repo.get(&workdir) {
                Arc::clone(existing)
            } else {
                let status = Arc::new(compute_repo_status(&repo)?);
                self.by_repo.insert(workdir.clone(), Arc::clone(&status));
                status
            };

            // Relative path from workdir → network directory, used as the pathspec for
            // path-local status queries.
            let network_relative = canonical_dir
                .strip_prefix(&workdir)
                .unwrap_or(Path::new(""))
                .to_path_buf();

            // Relative path from network directory → .git directory.
            // strip_prefix always succeeds here: canonical_dir was produced by
            // Repository::discover starting from within workdir, so workdir is
            // guaranteed to be a prefix of canonical_dir (or equal to it).
            let repo_root_rel = workdir
                .strip_prefix(&canonical_dir)
                .unwrap_or(Path::new(""))
                .to_path_buf();

            // Path-local dirty/untracked/modified counts.
            let (dirty, untracked, modified) = compute_path_local_status(&repo, &network_relative)?;

            let net_status = NetworkGitStatus {
                repo_root: repo_root_rel,
                network_prefix: network_relative,
                dirty,
                untracked,
                modified,
                repo: repo_status,
            };
            self.by_network.insert(canonical_dir, net_status);
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Repo-level status computation
    // -------------------------------------------------------------------------

    fn compute_repo_status(repo: &Repository) -> Result<RepoGitStatus, git2::Error> {
        let head = repo.head()?;
        let commit = head.peel_to_commit()?;
        let commit_id = commit.id();
        let commit_str = commit_id.to_string();
        let commit_short = commit_str[..7.min(commit_str.len())].to_string();

        let branch = if head.is_branch() {
            head.shorthand().map(|s| s.to_string())
        } else {
            None // detached HEAD
        };

        // Upstream tracking branch and ahead/behind counts.
        let (upstream, ahead, behind) = compute_upstream(repo, &branch);

        // Commit timestamp → RFC 3339.
        let commit_time = commit.time();
        let last_commit_date = format_git_time(commit_time.seconds());

        let checked_at = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            format_git_time(secs)
        };

        // Normalized remote URL.
        let remote_url = normalize_origin_url(repo);

        Ok(RepoGitStatus {
            commit: commit_str,
            commit_short,
            branch,
            upstream,
            ahead,
            behind,
            last_commit_date,
            checked_at,
            remote_url,
        })
    }

    fn compute_upstream(
        repo: &Repository,
        branch: &Option<String>,
    ) -> (Option<String>, usize, usize) {
        let branch_name = match branch {
            Some(b) => b,
            None => return (None, 0, 0),
        };

        // Look up the local branch reference and its upstream.
        let local_ref = match repo.find_branch(branch_name, git2::BranchType::Local) {
            Ok(b) => b,
            Err(_) => return (None, 0, 0),
        };
        let upstream_branch = match local_ref.upstream() {
            Ok(u) => u,
            Err(_) => return (None, 0, 0),
        };
        let upstream_name = upstream_branch.name().ok().flatten().map(|s| s.to_string());

        let local_oid = match repo.head().ok().and_then(|h| h.target()) {
            Some(oid) => oid,
            None => return (upstream_name, 0, 0),
        };
        let upstream_oid = match upstream_branch.get().target() {
            Some(oid) => oid,
            None => return (upstream_name, 0, 0),
        };

        let (ahead, behind) = repo
            .graph_ahead_behind(local_oid, upstream_oid)
            .unwrap_or((0, 0));

        (upstream_name, ahead, behind)
    }

    // -------------------------------------------------------------------------
    // Path-local status
    // -------------------------------------------------------------------------

    fn compute_path_local_status(
        repo: &Repository,
        network_relative: &Path,
    ) -> Result<(bool, usize, usize), git2::Error> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false);

        // Empty relative path means the network IS the repo root — no pathspec needed.
        let path_str = network_relative.to_string_lossy();
        if !path_str.is_empty() {
            opts.pathspec(path_str.as_ref());
        }

        let statuses = repo.statuses(Some(&mut opts))?;

        let mut untracked = 0usize;
        let mut modified = 0usize;

        for entry in statuses.iter() {
            let s = entry.status();
            if s.contains(git2::Status::WT_NEW) {
                untracked += 1;
            }
            if s.contains(git2::Status::WT_MODIFIED)
                || s.contains(git2::Status::INDEX_MODIFIED)
                || s.contains(git2::Status::INDEX_NEW)
                || s.contains(git2::Status::INDEX_DELETED)
                || s.contains(git2::Status::WT_DELETED)
                || s.contains(git2::Status::WT_RENAMED)
                || s.contains(git2::Status::INDEX_RENAMED)
            {
                modified += 1;
            }
        }

        let dirty = untracked > 0 || modified > 0;
        Ok((dirty, untracked, modified))
    }

    // -------------------------------------------------------------------------
    // Remote URL normalization
    // -------------------------------------------------------------------------

    /// Normalize the `origin` remote URL to an HTTPS base URL suitable for
    /// constructing blob links.
    ///
    /// Returns `None` when:
    /// - No `origin` remote is configured.
    /// - The remote URL pattern is not recognized (not GitHub or GitLab).
    ///
    /// SSH remotes (`git@github.com:org/repo.git`) are converted to HTTPS form.
    /// The `.git` suffix is stripped.
    pub fn normalize_remote_url(raw: &str) -> Option<String> {
        let raw = raw.trim();

        // SSH form: git@<host>:<org>/<repo>[.git]
        if let Some(rest) = raw.strip_prefix("git@") {
            // rest = "github.com:org/repo.git"
            if let Some(colon_pos) = rest.find(':') {
                let host = &rest[..colon_pos];
                let path = rest[colon_pos + 1..].trim_end_matches(".git");
                if is_known_host(host) {
                    return Some(format!("https://{}/{}", host, path));
                }
            }
            return None;
        }

        // HTTPS form: https://<host>/<org>/<repo>[.git]
        if let Some(rest) = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))
        {
            if let Some(slash_pos) = rest.find('/') {
                let host = &rest[..slash_pos];
                if is_known_host(host) {
                    let path = rest.trim_end_matches('/').trim_end_matches(".git");
                    return Some(format!("https://{}", path));
                }
            }
            return None;
        }

        None
    }

    fn is_known_host(host: &str) -> bool {
        host == "github.com" || host == "gitlab.com"
    }

    fn normalize_origin_url(repo: &Repository) -> Option<String> {
        let remote = repo.find_remote("origin").ok()?;
        let url = remote.url()?;
        normalize_remote_url(url)
    }

    // -------------------------------------------------------------------------
    // Time formatting
    // -------------------------------------------------------------------------

    /// Format a Unix timestamp (seconds) as a naive RFC 3339 UTC string.
    /// Uses manual arithmetic to avoid pulling in `chrono` or `time`.
    fn format_git_time(unix_secs: i64) -> String {
        // Days since Unix epoch → calendar date via the Gregorian proleptic calendar.
        // Algorithm: Neri-Schneider (2023), adapted from
        // <https://howardhinnant.github.io/date_algorithms.html>
        let secs_per_day: i64 = 86400;
        let secs_of_day = unix_secs.rem_euclid(secs_per_day);
        let days = (unix_secs - secs_of_day) / secs_per_day;

        // Shift epoch from 1970-01-01 to 0000-03-01 for the algorithm.
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };

        let hour = secs_of_day / 3600;
        let minute = (secs_of_day % 3600) / 60;
        let second = secs_of_day % 60;

        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            y, m, d, hour, minute, second
        )
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_normalize_github_https() {
            assert_eq!(
                normalize_remote_url("https://github.com/org/repo"),
                Some("https://github.com/org/repo".to_string())
            );
        }

        #[test]
        fn test_normalize_github_https_git_suffix() {
            assert_eq!(
                normalize_remote_url("https://github.com/org/repo.git"),
                Some("https://github.com/org/repo".to_string())
            );
        }

        #[test]
        fn test_normalize_github_ssh() {
            assert_eq!(
                normalize_remote_url("git@github.com:org/repo.git"),
                Some("https://github.com/org/repo".to_string())
            );
        }

        #[test]
        fn test_normalize_github_ssh_no_git_suffix() {
            assert_eq!(
                normalize_remote_url("git@github.com:org/repo"),
                Some("https://github.com/org/repo".to_string())
            );
        }

        #[test]
        fn test_normalize_gitlab_https() {
            assert_eq!(
                normalize_remote_url("https://gitlab.com/org/repo.git"),
                Some("https://gitlab.com/org/repo".to_string())
            );
        }

        #[test]
        fn test_normalize_gitlab_ssh() {
            assert_eq!(
                normalize_remote_url("git@gitlab.com:org/repo.git"),
                Some("https://gitlab.com/org/repo".to_string())
            );
        }

        #[test]
        fn test_normalize_unknown_host_returns_none() {
            assert_eq!(
                normalize_remote_url("https://bitbucket.org/org/repo.git"),
                None
            );
            assert_eq!(normalize_remote_url("git@bitbucket.org:org/repo.git"), None);
        }

        #[test]
        fn test_normalize_gitea_returns_none() {
            assert_eq!(
                normalize_remote_url("https://git.example.com/org/repo.git"),
                None
            );
        }

        #[test]
        fn test_normalize_empty_returns_none() {
            assert_eq!(normalize_remote_url(""), None);
        }

        #[test]
        fn test_format_git_time_unix_epoch() {
            assert_eq!(format_git_time(0), "1970-01-01T00:00:00Z");
        }

        #[test]
        fn test_format_git_time_known_date() {
            // 2024-01-15 10:30:00 UTC = 1705314600
            assert_eq!(format_git_time(1705314600), "2024-01-15T10:30:00Z");
        }

        #[test]
        fn test_format_git_time_leap_year() {
            // 2000-02-29 00:00:00 UTC = 951782400
            assert_eq!(format_git_time(951782400), "2000-02-29T00:00:00Z");
        }

        #[test]
        fn test_gitcache_populate_no_repo() {
            // A temp directory with no git repo should produce an empty cache.
            let dir = std::env::temp_dir().join("noet_gitcache_test_no_repo");
            std::fs::create_dir_all(&dir).ok();
            let cache = GitCache::populate(&[dir.clone()]);
            // No git repo → no entry in cache.
            assert!(cache.get(&dir).is_none());
        }

        #[test]
        fn test_gitcache_populate_with_real_repo() {
            // The noet-core source tree is itself a git repo — use it as a fixture.
            // This test is only meaningful when run from within the repo.
            let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let cache = GitCache::populate(&[repo_root.clone()]);
            if let Some(status) = cache.get(&repo_root) {
                // Commit hash should be 40 hex chars.
                assert_eq!(status.repo.commit.len(), 40);
                assert_eq!(status.repo.commit_short.len(), 7);
                // checked_at should look like an ISO timestamp.
                assert!(status.repo.checked_at.contains('T'));
            }
            // If no entry, the repo may be in a weird state — just don't panic.
        }

        #[test]
        fn test_path_local_dirty_detection() {
            // Create a temp git repo, add a file, and verify dirty detection.
            let tmp = tempfile::tempdir().expect("tempdir");
            let repo = Repository::init(tmp.path()).expect("git init");

            // Initially clean — no files.
            let (dirty, untracked, modified) =
                compute_path_local_status(&repo, Path::new("")).expect("status");
            assert!(!dirty);
            assert_eq!(untracked, 0);
            assert_eq!(modified, 0);

            // Create an untracked file.
            std::fs::write(tmp.path().join("hello.txt"), "hi").expect("write");
            let (dirty, untracked, _modified) =
                compute_path_local_status(&repo, Path::new("")).expect("status");
            assert!(dirty);
            assert_eq!(untracked, 1);
        }

        #[test]
        fn test_path_local_dirty_outside_network_is_clean() {
            // Files outside the network subdir must not make it dirty.
            let tmp = tempfile::tempdir().expect("tempdir");
            let repo = Repository::init(tmp.path()).expect("git init");

            // Create a subdir for our "network".
            let net_dir = tmp.path().join("docs");
            std::fs::create_dir_all(&net_dir).expect("mkdir docs");

            // Write a file OUTSIDE the network dir.
            std::fs::write(tmp.path().join("outside.txt"), "hi").expect("write outside");

            let (dirty, untracked, _modified) =
                compute_path_local_status(&repo, Path::new("docs")).expect("status");
            assert!(!dirty, "file outside network dir must not mark it dirty");
            assert_eq!(untracked, 0);
        }

        #[test]
        fn test_path_local_dirty_inside_network_is_dirty() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let repo = Repository::init(tmp.path()).expect("git init");

            let net_dir = tmp.path().join("docs");
            std::fs::create_dir_all(&net_dir).expect("mkdir docs");
            std::fs::write(net_dir.join("inside.txt"), "hi").expect("write inside");

            let (dirty, untracked, _modified) =
                compute_path_local_status(&repo, Path::new("docs")).expect("status");
            assert!(dirty, "file inside network dir must mark it dirty");
            assert_eq!(untracked, 1);
        }
    }
}
