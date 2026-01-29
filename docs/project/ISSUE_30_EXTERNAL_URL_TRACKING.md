# Issue 30: External URL Tracking and Content Hashing

**Priority**: MEDIUM
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 29 (Static Asset Tracking - Core)
**Blocks**: Issue 31 (Watch Service Integration)

## Summary

Track external URLs as first-class BeliefNodes with content-addressed BIDs based on fetched content, enabling change detection, broken link detection, and offline caching. This extends the asset tracking pattern (Issue 29) to remote resources.

## Goals

1. **Generate content-addressed BIDs** for external URLs using `buildonomy_href_bid(content_hash)`
2. **Fetch URL content** and compute SHA256 hash of response text (not binary)
3. **Track URLs as BeliefNodes** in belief graph (already partially implemented)
4. **Detect content changes** when URLs are refetched (BID changes trigger document reparse)
5. **Create URL manifests** as peer files to BeliefNodes (parallel to asset manifests)
6. **Enable bidirectional queries**: "What documents link to this URL?" and "What URLs does this document reference?"
7. **Respect user privacy**: Fetching controlled by explicit opt-in flag

## Architecture

### Parallel to Static Assets

External URLs use the **same architectural pattern** as static assets:

```
Repository (Source of Truth)
├── docs/
│   ├── research/
│   │   ├── paper.md                  (references https://example.com/spec.html)
│   │   ├── paper.toml                (BeliefNode metadata)
│   │   └── .url-manifest.toml        (NEW: URL tracking)

BeliefBase
├── Document Node (bid:doc-abc...)
│   └─[Epistemic]→ URL Node (bid:href-ns:sha256-def...)
│       ├── kind: External
│       ├── id: "sha256-def456"
│       └── payload: { url, content_hash, fetched_at, status }
```

### Key Differences from Static Assets

| Aspect | Static Assets (Issue 29) | External URLs (Issue 30) |
|--------|-------------------------|--------------------------|
| **Namespace** | `UUID_NAMESPACE_ASSET` | `UUID_NAMESPACE_HREF` (already exists) |
| **BID Input** | `sha256(file_bytes)` | `sha256(GET(url).text)` |
| **Content** | File on disk | HTTP GET response body |
| **Hash Target** | Binary bytes | Text content (not headers/metadata) |
| **Privacy** | Always tracked | Opt-in via `--fetch-urls` flag |
| **Offline** | Always available | May fail (network required) |
| **Manifest** | `.asset-manifest.toml` | `.url-manifest.toml` |

### URL Manifest Structure

Written as `.url-manifest.{toml|json|yaml}` peer to each `BeliefNode.{toml|json|yaml}`:

```toml
version = "0.1"
network_bid = "bid:doc-abc123..."

[[urls]]
bid = "bid:href-ns:sha256-def456"
url = "https://example.com/spec.html"
content_hash = "def456789..."  # SHA256 of response text
status_code = 200
fetched_at = "2026-01-29T12:00:00Z"
content_type = "text/html"
link_text = "Official Specification"
last_modified = "2026-01-15T08:00:00Z"  # From HTTP header if available

[[urls]]
bid = "bid:href-ns:sha256-abc123"
url = "https://broken-url.com/missing"
content_hash = null  # Fetch failed
status_code = 404
fetched_at = "2026-01-29T12:00:00Z"
link_text = "Broken Link"
```

### BID Generation

```rust
/// Generate content-addressed BID for external URLs
/// Input: SHA256 hash of fetched content (text only)
/// Output: Deterministic BID in UUID_NAMESPACE_HREF
pub fn buildonomy_href_bid(hash_str: &str) -> Bid {
    let uuid = Uuid::new_v5(&UUID_NAMESPACE_HREF, hash_str.as_bytes());
    let mut bytes = *uuid.as_bytes();
    bytes[10..16].copy_from_slice(&Bid::from(UUID_NAMESPACE_HREF).parent_namespace_bytes());
    Bid(Uuid::from_bytes(bytes))
}
```

**Why hash text not binary?**
- Many URLs return different metadata/headers on each request
- Text content is what matters for "has the spec changed?"
- Ignores cache headers, ETags, cookies, etc.
- Focus on semantic content, not HTTP artifacts

## Implementation Steps

### 1. Add URL Content Fetching (1 day)

Create `src/codec/url_fetch.rs`:

```rust
pub struct UrlFetcher {
    client: reqwest::Client,
    fetch_enabled: bool,
}

impl UrlFetcher {
    pub fn new(fetch_enabled: bool) -> Self {
        UrlFetcher {
            client: reqwest::Client::builder()
                .user_agent(format!("noet/{}", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
            fetch_enabled,
        }
    }

    pub async fn fetch_and_hash(&self, url: &str) -> Result<UrlFetchResult> {
        if !self.fetch_enabled {
            return Ok(UrlFetchResult::Disabled);
        }

        let response = self.client.get(url).send().await?;
        let status = response.status();
        let content_type = response.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let last_modified = response.headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        if !status.is_success() {
            return Ok(UrlFetchResult::Error {
                status: status.as_u16(),
                message: format!("HTTP {}", status),
            });
        }

        let text = response.text().await?;
        let hash = compute_sha256(&text);

        Ok(UrlFetchResult::Success {
            content_hash: hash,
            status_code: status.as_u16(),
            content_type,
            last_modified,
            fetched_at: chrono::Utc::now(),
        })
    }
}
```

### 2. Extend DocumentCompiler (0.5 days)

Add to `compiler.rs`:

