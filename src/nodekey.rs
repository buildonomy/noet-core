/// [crate::nodekey] contains NodeKey and the link markup parsing for converting links into
/// [crate::beliefbase::BeliefBase] [crate::properties::BeliefNode] references.
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};
use toml::{from_str, to_string};
pub use url::Url;

uniffi::custom_type!(Url, String, {
    remote,
    try_lift: |val| Ok(Url::parse(&val)?),
    lower: |obj| format!("{}", obj)
});

use crate::{
    beliefbase::BeliefBase,
    codec::CODECS,
    paths::{to_anchor, AnchorPath, AnchorPathBuf},
    properties::{asset_namespace, content_namespaces, href_namespace, Bid, Bref},
    query::{BeliefSource, Expression, StatePred},
    BuildonomyError,
};

/// Used to specify the join logic between two (sets of) BeliefNodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Bid { bid: Bid },
    Bref { bref: Bref },
    Path { net: Bref, path: String },
    Id { net: Bref, id: String },
}

/// Identifies the URL scheme used in a NodeKey string representation.
///
/// Known schemes (`Bid`, `Bref`, `Id`, `Path`) map to NodeKey variants.
/// `None` means no scheme was present (bare string). `External` means
/// an unrecognized scheme (e.g. `https:`, `mailto:`), which gets routed
/// to `NodeKey::Path` with `href_namespace`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NodeKeyScheme {
    Bid,
    Bref,
    #[default]
    Path,
    Id,
    /// No scheme present — bare string (e.g. `docs/file.md`, `My Title`, `#section`)
    None,
    /// Unrecognized scheme — external URL (e.g. `https:`, `mailto:`, `tel:`)
    External,
}

impl From<&str> for NodeKeyScheme {
    fn from(scheme_str: &str) -> Self {
        match scheme_str.to_lowercase().trim() {
            "" => NodeKeyScheme::None,
            "bid" => NodeKeyScheme::Bid,
            "bref" => NodeKeyScheme::Bref,
            "id" => NodeKeyScheme::Id,
            "path" => NodeKeyScheme::Path,
            _ => NodeKeyScheme::External,
        }
    }
}

uniffi::custom_type!(NodeKey, String, {
    try_lift: |val| Ok(from_str(&val)?),
    lower: |obj| to_string(&obj).expect("NodeKeys should serialize without error")
});

impl NodeKey {
    /// Parse the network (authority) and value from a schemed URL using AnchorPath.
    ///
    /// For **hierarchical** URLs (`scheme://authority/path`), the authority must be a
    /// valid Bid or Bref (the explicit network). If it's not, returns
    /// [`BuildonomyError::UnresolvedNetwork`]. If the authority is valid but there's no
    /// resource path after it, the authority is treated as the value (backward compat
    /// for `bid://VALUE`, `id://supremum`).
    ///
    /// For **non-hierarchical** URLs (`scheme:value`), everything after the colon is the
    /// value with no network (implicit, filled by [`regularize_unchecked`]).
    ///
    /// Returns an error if the resulting value is empty after parsing.
    fn parse_network_and_value(
        s: &str,
        ap: &AnchorPath<'_>,
        scheme: &NodeKeyScheme,
    ) -> Result<(Bref, String), BuildonomyError> {
        let is_hierarchical = ap.has_hostname();
        let hostname = ap.hostname();

        let (net, value) = if is_hierarchical {
            // resource() returns path + params + anchor after hostname.
            // For hierarchical URLs this includes a leading '/' which we strip.
            let resource = ap.resource().trim_start_matches('/');

            if hostname.is_empty() {
                // Empty authority (e.g. scheme:///value) — no explicit network
                (Bid::nil().bref(), resource.to_string())
            } else {
                // Try parsing authority as Bid/Bref
                let parsed_net = Bid::try_from(hostname)
                    .ok()
                    .map(|bid| bid.bref())
                    .or_else(|| Bref::try_from(hostname).ok());

                match (parsed_net, resource.is_empty()) {
                    (Some(net), false) => {
                        // Valid Bid/Bref authority AND a value after it → authority is
                        // the explicit network, rest is the value.
                        (net, resource.to_string())
                    }
                    (_, true) => {
                        // No value after authority — authority IS the value, not the
                        // network. Covers backward compat (bid://VALUE, id://supremum)
                        // and the case where authority is not a valid Bid/Bref but also
                        // has no path (id://some-name).
                        (Bid::nil().bref(), hostname.to_string())
                    }
                    (None, false) => {
                        // Authority is not a valid Bid/Bref and there IS a value after
                        // it — this is an unresolved network reference.
                        return Err(BuildonomyError::UnresolvedNetwork {
                            network_ref: hostname.to_string(),
                            key_type: match scheme {
                                NodeKeyScheme::Bid => "bid",
                                NodeKeyScheme::Bref => "bref",
                                NodeKeyScheme::Id => "id",
                                NodeKeyScheme::Path => "path",
                                _ => "unknown",
                            }
                            .to_string(),
                            value: resource.to_string(),
                        });
                    }
                }
            }
        } else {
            // Non-hierarchical (scheme:value) — no network, everything after : is value
            let value = ap.path_after_schema();
            (Bid::nil().bref(), value.to_string())
        };

        if value.is_empty() {
            return Err(BuildonomyError::Serialization(format!(
                "[Nodekey] cannot generate a nodekey from an empty string. \
                After parsing '{s}' for schema and network, remaining path is empty",
            )));
        }

        Ok((net, value))
    }

