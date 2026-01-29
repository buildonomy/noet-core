# Issue 20: Git-Aware BeliefNetwork Nodes

**Priority**: MEDIUM - Post-v0.1.0 feature
**Estimated Effort**: 3-4 days
**Dependencies**: None (standalone feature)

## Summary

Inject Git repository metadata into BeliefNetwork nodes during parse time, tracking commit hash, branch, dirty status, and upstream info. Git status is **path-local** to each network (only tracks changes within the network's directory), enabling per-network version control awareness for publishing workflows, CI/CD validation, and multi-repository documentation sites.

**Use Cases**:
- **Publishing validation**: Reject exports with uncommitted changes
- **Version tracking**: Embed git commit in exported HTML
- **Multi-network sync**: Only sync networks without pending changes
- **CI/CD integration**: Validate all networks are committed before deployment
- **Audit trails**: Capture git state in beliefbase snapshots

## Goals

1. Detect Git repository for each BeliefNetwork node
2. Inject path-local git status into BeliefNode payload during parsing
3. Track: commit hash, branch name, dirty flag, upstream, ahead/behind counts
4. Make git tracking optional/configurable (performance, privacy)
5. Handle edge cases: no git repo, nested repos, submodules
6. Provide CLI commands for git-based validation and queries

## Architecture

### Git Metadata in BeliefNode Payload

During parse, inject git status into network node's payload:

```yaml
# BeliefNetwork.yaml (after injection)
bid: "550e8400-e29b-41d4-a716-446655440000"
kind: "Document"
schema: "buildonomy.network"
title: "noet-core Documentation"

# Git metadata (injected, not user-editable)
git:
  repo_root: "../.."           # Relative path to .git directory
  commit: "a1b2c3d4e5f6..."    # Current HEAD commit (full SHA)
  commit_short: "a1b2c3d"      # Short SHA (7 chars)
  branch: "main"               # Current branch name
  upstream: "origin/main"      # Configured upstream branch
  dirty: false                 # Uncommitted changes in network path?
  untracked: 0                 # Count of untracked files in network path
  modified: 0                  # Count of modified files in network path
  ahead: 2                     # Commits ahead of upstream
  behind: 0                    # Commits behind upstream
  last_commit_date: "2024-01-15T10:30:00Z"
  checked_at: "2024-01-15T14:22:33Z"  # When status was computed
```

### Path-Local Git Status

**Critical requirement**: Only track changes within the network's path.

**Example**:
```
/project/.git
/project/docs/core/BeliefNetwork.yaml    ← Network path: docs/core/
/project/docs/tutorials/                 ← Different network
/project/src/                            ← Outside any network
```

If changes exist in `/project/src/` but not in `/project/docs/core/`, the `docs/core` network should **not** be marked dirty.

**Implementation**: Use `git status --porcelain <path>` or libgit2 path filtering.

### Configuration

Make git tracking configurable:

```yaml
# In BeliefNetwork.yaml (per-network config)
git_tracking:
  enabled: true                # Enable git metadata injection
  include_file_lists: false    # Don't list every modified file (just counts)
  include_upstream: true       # Query remote tracking info
  check_submodules: true       # Recurse into submodules
```

Or global config in project root:

```toml
# .noet.toml (project-wide config)
[git_tracking]
enabled = true
cache_duration_secs = 300    # Cache git status for 5 minutes (performance)
fail_if_no_repo = false      # Allow networks outside git repos
```

### Data Structures

```rust
use git2::{Repository, StatusOptions};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    /// Relative path to .git directory
    pub repo_root: PathBuf,
    
    /// Full commit SHA
    pub commit: String,
    
    /// Short commit SHA (7 chars)
    pub commit_short: String,
    
    /// Current branch name (if on a branch)
    pub branch: Option<String>,
    
    /// Upstream tracking branch
    pub upstream: Option<String>,
    
    /// Any uncommitted changes in network path?
    pub dirty: bool,
    
    /// Count of untracked files in network path
    pub untracked: usize,
    
    /// Count of modified files in network path
    pub modified: usize,
    
    /// Commits ahead of upstream
    pub ahead: usize,
    
    /// Commits behind upstream
    pub behind: usize,
    
    /// Last commit timestamp
    pub last_commit_date: DateTime<Utc>,
    
    /// When this status was computed
    pub checked_at: DateTime<Utc>,
}

impl GitStatus {
    /// Compute git status for a specific path (network directory)
    pub fn for_path(path: &Path) -> Result<Option<Self>, BuildonomyError> {
        // 1. Find git repository
        let repo = match Repository::discover(path) {
            Ok(r) => r,
            Err(_) => return Ok(None), // Not in a git repo
        };
        
        // 2. Get HEAD commit
        let head = repo.head()?;
        let commit_obj = head.peel_to_commit()?;
        let commit = commit_obj.id().to_string();
        let commit_short = commit[..7].to_string();
        
        // 3. Get branch name
        let branch = head.shorthand().map(String::from);
        
        // 4. Get upstream branch
        let upstream = if let Some(ref branch_name) = branch {
            repo.find_branch(branch_name, git2::BranchType::Local)?
                .upstream()
                .ok()
                .and_then(|b| b.name().ok().flatten().map(String::from))
        } else {
            None
        };
        
        // 5. Check dirty status (path-local only!)
        let mut opts = StatusOptions::new();
        opts.pathspec(path); // CRITICAL: Filter by network path
        let statuses = repo.statuses(Some(&mut opts))?;
        
        let mut untracked = 0;
        let mut modified = 0;
        
        for entry in statuses.iter() {
            let status = entry.status();
            if status.is_wt_new() {
                untracked += 1;
            }
            if status.is_wt_modified() || status.is_index_modified() {
                modified += 1;
            }
        }
        
        let dirty = untracked > 0 || modified > 0;
        
        // 6. Compute ahead/behind counts
        let (ahead, behind) = if let Some(ref upstream_name) = upstream {
            let local_oid = commit_obj.id();
            let upstream_ref = repo.find_reference(upstream_name)?;
            let upstream_oid = upstream_ref.peel_to_commit()?.id();
            
            repo.graph_ahead_behind(local_oid, upstream_oid)
                .unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        
        // 7. Get last commit date
        let last_commit_date = DateTime::from_timestamp(commit_obj.time().seconds(), 0)
            .unwrap_or_else(|| Utc::now());
        
        Ok(Some(GitStatus {
            repo_root: repo.path().to_path_buf(),
            commit,
            commit_short,
            branch,
            upstream,
            dirty,
            untracked,
            modified,
            ahead,
            behind,
            last_commit_date,
            checked_at: Utc::now(),
        }))
    }
}
```

## Implementation Steps

### 1. Add git2 Dependency (0.5 days)

- [ ] Add `git2` crate to Cargo.toml
- [ ] Add optional `git-tracking` feature flag
- [ ] Document git tracking in README

```toml
[dependencies]
git2 = { version = "0.18", optional = true }

[features]
default = []
git-tracking = ["dep:git2"]
```

### 2. Implement GitStatus Computation (1 day)

- [ ] `GitStatus::for_path()` - Compute git status for network directory
- [ ] Handle no git repo gracefully (return `None`)
- [ ] Filter status by network path (path-local changes only)
- [ ] Compute ahead/behind counts vs upstream
- [ ] Extract branch name and commit info
- [ ] Handle detached HEAD state

**Key Implementation Detail**: Use `StatusOptions::pathspec()` to filter:

```rust
let network_relative_path = path.strip_prefix(repo.workdir().unwrap())?;
let mut opts = StatusOptions::new();
opts.pathspec(network_relative_path);
let statuses = repo.statuses(Some(&mut opts))?;
```

### 3. Integrate Git Status into GraphBuilder (1 day)

- [ ] Add git enrichment phase to GraphBuilder
- [ ] Inject git status into BeliefNetwork node payload
- [ ] Make injection optional based on config
- [ ] Cache git status per repository (performance optimization)
- [ ] Handle errors gracefully (log warning, don't fail parse)

```rust
impl GraphBuilder {
    fn enrich_with_git_status(&mut self, node: &mut BeliefNode) -> Result<()> {
        // Only enrich network nodes
        if node.schema != Some("buildonomy.network".to_string()) {
            return Ok(());
        }
        
        // Check if git tracking enabled
        if !self.config.git_tracking_enabled() {
            return Ok(());
        }
        
        // Compute git status for network path
        let network_path = self.resolve_network_path(node)?;
        
        // Check cache first (avoid redundant git queries)
        let git_status = if let Some(cached) = self.git_cache.get(&network_path) {
            cached.clone()
        } else {
            let status = GitStatus::for_path(&network_path)?;
            if let Some(ref s) = status {
                self.git_cache.insert(network_path.clone(), s.clone());
            }
            status
        };
        
        // Inject into payload
        if let Some(status) = git_status {
            node.payload.insert(
                "git".to_string(),
                toml::Value::try_from(status)?
            );
        }
        
        Ok(())
    }
}
```

### 4. Add CLI Commands for Git Queries (1 day)

- [ ] `noet git-status` - Show git status for all networks
- [ ] `noet validate --require-clean` - Fail if any network has uncommitted changes
- [ ] `noet export-html --require-clean` - Only export clean networks
- [ ] `noet query --filter git.dirty=true` - Query by git status

```rust
// CLI: noet git-status
#[derive(Parser)]
struct GitStatusArgs {
    /// Show only dirty networks
    #[arg(long)]
    dirty_only: bool,
    
    /// Show file-level details
    #[arg(long)]
    verbose: bool,
}

fn cmd_git_status(args: GitStatusArgs) -> Result<()> {
    let belief_base = BeliefBase::from_path(".")?;
    
    for network in belief_base.networks() {
        if let Some(git) = network.git_status() {
            if args.dirty_only && !git.dirty {
                continue;
            }
            
            println!("Network: {}", network.title);
            println!("  Commit: {} ({})", git.commit_short, git.branch.as_deref().unwrap_or("detached"));
            println!("  Dirty: {}", git.dirty);
            
            if args.verbose && git.dirty {
                println!("  Modified: {}", git.modified);
                println!("  Untracked: {}", git.untracked);
            }
            
            println!();
        }
    }
    
    Ok(())
}

// CLI: noet validate --require-clean
fn validate_clean_git(belief_base: &BeliefBase) -> Result<()> {
    let mut dirty_networks = Vec::new();
    
    for network in belief_base.networks() {
        if let Some(git) = network.git_status() {
            if git.dirty {
                dirty_networks.push(network.title.clone());
            }
        }
    }
    
    if !dirty_networks.is_empty() {
        return Err(BuildonomyError::ValidationFailed(
            format!("Networks with uncommitted changes: {}", dirty_networks.join(", "))
        ));
    }
    
    Ok(())
}
```

### 5. Documentation and Examples (0.5 days)

- [ ] Document git tracking in architecture.md
- [ ] Add examples to README (CI/CD validation workflow)
- [ ] Document performance implications (git queries add latency)
- [ ] Explain path-local status filtering

## Testing Requirements

### Unit Tests

- [ ] `GitStatus::for_path()` with mock git repo
- [ ] Path-local filtering (changes outside network path ignored)
- [ ] Branch detection (named branch vs detached HEAD)
- [ ] Upstream tracking (ahead/behind counts)
- [ ] No git repo handling (graceful None return)
- [ ] Submodule handling

### Integration Tests

- [ ] Create test git repo with multiple networks
- [ ] Modify files in one network, verify others not marked dirty
- [ ] Test with uncommitted changes, staged changes, untracked files
- [ ] Test with multiple branches and upstreams
- [ ] Test performance with large repos (cache effectiveness)

### Manual Testing

- [ ] Run on noet-core itself (self-hosting test)
- [ ] Test with nested git repos (submodules)
- [ ] Test with no git repo (non-git directories)
- [ ] Verify CI/CD validation workflow
- [ ] Check git status in exported HTML

## Success Criteria

- [ ] BeliefNetwork nodes have `git` field in payload after parsing
- [ ] Git status is path-local (only tracks network directory changes)
- [ ] Commit hash, branch, and dirty flag are accurate
- [ ] Upstream tracking and ahead/behind counts work
- [ ] Git tracking is optional (can be disabled for performance)
- [ ] CLI commands for git status queries work
- [ ] `--require-clean` validation prevents dirty exports
- [ ] Performance is acceptable (git queries cached per repo)
- [ ] Works with no git repo (graceful degradation)
- [ ] Documentation includes CI/CD workflow examples

## Risks

**Risk 1: Performance Overhead**
- **Impact**: Git queries slow down parsing, especially for large repos
- **Mitigation**: Cache git status per repo, make optional, async queries

**Risk 2: Path Filtering Complexity**
- **Impact**: Incorrectly marks networks dirty due to changes elsewhere
- **Mitigation**: Comprehensive tests, careful pathspec usage, path normalization

**Risk 3: Submodule Handling**
- **Impact**: Nested git repos confuse status detection
- **Mitigation**: Use libgit2's submodule APIs, test extensively, document behavior

**Risk 4: Stale Data**
- **Impact**: Payload written to disk becomes outdated quickly
- **Mitigation**: Always recompute on parse (ignore file value), add `checked_at` timestamp

**Risk 5: Cross-Platform Path Handling**
- **Impact**: Git paths differ on Windows vs Unix
- **Mitigation**: Use `Path` abstractions consistently, test on multiple platforms

## Open Questions

1. **Should git status be written back to BeliefNetwork files?**
   - **Recommendation**: NO - always recompute on parse, treat as ephemeral metadata

2. **Cache duration**: How long to cache git status before recomputing?
   - **Recommendation**: 5 minutes default, configurable

3. **Submodule recursion**: Should we track submodule status?
   - **Recommendation**: Phase 2 feature, defer for MVP

4. **Remote queries**: Should we fetch upstream to check ahead/behind?
   - **Recommendation**: NO - use local refs only (faster, works offline)

5. **Dirty definition**: Include staged changes as dirty, or only working tree?
   - **Recommendation**: Include both (any uncommitted changes = dirty)

## Future Work (Post-Issue 20)

- **Git hooks integration**: Auto-parse on git commit/checkout
- **Submodule recursion**: Track submodule status separately
- **Remote sync**: Fetch upstream before computing ahead/behind
- **File-level tracking**: List specific modified files (not just counts)
- **Git blame integration**: Track last-modified-by per node
- **Tag support**: Detect and inject git tags
- **Worktree support**: Handle multiple git worktrees

### Visual Diff in Browser (WASM)

**Use Case**: GitHub Pages hosts multiple versions of documentation (e.g., `/v1.0.0/`, `/v1.1.0/`, `/main/`). Users can select two versions and see a visual diff highlighting what changed between releases.

**Architecture**:
```javascript
class NoetVersionDiff {
  async compareVersions(versionA, versionB) {
    // 1. Load both BeliefBase snapshots
    const baseA = await fetch(`/${versionA}/belief-network.json`)
      .then(r => r.json())
      .then(data => wasm.BeliefBaseWasm.from_json(data));
    
    const baseB = await fetch(`/${versionB}/belief-network.json`)
      .then(r => r.json())
      .then(data => wasm.BeliefBaseWasm.from_json(data));
    
    // 2. Compute diff in WASM (fast, client-side)
    const diffEvents = baseA.compute_diff(baseB);
    
    // 3. Stylize current HTML based on diff
    this.applyDiffStyling(diffEvents);
  }
  
  applyDiffStyling(events) {
    for (const event of events) {
      const element = document.querySelector(`[data-bid="${event.bid}"]`);
      
      if (event.type === 'NodeAdded') {
        element.classList.add('diff-added');
      } else if (event.type === 'NodeRemoved') {
        element.classList.add('diff-removed');
      } else if (event.type === 'NodeUpdated') {
        element.classList.add('diff-modified');
        this.showChangedFields(element, event.changes);
      }
    }
  }
}
```

**Benefits**:
- No server-side diff processing required
- Works entirely in browser (GitHub Pages compatible)
- Leverages existing `BeliefBase::compute_diff()` implementation
- Git commit metadata enables version selection UI
- Real-time comparison between any two published versions

**Example UI**:
```html
<div class="version-diff-toolbar">
  <select id="version-a">
    <option value="v1.0.0">v1.0.0 (abc123)</option>
    <option value="v1.1.0">v1.1.0 (def456)</option>
    <option value="main" selected>main (789abc)</option>
  </select>
  
  <span>compared to</span>
  
  <select id="version-b">
    <option value="v1.0.0" selected>v1.0.0 (abc123)</option>
    <option value="v1.1.0">v1.1.0 (def456)</option>
    <option value="main">main (789abc)</option>
  </select>
  
  <button onclick="diff.compareVersions(versionA, versionB)">
    Show Diff
  </button>
</div>

<!-- Document content with diff styling -->
<article class="noet-document">
  <h1 class="diff-added">New Section</h1>
  <p>Added content...</p>
  
  <h2 class="diff-modified">Modified Section</h2>
  <p class="diff-removed">Old text</p>
  <p class="diff-added">New text</p>
</article>
```

**CSS Styling**:
```css
.diff-added {
  background-color: #e6ffed;
  border-left: 4px solid #28a745;
}

.diff-removed {
  background-color: #ffeef0;
  border-left: 4px solid #d73a49;
  text-decoration: line-through;
}

.diff-modified {
  background-color: #fff8c5;
  border-left: 4px solid #ffab00;
}
```

**Integration Points**:
- Git-aware networks (Issue 20) provide commit metadata for version selection
- WASM BeliefBase (Issue 06) provides `compute_diff()` in browser
- Static HTML export (Issue 06) generates versioned sites
- Per-network theming (Issue 19) can style diff views per network

See Issue 06 for WASM integration architecture.

## References

- `git2-rs` documentation: https://docs.rs/git2/
- libgit2 status API: https://libgit2.org/libgit2/#HEAD/group/status
- Git porcelain format: https://git-scm.com/docs/git-status#_porcelain_format_version_1
- Issue 19: Per-Network Theming (similar BeliefNetwork payload usage)