```rust
pub struct DocumentCompiler {
    // ... existing fields
    url_fetcher: UrlFetcher,
    url_manifests: HashMap<Bid, UrlManifest>,
}

impl DocumentCompiler {
    pub fn with_url_fetching(mut self, enabled: bool) -> Self {
        self.url_fetcher = UrlFetcher::new(enabled);
        self
    }
}
```

### 3. URL Processing During Parse (1 day)

Similar to asset processing (Issue 29 Step 3):

1. Extract URL references from `UnresolvedReference`s (where `net == href_namespace()`)
2. For each URL:
   - Fetch content via `url_fetcher.fetch_and_hash(url)`
   - Generate `buildonomy_href_bid(content_hash)`
   - Create `UrlEntry` with status/metadata
3. Build `UrlManifest` for the document
4. Store in `url_manifests` HashMap

### 4. Create BeliefNodes for URLs (0.5 days)

Already partially implemented in `builder.rs::push_relation()`. Extend to:
- Use content-addressed BID if fetch succeeded
- Store fetch metadata in payload
- Create relation with URL in weight

### 5. Write URL Manifests (0.5 days)

After `parse_all()` completes:

```rust
async fn write_url_manifests(&self) -> Result<()> {
    for (network_bid, manifest) in &self.url_manifests {
        let network_path = self.cache().get_path_for_bid(network_bid)?;
        let manifest_path = network_path.with_file_name(
            format!(".url-manifest.{}", network_path.extension()?)
        );
        fs::write(manifest_path, serialize_manifest(manifest)?).await?;
    }
    Ok(())
}
```

## Testing Requirements

### Unit Tests
- `test_url_fetch_and_hash()` - Verify HTTP GET and SHA256 computation
- `test_url_fetch_disabled()` - Verify no network requests when disabled
- `test_url_manifest_serialization()` - Roundtrip TOML/JSON/YAML
- `test_404_handling()` - Broken links tracked with status

### Integration Tests
- `test_url_tracking_in_network()` - Parse document with external links
- `test_url_belief_nodes()` - Verify URLs in belief graph
- `test_url_content_change_detection()` - Modify URL content → BID changes
- `test_privacy_opt_in()` - Default behavior doesn't fetch

### Manual Testing
1. Document with external links, compile with `--fetch-urls` flag
2. Verify `.url-manifest.toml` created with status codes
3. Modify URL content (mock server) → verify BID change triggers reparse
4. Test 404 links tracked with error status

## Success Criteria

- [ ] URL fetching controlled by explicit opt-in flag
- [ ] URL manifests written as peer files to BeliefNode metadata
- [ ] URLs tracked as BeliefNodes in belief graph
- [ ] Content-addressed BIDs enable change detection
- [ ] Broken links (404, timeout) tracked with error status
- [ ] Bidirectional queries work: doc→urls and url→docs
- [ ] All tests passing
- [ ] No network requests without user consent

## Risks

### Risk 1: Privacy Concerns
**Impact**: HIGH  
**Likelihood**: LOW (with proper design)  
**Mitigation**: Default behavior does NOT fetch. Requires explicit `--fetch-urls` or `fetch_external_urls = true` in config. Document clearly in CLI help.

### Risk 2: Network Failures
**Impact**: MEDIUM  
**Likelihood**: HIGH  
**Mitigation**: Graceful degradation. Failed fetches create nodes with error status. Compilation continues. User can retry later.

### Risk 3: Performance with Many URLs
**Impact**: MEDIUM  
**Likelihood**: MEDIUM  
**Mitigation**: Fetch in parallel with bounded concurrency (e.g., 10 concurrent requests). Add timeout. Cache results between sessions.

## Open Questions

### Q1: Cache fetched content?
**Options**:
- A) Refetch on every compilation (slow, always fresh)
- B) Cache with TTL (fast, may be stale)
- C) Only refetch if `--force-refetch` flag (manual control)

**Recommendation**: Start with A (simple), add B in future enhancement.

### Q2: Follow redirects?
**Decision needed**: Should 301/302 redirects be followed? Store final URL or original?

### Q3: Authenticate requests?
**Future enhancement**: Support `Authorization` headers for private APIs. Not in scope for initial implementation.

## References

### Related Issues
- **Issue 29**: Static Asset Tracking (parallel architecture pattern)
- **Issue 31**: Watch Service Integration (triggered by URL changes)
- **Issue 4**: Link Manipulation (existing URL reference handling)

### Architecture References
- `properties.rs:href_namespace()` - Existing URL namespace
- `builder.rs:push_relation()` - External node creation for URLs
- `md.rs:LinkAccumulator` - URL detection during parse

### Future Enhancements
- **Offline Archive**: Save fetched content for offline browsing
- **Link Rot Detection**: Periodic refetch to find broken links
- **API Authentication**: Support for private/authenticated URLs
- **Content Caching**: TTL-based cache to reduce network requests

## Notes

**Why Content-Hash BIDs**: Same as Issue 29 rationale. When URL content changes, BID changes, triggering markdown rewrite. Keeps belief graph synchronized with external world.

**Privacy First**: No network requests without explicit user consent. This is critical for trust and GDPR compliance.

**Broken Link Philosophy**: Track broken links (404, timeout) as nodes with error status. Don't fail compilation. Enable queries like "show all broken links in repository."

**Relation to Issue 11 (LSP)**: URL fetching enables rich diagnostics: "This link returned 404", "Content changed since last fetch", etc. LSP can surface these warnings in editor.