    /// Resolve a relative path `NodeKey` against a document's repo-relative path.
    ///
    /// Joins the link path against `doc_path` (using [`AnchorPath::join`]) and normalizes
    /// the result. The original `net` is preserved — this does path resolution only,
    /// without assigning a network.
    ///
    /// - `href_namespace` paths (external URLs) are returned unchanged.
    /// - Non-`Path` variants (`Id`, `Bid`, `Bref`) are returned unchanged.
    /// - If the resolved path escapes the repo boundary (starts with `../`), a warning
    ///   is logged and the normalized-but-un-joined path is returned.
    pub fn resolve_against(&self, doc_path: &str) -> NodeKey {
        match self {
            NodeKey::Path { ref net, ref path } => {
                if *net == href_namespace().bref() {
                    return self.clone();
                }
                let doc_ap = AnchorPath::from(doc_path);
                let normalized_link: AnchorPathBuf = AnchorPath::from(path.as_str()).normalize();
                let resolved: AnchorPathBuf = doc_ap.join(&normalized_link);
                if resolved.starts_with("../") {
                    tracing::warn!(
                        "[NodeKey::resolve_against] Resolved path '{}' escapes repo boundary \
                         (from link '{}' in '{}'). Returning normalized but un-joined path.",
                        resolved,
                        path,
                        doc_path
                    );
                    return NodeKey::Path {
                        net: *net,
                        path: normalized_link.into_string(),
                    };
                }
                NodeKey::Path {
                    net: *net,
                    path: resolved.into_string(),
                }
            }
            _ => self.clone(),
        }
    }

    /// Regularize relative references without cache lookup (unchecked).
    ///
    /// This is a lower-level API that doesn't perform security boundary checks.
    /// Prefer `regularize()` when possible.
    #[tracing::instrument(skip(self))]
    pub fn regularize_unchecked(&self, base_net: Bid, owner_path: &str) -> NodeKey {
        let base_ap = AnchorPath::from(owner_path);
        match self {
            NodeKey::Path {
                net: link_base,
                path: link_path,
            } => {
                let normalized_link_buf = AnchorPath::from(link_path).normalize();
                if *link_base == href_namespace().bref() {
                    // href_namespace paths are external URLs — normalize the path
                    // component (resolve ../ etc.) but treat as absolute. Never join
                    // with owner_path since they are outside the parsed repo.
                    return NodeKey::Path {
                        net: *link_base,
                        path: normalized_link_buf.into(),
                    };
                }
                // Asset-namespace paths need the owner join to resolve relative
                // references (e.g. ../assets/img.png), but keep their original net.
                // Other non-default nets (explicit network references) are already
                // absolute within their network — just normalize.
                let needs_owner_join =
                    link_base.is_default() || *link_base == asset_namespace().bref();
                if !needs_owner_join {
                    return NodeKey::Path {
                        net: *link_base,
                        path: normalized_link_buf.into(),
                    };
                }
                if normalized_link_buf.as_anchor_path().is_absolute() {
                    tracing::warn!(
                        "[Nodekey::regularize] NodeKey::Path supplied with an \
                        absolute path {}, but without an anchoring network. The meaning of \
                        the path is unclear. Assuming this is rooted at the root dir of \
                        the supplied network {}",
                        normalized_link_buf,
                        base_net
                    );
                    return NodeKey::Path {
                        net: base_net.bref(),
                        path: normalized_link_buf.into(),
                    };
                }

                let join_path = base_ap.join(&normalized_link_buf);
                if join_path.starts_with("../") {
                    tracing::warn!(
                        "[NodeKey::regularize] The normalized and regularized path \
                        exceeds the supplied relative path boundary. This may \
                        result in unexpected behavior. Initial path: {}, relative \
                        to path: {}",
                        link_path,
                        join_path,
                    );
                }

                // Preserve the original net for asset paths; assign base_net for
                // default-net paths (codec documents like .md).
                let resolved_net = if link_base.is_default() {
                    base_net.bref()
                } else {
                    *link_base
                };
                NodeKey::Path {
                    net: resolved_net,
                    path: join_path.into(),
                }
            }
            NodeKey::Id { net, id } => {
                if net.is_default() {
                    NodeKey::Id {
                        net: base_net.bref(),
                        id: id.clone(),
                    }
                } else {
                    self.clone()
                }
            }
            _ => self.clone(),
        }
    }

