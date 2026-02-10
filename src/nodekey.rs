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
    paths::{to_anchor, AnchorPath},
    properties::{asset_namespace, href_namespace, Bid, Bref},
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

/// Used to specify the join logic between two (sets of) BeliefNodes.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NodeKeyScheme {
    Bid,
    Bref,
    #[default]
    Path,
    Id,
}

impl From<&str> for NodeKeyScheme {
    fn from(scheme_str: &str) -> Self {
        match scheme_str.to_lowercase().trim() {
            "bid" => NodeKeyScheme::Bid,
            "bref" => NodeKeyScheme::Bref,
            "id" => NodeKeyScheme::Id,
            "path" => NodeKeyScheme::Path,
            _ => NodeKeyScheme::default(),
        }
    }
}

uniffi::custom_type!(NodeKey, String, {
    try_lift: |val| Ok(from_str(&val)?),
    lower: |obj| to_string(&obj).expect("NodeKeys should serialize without error")
});

impl NodeKey {
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
                let normalized_link_str = AnchorPath::from(link_path).normalize();
                let link_ap = AnchorPath::from(&normalized_link_str);
                if !link_base.is_default() {
                    // All we can do is ensure the link_base is normalized
                    return NodeKey::Path {
                        net: *link_base,
                        path: normalized_link_str,
                    };
                }
                if link_ap.is_absolute() {
                    tracing::warn!(
                        "[Nodekey::regularize] NodeKey::Path supplied with an \
                        absolute path {}, but without an anchoring network. The meaning of \
                        the path is unclear. Assuming this is rooted at the root dir of \
                        the supplied network {}",
                        normalized_link_str,
                        base_net
                    );
                    return NodeKey::Path {
                        net: base_net.bref(),
                        path: normalized_link_str,
                    };
                }

                let join_path = base_ap.join(&normalized_link_str);
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

                NodeKey::Path {
                    net: base_net.bref(),
                    path: join_path,
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

impl Display for NodeKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeKey::Bid { bid } => {
                write!(f, "bid://{bid}")
            }
            NodeKey::Bref { bref } => {
                write!(f, "bref://{bref}")
            }
            NodeKey::Id { net, id } => {
                write!(f, "id://")?;
                if !net.is_default() {
                    write!(f, "{net}/")?;
                };
                write!(f, "{id}")
            }
            NodeKey::Path { net, path } => {
                write!(f, "path://")?;
                if !net.is_default() {
                    write!(f, "{net}/")?;
                };
                write!(f, "{path}")
            }
        }
    }
}

impl FromStr for NodeKey {
    type Err = BuildonomyError;

    /// Parse a string into a NodeKey using URL-based format.
    ///
    /// See `tests::test_nodekey_parse_comprehensive` for detailed examples.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try parsing as URL first
        let scheme_stop = s.find(':');
        let scheme_str = &s[0..scheme_stop.unwrap_or(0)];
        let scheme = NodeKeyScheme::from(scheme_str);

        let mut path_start = scheme_stop.map(|idx| idx + 1).unwrap_or(0);
        while s.chars().nth(path_start).filter(|c| *c == '/').is_some() {
            path_start += 1;
        }

        // Try to parse network component (first part before '/')
        let remainder = &s[path_start..];
        let first_slash_pos = remainder.find('/');
        let potential_network = if let Some(pos) = first_slash_pos {
            &remainder[..pos]
        } else {
            remainder
        };

        if remainder.is_empty() {
            return Err(BuildonomyError::Serialization(format!(
                "Cannot construct a NodeKey from an empty value. Received {}",
                s
            )));
        }
        // For bare strings (no scheme), check if entire string is BID/Bref before network parsing
        if scheme_str.is_empty() {
            if let Ok(bid) = Bid::try_from(remainder) {
                return Ok(NodeKey::Bid { bid });
            }
            if let Ok(bref) = Bref::try_from(remainder) {
                return Ok(NodeKey::Bref { bref });
            }
            // If no scheme and no path-like indicators, treat as Id (title/identifier)
            // Path-like indicators: /, #, or starts with . or /
            let has_path_indicators = remainder.contains(['/', '#', '.']);
            if !has_path_indicators && !remainder.is_empty() {
                return Ok(NodeKey::Id {
                    net: Bref::default(),
                    id: to_anchor(remainder),
                });
            }
        }
        // For bid:// and bref:// schemes without a slash, skip network parsing
        // The entire string IS the bid/bref value
        let skip_network =
            matches!(scheme, NodeKeyScheme::Bid | NodeKeyScheme::Bref) && first_slash_pos.is_none();