    /// Regularize relative references to absolute within network context (sync).
    ///
    /// Converts relative paths, titles, and IDs to absolute references using the
    /// [crate::beliefbase::BeliefBase]. Paths are bounded by their home network's document path for
    /// security.
    #[tracing::instrument(skip(self, cache))]
    pub fn regularize(
        &self,
        cache: &BeliefBase,
        key_owner: Bid,
        root_net: Bid,
    ) -> Result<NodeKey, BuildonomyError> {
        // Get network and path from the relative_to node
        let (_home_net, owner_path) = cache
            .paths()
            .get_map(&root_net.bref())
            .and_then(|pm| pm.path(&key_owner, &cache.paths()))
            .map(|(home_net, rooted_path, _order)| (home_net, rooted_path))
            .or_else(|| cache.paths().path(&key_owner))
            .ok_or_else(|| {
                BuildonomyError::NotFound(format!(
                    "Could not determine home network/path for nodekey owner {key_owner}"
                ))
            })?;
        // It's ok to put base_rooted_path in regardless of what rel_net, because we only use
        // that argument in the case that self == NodeKey::Path
        Ok(self.regularize_unchecked(root_net, &owner_path))
    }

    /// Regularize relative references to absolute within network context (async).
    ///
    /// Converts relative paths, titles, and IDs to absolute references using the BeliefSource.
    /// Paths are bounded by their home network's document path for security.
    pub async fn regularize_async<C: BeliefSource>(
        &self,
        cache: &C,
        key_owner: Bid,
        root_net: Bid,
    ) -> Result<NodeKey, BuildonomyError> {
        // Query for the relative_to node to get its path
        let keys = vec![key_owner, root_net];
        let query_expr = Expression::StateIn(StatePred::Bid(keys));
        let cache = BeliefBase::from(cache.eval(&query_expr).await?);
        self.regularize(&cache, key_owner, root_net)
    }