        let (net, network_parsed) = if skip_network || potential_network.is_empty() {
            (Bid::nil().bref(), false)
        } else {
            match Bid::try_from(potential_network)
                .ok()
                .map(|bid| bid.bref())
                .or_else(|| Bref::try_from(potential_network).ok())
            {
                Some(parsed_net) => {
                    // Successfully parsed network, advance path_start past it
                    path_start += potential_network.len();
                    // Skip any slashes after the network component
                    while s.chars().nth(path_start).filter(|c| *c == '/').is_some() {
                        path_start += 1;
                    }
                    (parsed_net, true)
                }
                None => {
                    // Network parsing failed
                    // For other schemes, keep path_start unchanged to use full remainder
                    if first_slash_pos.is_some() && matches!(scheme, NodeKeyScheme::Id) {
                        path_start += potential_network.len();
                        // Skip slashes for id:// error case
                        while s.chars().nth(path_start).filter(|c| *c == '/').is_some() {
                            path_start += 1;
                        }
                    }
                    (Bid::nil().bref(), false)
                }
            }
        };

        let nk_ap = AnchorPath::from(&s[path_start..s.len()]);

        if nk_ap.path.is_empty() {
            return Err(BuildonomyError::Serialization(format!(
                "[Nodekey] cannot generate a nodekey from an empty string. \
                After parsing '{s}' for schema and network, remaining path is empty",
            )));
        }
        let key = match scheme {
            NodeKeyScheme::Bid => {
                let bid = Bid::try_from(nk_ap.path)?;
                NodeKey::Bid { bid }
            }
            NodeKeyScheme::Bref => {
                let bref = Bref::try_from(nk_ap.path)?;
                NodeKey::Bref { bref }
            }
            NodeKeyScheme::Id => {
                // For id:// with multi-part path, require valid network or return error
                if !scheme_str.is_empty()
                    && first_slash_pos.is_some()
                    && !network_parsed
                    && !potential_network.is_empty()
                {
                    return Err(BuildonomyError::UnresolvedNetwork {
                        network_ref: potential_network.to_string(),
                        key_type: "id".to_string(),
                        value: nk_ap.path.to_string(),
                    });
                }
                let id = to_anchor(nk_ap.path);
                NodeKey::Id { net, id }
            }
            NodeKeyScheme::Path => {
                let mut path_net = net;
                let norm_path = nk_ap.normalize();
                // External URLs (non-empty scheme that's not "path") should be Id, not Path
                if !matches!(
                    s[..scheme_stop.unwrap_or(0)].to_lowercase().as_str(),
                    "" | "path"
                ) {
                    tracing::debug!(
                        "Found External URL '{s}' - treating as NodeKey::Id with href_namespace"
                    );
                    return Ok(NodeKey::Id {
                        net: href_namespace().bref(),
                        id: s.to_string(),
                    });
                } else if !nk_ap.ext().is_empty()
                    && !CODECS
                        .extensions()
                        .iter()
                        .any(|codec_ext| codec_ext == nk_ap.ext())
                {
                    path_net = asset_namespace().bref();
                }
                NodeKey::Path {
                    net: path_net,
                    path: norm_path,
                }
            }
        };
        Ok(key)
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
        // Simple form: id://value (host is the value, no network)
        let key: NodeKey = "id://supremum".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bref::default() && id == "supremum"));

        // Network form: id://network/value (host is network, path is value)
        let network_bid = Bid::new(Bid::nil()).bref();
        let key: NodeKey = format!("id://{network_bid}/supremum").parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == network_bid && id == "supremum"));

        // Path with slash where first part isn't a BID - returns UnresolvedNetwork error
        let result: Result<NodeKey, _> = "path://docs/README.md".parse();
        assert_eq!(
            result,
            Ok(NodeKey::Path {
                net: Bref::default(),
                path: "docs/README.md".to_string()
            })
        );
    }

    #[test]
    fn test_nodekey_url_parsing() {
        // Test URL-based explicit formats
        let network_bid = Bid::new(Bid::nil());
        let network_bref = network_bid.bref();
        let test_bid = Bid::new(Bid::nil());
        let test_bref = test_bid.bref();

        // BID format (no network needed)
        let key: NodeKey = format!("bid://{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // BID format (no network needed), but it doesn't hurt
        let key: NodeKey = format!("bid://{network_bid}/{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // BID format (extra slashes)
        let key: NodeKey = format!("bid://///{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // Bref format (network optional)
        let key: NodeKey = format!("bref:///{test_bref}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // ID with network
        let key: NodeKey = format!("id://{network_bid}/supremum").parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == network_bref && id == "supremum"));

        // ID without network (defaults to nil)
        let key: NodeKey = "id:///supremum".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bref::default() && id == "supremum"));

        // Path with network
        let key: NodeKey = format!("path://{network_bid}/docs/council/README.md")
            .parse()
            .unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == network_bref && path == "docs/council/README.md"));

        // External URLs
        let key: NodeKey = "https://example.com/page".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == href_namespace().bref() && id == "https://example.com/page"));
    }

    #[test]
    fn test_unresolved_network_error() {
        // Network reference that's not a valid BID should return UnresolvedNetwork error
        let result: Result<NodeKey, _> = "id://my-network-id/supremum".parse();
        assert!(matches!(result, Err(BuildonomyError::UnresolvedNetwork {
        network_ref, key_type, value
    }) if network_ref == "my-network-id" && key_type == "id" && value == "supremum"));

        // Valid BID should work fine
        let network_bid = Bid::new(Bid::nil());
        let result: Result<NodeKey, _> = format!("id://{network_bid}/supremum").parse();
        assert!(result.is_ok());

        // Simple form (no network) should work fine
        let result: Result<NodeKey, _> = "id://supremum".parse();
        assert!(result.is_ok());
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
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bref::default() && path == "#section"));

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
        assert_eq!(
            format!("bid://{net_bid}/{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        assert_eq!(
            format!("bid://{net_bref}/{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        assert_eq!(
            format!("bid://{a_bid}").parse::<NodeKey>(),
            Ok(bid_nk.clone())
        );
        assert_eq!(format!("{a_bid}").parse::<NodeKey>(), Ok(bid_nk));

        // Test Bref parsing variations
        let bref_nk = NodeKey::Bref { bref: a_bref };
        assert_eq!(
            format!("bref://{net_bid}/{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
        assert_eq!(
            format!("bref://{net_bref}/{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
        assert!(format!("bref://{net_bref}/{a_bref}321")
            .parse::<NodeKey>()
            .is_err());
        assert!(format!("bref://{net_bref}abc/{a_bref}")
            .parse::<NodeKey>()
            .is_err());
        assert_eq!(
            format!("bref://{a_bref}").parse::<NodeKey>(),
            Ok(bref_nk.clone())
        );
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
        assert_eq!(
            format!("id://{net_bid}/{id_str}").parse::<NodeKey>(),
            Ok(id_ns_nk.clone())
        );
        assert_eq!(
            format!("id://{net_bref}/{id_str}").parse::<NodeKey>(),
            Ok(id_ns_nk)
        );
        assert_eq!(format!("id://{id_str}").parse::<NodeKey>(), Ok(id_nil_nk));

        assert!(String::new().parse::<NodeKey>().is_err());

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
            "net/.dir/#achor".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/.dir#achor".to_string()
            })
        );
        assert_eq!(
            "net/dir/file.toml".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "net/dir/file.toml".to_string()
            })
        );
        assert_eq!(
            "file.toml".parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: default_bref,
                path: "file.toml".to_string()
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
        assert_eq!(
            format!("{net_bref}/file.toml").parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: net_bref,
                path: "file.toml".to_string()
            })
        );
        assert_eq!(
            format!("{net_bid}/file.toml").parse::<NodeKey>(),
            Ok(NodeKey::Path {
                net: net_bref,
                path: "file.toml".to_string()
            })
        );

        // Test asset paths use asset_namespace regardless of specified network
        let asset_bref = asset_namespace().bref();
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
}