    /// Parse a string into a NodeKey with network resolution using a [crate::beliefbase::BeliefBase] (sync).
    ///
    /// This handles UnresolvedNetwork errors by querying the cache for the network reference.
    pub fn from_str_with_cache(s: &str, cache: &BeliefBase) -> Result<Self, BuildonomyError> {
        match s.parse::<NodeKey>() {
            Ok(key) => Ok(key),
            Err(BuildonomyError::UnresolvedNetwork {
                network_ref,
                key_type,
                value,
            }) => {
                // Try to resolve the network reference
                let net = Self::resolve_network_sync(&network_ref, cache)?.bref();

                // Construct the appropriate NodeKey with resolved network
                match key_type.as_str() {
                    "id" => Ok(NodeKey::Id { net, id: value }),
                    "path" => Ok(NodeKey::Path { net, path: value }),
                    _ => Err(BuildonomyError::Serialization(format!(
                        "Unknown key type: {key_type}"
                    ))),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Parse a string into a NodeKey with network resolution using a BeliefSource (async).
    pub async fn from_str_with_cache_async<C: BeliefSource>(
        s: &str,
        cache: &C,
    ) -> Result<Self, BuildonomyError> {
        match s.parse::<NodeKey>() {
            Ok(key) => Ok(key),
            Err(BuildonomyError::UnresolvedNetwork {
                network_ref,
                key_type,
                value,
            }) => {
                // Try to resolve the network reference
                let net = Self::resolve_network_async(&network_ref, cache)
                    .await?
                    .bref();

                // Construct the appropriate NodeKey with resolved network
                match key_type.as_str() {
                    "id" => Ok(NodeKey::Id { net, id: value }),
                    "path" => Ok(NodeKey::Path { net, path: value }),
                    _ => Err(BuildonomyError::Serialization(format!(
                        "Unknown key type: {key_type}"
                    ))),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Resolve a network reference string to a BID using a [crate::beliefbase::BeliefBase] (sync).
    fn resolve_network_sync(network_ref: &str, cache: &BeliefBase) -> Result<Bid, BuildonomyError> {
        // Try parsing as Bref first
        if let Ok(bref) = Bref::try_from(network_ref) {
            // Search states for a node with this namespace
            for node in cache.states().values() {
                if node.bid.bref() == bref {
                    return Ok(node.bid);
                }
            }
        }

        // Try finding by ID
        for node in cache.states().values() {
            if node.id.as_deref() == Some(network_ref) {
                return Ok(node.bid);
            }
        }

        Err(BuildonomyError::NotFound(format!(
            "Network reference '{network_ref}' not found in cache"
        )))
    }

    /// Resolve a network reference string to a BID using a BeliefSource (async).
    async fn resolve_network_async<C: BeliefSource>(
        network_ref: &str,
        cache: &C,
    ) -> Result<Bid, BuildonomyError> {
        // Try parsing as Bref first
        if let Ok(bref) = Bref::try_from(network_ref) {
            // Query for nodes with this bref
            let query_expr = Expression::StateIn(StatePred::Bref(vec![bref]));
            let result = cache.eval_unbalanced(&query_expr).await?;

            if let Some(node) = result.states.values().next() {
                return Ok(node.bid);
            }
        }

        Err(BuildonomyError::NotFound(format!(
            "Network reference '{network_ref}' not found in cache"
        )))
    }

    /// Generate a [Url] object for this [Bid] using the 'bid://' schema.
    /// Generate a [Url] object for this NodeKey.
    ///
    /// Uses hierarchical form (`scheme://authority/path`) only when an explicit network
    /// is present. Otherwise uses non-hierarchical form (`scheme:value`).
    ///
    /// Note: The `url` crate requires `://` for hierarchical URLs, so `as_url()` always
    /// includes the authority component (using `Bid::nil()` as the default network).
    /// For canonical string representation, use `Display` instead.
    pub fn as_url(&self) -> Url {
        match self {
            NodeKey::Bid { bid } => {
                Url::parse(&format!("bid://{bid}")).expect("Url format explicitly specified.")
            }
            NodeKey::Bref { bref } => {
                Url::parse(&format!("bref://{bref}")).expect("Url format explicitly specified.")
            }
            NodeKey::Id { net, id } => {
                Url::parse(&format!("id://{net}/{id}")).expect("Url format explicitly specified.")
            }
            NodeKey::Path { net, path } => Url::parse(&format!("path://{net}/{path}"))
                .expect("Url format explicitly specified."),
        }
    }
}

/// Display uses canonical NodeKey URL syntax:
///
/// - `bid:VALUE` / `bref:VALUE` — always non-hierarchical (globally unique, no network)
/// - `id:value` / `path:value` — non-hierarchical when network is implicit (default)
/// - `id://NETWORK/value` / `path://NETWORK/value` — hierarchical when network is explicit
/// - Content namespace paths (href, asset) — emitted as raw path strings so that
///   `Display` → `FromStr` roundtrips correctly (external URLs re-parse as `External`
///   scheme, asset paths re-parse as bare strings with `asset_namespace`).
impl Display for NodeKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeKey::Bid { bid } => {
                write!(f, "bid:{bid}")
            }
            NodeKey::Bref { bref } => {
                write!(f, "bref:{bref}")
            }
            NodeKey::Id { net, id } => {
                if !net.is_default() {
                    write!(f, "id://{net}/{id}")
                } else {
                    write!(f, "id:{id}")
                }
            }
            NodeKey::Path { net, path } => {
                if content_namespaces().iter().any(|ns| *net == ns.bref()) {
                    // Content namespace path (href or asset) — emit as bare path for
                    // clean roundtripping. External URLs will be re-parsed as External
                    // scheme, asset paths will be re-parsed as bare Path with asset_namespace.
                    write!(f, "{path}")
                } else if !net.is_default() {
                    write!(f, "path://{net}/{path}")
                } else {
                    write!(f, "path:{path}")
                }
            }
        }
    }
}

impl FromStr for NodeKey {
    type Err = BuildonomyError;

    /// Parse a string into a NodeKey using URL-based format.
    ///
    /// # URL Semantics
    ///
    /// NodeKey URLs follow standard URI authority conventions:
    ///
    /// - **Hierarchical** (`scheme://authority/path`): The authority component is an
    ///   **explicit network** and MUST be a valid Bid or Bref. If the authority is not
    ///   a valid Bid/Bref, parsing returns an [`BuildonomyError::UnresolvedNetwork`] error
    ///   (future: DNS-style resolution could resolve arbitrary hostnames to networks).
    ///
    /// - **Non-hierarchical** (`scheme:value`): No authority/network component. The network
    ///   is implicit (filled in later by [`NodeKey::regularize_unchecked`]).
    ///
    /// - **Bare strings** (no scheme): Heuristic parsing — first path component is probed
    ///   as Bid/Bref; if it matches, it becomes the network. Otherwise, the full string
    ///   is treated as a path or identifier.
    ///
    /// See `tests::test_nodekey_parse_comprehensive` for detailed examples.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(BuildonomyError::Serialization(
                "[Nodekey] cannot generate a nodekey from an empty string.".to_string(),
            ));
        }

        let ap = AnchorPath::new(s);
        let scheme = NodeKeyScheme::from(ap.schema());

        match scheme {
            // --- External URLs ---
            // Unrecognized scheme (https:, mailto:, etc.) — the entire original string
            // is stored as a Path under the href_namespace. URLs are locations (paths),
            // not identifiers — storing as Path avoids destructive to_anchor() slugification.
            NodeKeyScheme::External => {
                tracing::debug!(
                    "Found External URL '{s}' - treating as NodeKey::Path with href_namespace"
                );
                Ok(NodeKey::Path {
                    net: href_namespace().bref(),
                    path: s.to_string(),
                })
            }

            // --- Bare strings (no scheme) ---
            // Heuristic: try whole-string Bid/Bref, then plain-text Id, then path-like
            // with optional Bid/Bref network in the first component.
            NodeKeyScheme::None => {
                // Whole-string Bid or Bref?
                if let Ok(bid) = Bid::try_from(s) {
                    return Ok(NodeKey::Bid { bid });
                }
                if let Ok(bref) = Bref::try_from(s) {
                    return Ok(NodeKey::Bref { bref });
                }
                // No path-like indicators (/, #, .) → plain-text Id
                let has_path_indicators = s.contains(['/', '#', '.']);
                if !has_path_indicators {
                    return Ok(NodeKey::Id {
                        net: Bref::default(),
                        id: to_anchor(s),
                    });
                }

                // Path-like bare string — probe first component for Bid/Bref network
                let remainder = ap.path_after_schema(); // == s for bare strings
                let first_slash = remainder.find('/');
                let (candidate, rest) = match first_slash {
                    Some(pos) => (&remainder[..pos], &remainder[pos + 1..]),
                    None => (remainder, ""),
                };

                let (net, value_str) = if !rest.is_empty() {
                    match Bid::try_from(candidate)
                        .ok()
                        .map(|bid| bid.bref())
                        .or_else(|| Bref::try_from(candidate).ok())
                    {
                        Some(parsed_net) => (parsed_net, rest),
                        None => (Bid::nil().bref(), remainder),
                    }
                } else {
                    (Bid::nil().bref(), remainder)
                };

                let norm_input = value_str.strip_prefix('/').unwrap_or(value_str);
                let norm_ap = AnchorPath::new(norm_input);
                let norm_path: String = norm_ap.normalize().into();
                if norm_path.is_empty() {
                    return Err(BuildonomyError::Serialization(format!(
                        "[Nodekey] cannot generate a nodekey from an empty string. \
                        After parsing '{s}' for network, remaining path is empty",
                    )));
                }
                let mut path_net = net;
                if CODECS.get(&norm_ap).is_none() {
                    path_net = asset_namespace().bref();
                }
                Ok(NodeKey::Path {
                    net: path_net,
                    path: norm_path,
                })
            }

            // --- Known NodeKey schemes ---
            // Parse network from authority (hierarchical) or use implicit (non-hierarchical),
            // then dispatch by scheme to construct the appropriate NodeKey variant.
            NodeKeyScheme::Bid => {
                let (_net, value_str) = Self::parse_network_and_value(s, &ap, &scheme)?;
                let bid = Bid::try_from(value_str.as_str())?;
                Ok(NodeKey::Bid { bid })
            }
            NodeKeyScheme::Bref => {
                let (_net, value_str) = Self::parse_network_and_value(s, &ap, &scheme)?;
                let bref = Bref::try_from(value_str.as_str())?;
                Ok(NodeKey::Bref { bref })
            }
            NodeKeyScheme::Id => {
                let (net, value_str) = Self::parse_network_and_value(s, &ap, &scheme)?;
                let id = to_anchor(&value_str);
                Ok(NodeKey::Id { net, id })
            }
            NodeKeyScheme::Path => {
                let (net, value_str) = Self::parse_network_and_value(s, &ap, &scheme)?;
                let mut path_net = net;
                let norm_input = value_str.strip_prefix('/').unwrap_or(&value_str);
                let norm_ap = AnchorPath::new(norm_input);
                let norm_path: String = norm_ap.normalize().into();
                if CODECS.get(&norm_ap).is_none() {
                    path_net = asset_namespace().bref();
                }
                Ok(NodeKey::Path {
                    net: path_net,
                    path: norm_path,
                })
            }
        }
    }
}

/// When parsing links, the net should be chosen based on how well the link str lines up with the
/// stack. If it starts_with, it should chose the most specific of the start_with set. If it
/// contains relative back-links, back link through the stack to find the proper path network, and
/// then canonicalize the link path from there.
pub fn href_to_nodekey(link: &str) -> NodeKey {
    link.parse().unwrap_or_else(|_| NodeKey::Id {
        net: Bid::nil().bref(),
        id: link.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn test_to_anchor() {
        assert_eq!(to_anchor("Hello World"), "hello-world");
        assert_eq!(to_anchor("  leading spaces"), "leading-spaces");
        assert_eq!(to_anchor("trailing spaces  "), "trailing-spaces");
        assert_eq!(to_anchor("CAPITALS"), "capitals");

        // Test punctuation removal for HTML/URL compatibility
        assert_eq!(to_anchor("API & Reference"), "api--reference");
        assert_eq!(to_anchor("Section 2.1: Overview"), "section-21-overview");
        assert_eq!(to_anchor("Step 1: Install"), "step-1-install");
        assert_eq!(to_anchor("What's this?"), "whats-this");
        assert_eq!(to_anchor("Hello, World!"), "hello-world");
    }

    #[test]
    fn test_url_ergonomic_parsing() {
        // Non-hierarchical form: id:value (implicit network)
        let key: NodeKey = "id:supremum".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bref::default() && id == "supremum"));

        // Hierarchical form: id://network/value (explicit network)
        let network_bid = Bid::new(Bid::nil()).bref();
        let key: NodeKey = format!("id://{network_bid}/supremum").parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == network_bid && id == "supremum"));

        // Non-hierarchical path: path:docs/README.md (implicit network)
        let result: Result<NodeKey, _> = "path:docs/README.md".parse();
        assert_eq!(
            result,
            Ok(NodeKey::Path {
                net: Bref::default(),
                path: "docs/README.md".to_string()
            })
        );

        // Hierarchical path with non-Bid/Bref authority → UnresolvedNetwork error
        let result: Result<NodeKey, _> = "path://docs/README.md".parse();
        assert!(matches!(
            result,
            Err(BuildonomyError::UnresolvedNetwork { .. })
        ));
    }

    #[test]
    fn test_nodekey_url_parsing() {
        // Test URL-based explicit formats
        let network_bid = Bid::new(Bid::nil());
        let network_bref = network_bid.bref();
        let test_bid = Bid::new(Bid::nil());
        let test_bref = test_bid.bref();

        // BID non-hierarchical (canonical form)
        let key: NodeKey = format!("bid:{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // BID hierarchical with network (network silently ignored, BIDs are global)
        let key: NodeKey = format!("bid://{network_bid}/{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // BID hierarchical backward compat (bid://VALUE with no path → VALUE is the bid)
        let key: NodeKey = format!("bid://{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // Bref non-hierarchical (canonical form)
        let key: NodeKey = format!("bref:{test_bref}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // Bref hierarchical with network (network silently ignored)
        let key: NodeKey = format!("bref://{network_bid}/{test_bref}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // Bref hierarchical backward compat (bref://VALUE)
        let key: NodeKey = format!("bref://{test_bref}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // ID hierarchical with explicit network
        let key: NodeKey = format!("id://{network_bid}/supremum").parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == network_bref && id == "supremum"));

        // ID non-hierarchical (implicit network)
        let key: NodeKey = "id:supremum".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bref::default() && id == "supremum"));

        // ID hierarchical with empty authority (defaults to nil)
        let key: NodeKey = "id:///supremum".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bref::default() && id == "supremum"));

        // Path hierarchical with explicit network
        let key: NodeKey = format!("path://{network_bid}/docs/council/README.md")
            .parse()
            .unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == network_bref && path == "docs/council/README.md"));

        // Path non-hierarchical (implicit network)
        let key: NodeKey = "path:docs/council/README.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "docs/council/README.md"));

        // External URLs
        let key: NodeKey = "https://example.com/page".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == href_namespace().bref() && path == "https://example.com/page"));

        // External URL roundtrip: Display emits raw URL, FromStr re-parses as External → Path
        let key: NodeKey = "https://google.com/search?q=test#results".parse().unwrap();
        let serialized = key.to_string();
        assert_eq!(serialized, "https://google.com/search?q=test#results");
        let reparsed: NodeKey = serialized.parse().unwrap();
        assert_eq!(key, reparsed);

        // Simple external URL roundtrip
        let key: NodeKey = "https://google.com".parse().unwrap();
        let serialized = key.to_string();
        assert_eq!(serialized, "https://google.com");
        let reparsed: NodeKey = serialized.parse().unwrap();
        assert_eq!(key, reparsed);

        // Mailto roundtrip
        let key: NodeKey = "mailto:user@example.com".parse().unwrap();
        let serialized = key.to_string();
        assert_eq!(serialized, "mailto:user@example.com");
        let reparsed: NodeKey = serialized.parse().unwrap();
        assert_eq!(key, reparsed);
    }

    #[test]
    fn test_unresolved_network_error() {
        // Hierarchical with invalid authority → UnresolvedNetwork error
        let result: Result<NodeKey, _> = "id://my-network-id/supremum".parse();
        assert!(matches!(result, Err(BuildonomyError::UnresolvedNetwork {
        network_ref, key_type, value
    }) if network_ref == "my-network-id" && key_type == "id" && value == "supremum"));

        // Hierarchical with valid BID authority should work fine
        let network_bid = Bid::new(Bid::nil());
        let result: Result<NodeKey, _> = format!("id://{network_bid}/supremum").parse();
        assert!(result.is_ok());

        // Non-hierarchical form (no network) should work fine
        let result: Result<NodeKey, _> = "id:supremum".parse();
        assert!(result.is_ok());

        // path:// with non-Bid/Bref authority → UnresolvedNetwork error
        let result: Result<NodeKey, _> = "path://docs/README.md".parse();
        assert!(matches!(result, Err(BuildonomyError::UnresolvedNetwork {
            network_ref, key_type, value
        }) if network_ref == "docs" && key_type == "path" && value == "README.md"));

        // bref:// with corrupted authority → UnresolvedNetwork error
        let result: Result<NodeKey, _> = "bref://not-a-bref/something".parse();
        assert!(matches!(
            result,
            Err(BuildonomyError::UnresolvedNetwork { .. })
        ));
    }

    #[test]
    fn test_nodekey_backward_compatibility() {
        let test_bid = Bid::new(Bid::nil());
        let test_bref = test_bid.bref();

        // Bare BID string
        let key: NodeKey = test_bid.to_string().parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // Bare Bref string
        let key: NodeKey = test_bref.to_string().parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // Relative paths
        let key: NodeKey = "./README.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "README.md"));

        let key: NodeKey = "../docs/file.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "../docs/file.md"));

        // Anchors
        let key: NodeKey = "#section".parse().unwrap();
        let key_clone = key.clone();
        assert!(
            matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "#section"),
            "expected net: {}, and path: #section. Received: {key_clone:?}. asset_namespace: {}",
            Bref::default(),
            asset_namespace().bref()
        );

        // Absolute paths
        let key: NodeKey = "/docs/council/README.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "docs/council/README.md"));

        // Plain text without slashes defaults to Id (normalized)
        let plain_text = " My Node Title";
        let key: NodeKey = plain_text.parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bref::default() && id == "my-node-title"));

        // Plain text with slashes defaults to Path
        let path_text = "docs/my-node.md";
        let key: NodeKey = path_text.parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "docs/my-node.md"));
    }

    #[test]
    fn test_nodekey_parse_comprehensive() {
        let a_bid = Bid::new(Bid::nil());
        let a_bref = a_bid.bref();
        let net_bid = Bid::new(Bid::nil());
        let net_bref = net_bid.bref();

        // Test BID parsing variations
        let bid_nk = NodeKey::Bid { bid: a_bid };
        // Non-hierarchical (canonical form)
        assert_eq!(
            format!("bid:{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        // Hierarchical with network (network ignored for BIDs)
        assert_eq!(
            format!("bid://{net_bid}/{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        assert_eq!(
            format!("bid://{net_bref}/{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        // Hierarchical backward compat (bid://VALUE → hostname is the value)
        assert_eq!(
            format!("bid://{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        // Bare string
        assert_eq!(format!("{a_bid}").parse::<NodeKey>(), Ok(bid_nk));

        // Test Bref parsing variations
        let bref_nk = NodeKey::Bref { bref: a_bref };
        // Non-hierarchical (canonical form)
        assert_eq!(
            format!("bref:{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
        // Hierarchical with network (network ignored for Brefs)
        assert_eq!(
            format!("bref://{net_bid}/{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
        assert_eq!(
            format!("bref://{net_bref}/{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
        // Corrupted bref values should still error
        assert!(format!("bref://{net_bref}/{a_bref}321")
            .parse::<NodeKey>()
            .is_err());
        assert!(format!("bref://{net_bref}abc/{a_bref}")
            .parse::<NodeKey>()
            .is_err());
        // Hierarchical backward compat (bref://VALUE)
        assert_eq!(
            format!("bref://{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
        // Bare string
        assert_eq!(format!("{a_bref}").parse::<NodeKey>(), Ok(bref_nk));

        // Test ID parsing variations
        let id_str = "my-id-123";
        let id_ns_nk = NodeKey::Id {
            net: net_bref,
            id: id_str.to_string(),
        };
        let id_nil_nk = NodeKey::Id {
            net: Bid::nil().bref(),
            id: id_str.to_string(),
        };
        // Hierarchical with explicit network
        assert_eq!(
            format!("id://{net_bid}/{id_str}").parse::<NodeKey>(),
            Ok(id_ns_nk.clone())
        );
        assert_eq!(
            format!("id://{net_bref}/{id_str}").parse::<NodeKey>(),
            Ok(id_ns_nk)
        );
        // Non-hierarchical (implicit network)
        assert_eq!(format!("id:{id_str}").parse::<NodeKey>(), Ok(id_nil_nk));

        assert!(String::new().parse::<NodeKey>().is_err());

        // Test that id:bref works (non-hierarchical, bref value treated as id string)
        let id_bref_nk = NodeKey::Id {
            net: Bid::nil().bref(),
            id: net_bref.to_string(),
        };
        assert_eq!(
            format!("id:{net_bref}").parse::<NodeKey>(),
            Ok(id_bref_nk.clone())
        );
        // Hierarchical backward compat (id://VALUE → hostname is the value)
        assert_eq!(
            format!("id://{net_bref}").parse::<NodeKey>(),
            Ok(id_bref_nk.clone())
        );

        // Test bare strings without slashes (treated as normalized Id)
        let title_str = " Abc 123 789";
        let title_id_nk = NodeKey::Id {
            net: Bid::nil().bref(),
            id: "abc-123-789".to_string(),
        };
        assert_eq!(title_str.parse::<NodeKey>(), Ok(title_id_nk));

        // Test bare strings with slashes (treated as Path)
        let path_str = "docs/README.md";
        let path_nk = NodeKey::Path {
            net: Bid::nil().bref(),
            path: "docs/README.md".to_string(),
        };
        assert_eq!(path_str.parse::<NodeKey>(), Ok(path_nk));

        // Test path strings with default network
        // Note: Bare strings without path indicators (/, #, .) become Ids, not Paths
        let default_bref = Bref::default();

        // Strings with path indicators (/, #, .) become Paths
        assert_eq!(
            "net/.hidden#achor".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/.hidden#achor".to_string()
            })
        );
        // .toml files are now treated as assets (not BeliefNetwork documents)
        assert_eq!(
            "net/dir/file.toml".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: asset_namespace().bref(),
                path: "net/dir/file.toml".to_string()
            })
        );
        assert_eq!(
            "file.toml".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: asset_namespace().bref(),
                path: "file.toml".to_string()
            })
        );
        // index.md files are BeliefNetwork documents (not assets)
        assert_eq!(
            "net/dir/index.md".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/dir/index.md".to_string()
            })
        );
        assert_eq!(
            "index.md".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "index.md".to_string()
            })
        );
        assert_eq!(
            "net/file1.md#description".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/file1.md#description".to_string()
            })
        );
        assert_eq!(
            "../common_dir/common_file.md".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "../common_dir/common_file.md".to_string()
            })
        );
        assert_eq!(
            "net/dir".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/dir".to_string()
            })
        );
        assert_eq!(
            "net/.dir".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/.dir".to_string()
            })
        );
        assert_eq!(
            ".dir".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: ".dir".to_string()
            })
        );

        // Test path strings with explicit network
        // .toml files are assets, so they use asset_namespace regardless of specified network
        let asset_bref = asset_namespace().bref();
        assert_eq!(
            format!("{net_bref}/file.toml").parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: asset_bref,
                path: "file.toml".to_string()
            })
        );
        assert_eq!(
            format!("{net_bid}/file.toml").parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: asset_bref,
                path: "file.toml".to_string()
            })
        );

        // Test other asset paths also use asset_namespace
        assert_eq!(
            "net/dir/file.png".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: asset_bref,
                path: "net/dir/file.png".to_string()
            })
        );
        assert_eq!(
            format!("{net_bref}/net/dir/file.png").parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: asset_bref,
                path: "net/dir/file.png".to_string()
            })
        );
    }

    #[test]
    fn test_regularize_unchecked_asset_paths() {
        let base_net = Bid::new(Bid::nil());
        let asset_bref = asset_namespace().bref();

        // Asset path from same directory — ./assets/img.png normalizes to assets/img.png,
        // join against owner doc dir produces the correct repo-relative path.
        let key: NodeKey = "./assets/test_image.png".parse().unwrap();
        assert_eq!(
            key,
            NodeKey::Path {
                net: asset_bref,
                path: "assets/test_image.png".to_string()
            }
        );
        let result = key.regularize_unchecked(base_net, "asset_tracking_test.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: asset_bref,
                path: "assets/test_image.png".to_string()
            }
        );

        // Asset path from subdirectory — ../assets/img.png relative to subnet1/file.md
        // resolves to assets/img.png
        let key: NodeKey = "../assets/test_image.png".parse().unwrap();
        assert_eq!(
            key,
            NodeKey::Path {
                net: asset_bref,
                path: "../assets/test_image.png".to_string()
            }
        );
        let result = key.regularize_unchecked(base_net, "subnet1/subnet1_file1.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: asset_bref,
                path: "assets/test_image.png".to_string()
            }
        );

        // Asset path that's already repo-relative (no ..) still works
        let key: NodeKey = "assets/diagram.pdf".parse().unwrap();
        let result = key.regularize_unchecked(base_net, "docs/readme.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: asset_bref,
                path: "docs/assets/diagram.pdf".to_string()
            }
        );

        // Codec (.md) paths get base_net assigned, not asset_namespace
        let default_bref = Bid::nil().bref();
        let key: NodeKey = "../other.md".parse().unwrap();
        assert_eq!(
            key,
            NodeKey::Path {
                net: default_bref,
                path: "../other.md".to_string()
            }
        );
        let result = key.regularize_unchecked(base_net, "subnet1/file.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: base_net.bref(),
                path: "other.md".to_string()
            }
        );

        // href paths are never joined
        let key = NodeKey::Path {
            net: href_namespace().bref(),
            path: "https://example.com/page".to_string(),
        };
        let result = key.regularize_unchecked(base_net, "docs/file.md");
        assert_eq!(result, key);
    }

    #[test]
    fn test_resolve_against() {
        let asset_bref = asset_namespace().bref();

        // Asset link from document in same directory
        let key: NodeKey = "./assets/test_image.png".parse().unwrap();
        let result = key.resolve_against("asset_tracking_test.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: asset_bref,
                path: "assets/test_image.png".to_string()
            }
        );

        // Asset link from subdirectory going up
        let key: NodeKey = "../assets/test_image.png".parse().unwrap();
        let result = key.resolve_against("subnet1/subnet1_file1.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: asset_bref,
                path: "assets/test_image.png".to_string()
            }
        );

        // Boundary escape — link goes above repo root
        let key: NodeKey = "../../outside.png".parse().unwrap();
        let result = key.resolve_against("file.md");
        // Warns and returns normalized-but-un-joined
        assert_eq!(
            result,
            NodeKey::Path {
                net: asset_bref,
                path: "../../outside.png".to_string()
            }
        );

        // Codec path resolution preserves default net
        let default_bref = Bid::nil().bref();
        let key: NodeKey = "../sibling.md".parse().unwrap();
        let result = key.resolve_against("subdir/current.md");
        assert_eq!(
            result,
            NodeKey::Path {
                net: default_bref,
                path: "sibling.md".to_string()
            }
        );

        // href paths are unchanged
        let key = NodeKey::Path {
            net: href_namespace().bref(),
            path: "https://example.com".to_string(),
        };
        let result = key.resolve_against("any/doc.md");
        assert_eq!(result, key);

        // Non-Path variants are unchanged
        let key = NodeKey::Id {
            net: default_bref,
            id: "some-id".to_string(),
        };
        let result = key.resolve_against("any/doc.md");
        assert_eq!(result, key);
    }
}
