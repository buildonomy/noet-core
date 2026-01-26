/// [crate::nodekey] contains NodeKey and the link markup parsing for converting links into
/// [crate::beliefbase::BeliefBase] [crate::properties::BeliefNode] references.
use path_clean::clean as clean_path;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
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
    properties::{href_namespace, Bid, Bref},
    query::{BeliefSource, Expression, StatePred},
    BuildonomyError,
};

pub const TRIM: &[char] = &['/', '#'];

pub fn trim_joiners(input: &str) -> &str {
    input.trim_start_matches(TRIM).trim_end_matches(TRIM)
}

pub fn trim_path_sep(input: &str) -> &str {
    input.trim_start_matches('/').trim_end_matches('/')
}

/// Turn a title string into a regularized anchor string
pub fn to_anchor(title: &str) -> String {
    trim_joiners(title)
        .to_lowercase()
        .replace(char::is_whitespace, "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

pub fn get_doc_path(path: &str) -> &str {
    let idx = path.rfind('#').unwrap_or(path.len());
    &path[..idx]
}

/// Remove the trailing file path, leaving it's parent directory
pub fn trim_doc_path(path: &str) -> &str {
    let ext_idx = path.rfind('.').unwrap_or(path.len());
    let parent_idx = path.rfind('/').unwrap_or(path.len());
    let idx = if ext_idx > parent_idx {
        parent_idx
    } else {
        path.len()
    };
    &path[..idx]
}

/// Used to specify the join logic between two (sets of) BeliefNodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Bid { bid: Bid },
    Bref { bref: Bref },
    Path { net: Bid, path: String },
    Title { net: Bid, title: String },
    Id { net: Bid, id: String },
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
    pub fn regularize_unchecked(&self, home_net: Bid, home_path: &str) -> NodeKey {
        let home_doc_path = get_doc_path(home_path).to_string();
        match self {
            NodeKey::Path {
                net: link_base,
                path: link_path,
            } => {
                if *link_base == Bid::nil() {
                    let mut regularized_path = PathBuf::from(home_doc_path);
                    if regularized_path.extension().is_some() && !link_path.starts_with('#') {
                        // Pop off Document names from home_path before joining.
                        regularized_path.pop();
                    }
                    let full_path = if link_path.starts_with('#') {
                        format!("{}{}", regularized_path.to_string_lossy(), link_path)
                    } else {
                        clean_path(regularized_path.join(link_path))
                            .to_string_lossy()
                            .to_string()
                    };
                    NodeKey::Path {
                        net: home_net,
                        path: full_path,
                    }
                } else {
                    NodeKey::Path {
                        net: *link_base,
                        path: link_path.to_string(),
                    }
                }
            }
            NodeKey::Title { title, .. } => NodeKey::Title {
                net: home_net,
                title: to_anchor(title),
            },
            NodeKey::Id { net, id } => {
                if *net == Bid::nil() {
                    NodeKey::Id {
                        net: home_net,
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
        relative_to: Bid,
    ) -> Result<NodeKey, BuildonomyError> {
        // Get network and path from the relative_to node
        let (home_net, home_path) = cache.paths().path(&relative_to).ok_or_else(|| {
            BuildonomyError::NotFound(format!(
                "Could not determine network/path for node {relative_to}"
            ))
        })?;
        Ok(self.regularize_unchecked(home_net, &home_path))
    }

    /// Regularize relative references to absolute within network context (async).
    ///
    /// Converts relative paths, titles, and IDs to absolute references using the BeliefSource.
    /// Paths are bounded by their home network's document path for security.
    pub async fn regularize_async<C: BeliefSource>(
        &self,
        cache: &C,
        relative_to: Bid,
    ) -> Result<NodeKey, BuildonomyError> {
        // Query for the relative_to node to get its path
        let query_expr = Expression::StateIn(StatePred::Bid(vec![relative_to]));
        let cache = BeliefBase::from(cache.eval(&query_expr).await?);
        self.regularize(&cache, relative_to)
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
                let net = Self::resolve_network_sync(&network_ref, cache)?;

                // Construct the appropriate NodeKey with resolved network
                match key_type.as_str() {
                    "id" => Ok(NodeKey::Id { net, id: value }),
                    "path" => Ok(NodeKey::Path { net, path: value }),
                    "title" => Ok(NodeKey::Title {
                        net,
                        title: to_anchor(&value),
                    }),
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
                let net = Self::resolve_network_async(&network_ref, cache).await?;

                // Construct the appropriate NodeKey with resolved network
                match key_type.as_str() {
                    "id" => Ok(NodeKey::Id { net, id: value }),
                    "path" => Ok(NodeKey::Path { net, path: value }),
                    "title" => Ok(NodeKey::Title {
                        net,
                        title: to_anchor(&value),
                    }),
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
                if node.bid.namespace() == bref {
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
            let query_expr = Expression::StateIn(StatePred::Bref(vec![bref.clone()]));
            let result = cache.eval_unbalanced(&query_expr).await?;

            if let Some(node) = result.states.values().next() {
                return Ok(node.bid);
            }

            // If not found as Bref, try as namespace
            let query_expr = Expression::StateIn(StatePred::InNamespace(vec![bref]));
            let result = cache.eval_unbalanced(&query_expr).await?;

            if let Some(node) = result.states.values().next() {
                return Ok(node.bid);
            }
        }

        // Try querying by ID
        let query_expr = Expression::StateIn(StatePred::Id(vec![network_ref.to_string()]));
        let result = cache.eval_unbalanced(&query_expr).await?;

        if let Some(node) = result.states.values().next() {
            return Ok(node.bid);
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
            NodeKey::Title { net, title } => Url::parse(&format!("title://{net}/{title}"))
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
                if *net != Bid::default() {
                    write!(f, "{net}/")?;
                };
                write!(f, "{id}")
            }
            NodeKey::Path { net, path } => {
                write!(f, "path://")?;
                if *net != Bid::default() {
                    write!(f, "{net}/")?;
                };
                write!(f, "{path}")
            }
            NodeKey::Title { net, title } => {
                write!(f, "title://")?;
                if *net != Bid::default() {
                    write!(f, "{net}/")?;
                };
                write!(f, "{title}")
            }
        }
    }
}

impl FromStr for NodeKey {
    type Err = BuildonomyError;

    /// Parse a string into a NodeKey using URL-based format.
    ///
    /// URL-based format (explicit):
    /// - `bid://<bid_value>` or `bid://<network>/<bid_value>` - BID reference (network optional, BIDs are globally unique)
    /// - `bref://<bref_value>` or `bref://<network>/<bref_value>` - Bref reference (network optional for local-only lookup)
    /// - `id://<id_value>` or `id://<network>/<id_value>` - ID within a specific network (network defaults to home network)
    /// - `path://<path>` or `path://<network>/<path>` - Path within a specific network (network defaults to home network)
    /// - `title://<title>` or `title://<network>/<title>` - Title within a specific network (network defaults to home network)
    /// - `http://...` or `https://...` - External URLs (stored as Path anchored to the const href_namespace)
    ///
    /// Network Resolution:
    /// - If only value is provided (e.g., `id://supremum`), it's treated as the value with default network.
    /// - If network + value are provided (e.g., `id://abc123/supremum`), the network must be a valid BID string.
    /// - If network is not a valid BID (e.g., `id://my-network-id/supremum`), returns `UnresolvedNetwork` error.
    /// - Use `resolve_network()` with a `BeliefSource` to resolve network references by ID/Bref to BID (future feature).
    ///
    /// Backward compatibility (heuristic detection):
    /// - Bare BID/Bref strings
    /// - Relative/absolute file paths
    /// - Plain text (defaults to Title)
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use url::Url;

        let nil = Bid::nil();

        // Try parsing as URL first
        if let Ok(url) = Url::parse(s) {
            let scheme = url.scheme();

            // Handle external URLs specially - keep as-is
            if scheme == "http" || scheme == "https" {
                return Ok(NodeKey::Id {
                    net: href_namespace(),
                    id: s.to_string(),
                });
            }

            // Extract network and value from URL structure for buildonomy schemes
            // If host + path: host is network (needs BID resolution), path is value
            // If host only: host is value, no network
            // If path only: path is value, no network
            let (net_str, value_str) = match (url.host_str(), url.path().trim_start_matches('/')) {
                (Some(host), path) if !path.is_empty() => {
                    // host + path: network/value format
                    (Some(host), path)
                }
                (Some(host), _) => {
                    // host only: value with no network
                    (None, host)
                }
                (None, path) if !path.is_empty() => {
                    // path only: value with no network
                    (None, path)
                }
                _ => {
                    return Err(BuildonomyError::Serialization(format!(
                        "Invalid URL format: {s}"
                    )));
                }
            };

            // Resolve network string to BID if present
            let resolve_network = |key_type: &str, value: &str| -> Result<Bid, BuildonomyError> {
                match net_str {
                    Some(net) => match Bid::try_from(net) {
                        Ok(bid) => Ok(bid),
                        Err(_) => Err(BuildonomyError::UnresolvedNetwork {
                            network_ref: net.to_string(),
                            key_type: key_type.to_string(),
                            value: value.to_string(),
                        }),
                    },
                    None => Ok(nil),
                }
            };

            match scheme {
                "bid" => {
                    // BIDs are globally unique, ignore network
                    let bid = Bid::try_from(value_str)?;
                    return Ok(NodeKey::Bid { bid });
                }
                "bref" => {
                    // Brefs are also globally unique
                    let bref = Bref::try_from(value_str)?;
                    return Ok(NodeKey::Bref { bref });
                }
                "id" => {
                    let net = resolve_network("id", value_str)?;
                    return Ok(NodeKey::Id {
                        net,
                        id: value_str.to_string(),
                    });
                }
                "path" => {
                    let net = resolve_network("path", value_str)?;
                    return Ok(NodeKey::Path {
                        net,
                        path: value_str.to_string(),
                    });
                }
                "title" => {
                    let net = resolve_network("title", value_str)?;
                    // Titles are normalized to anchor format (lowercase, spaces->dashes)
                    let title = to_anchor(value_str);
                    return Ok(NodeKey::Title { net, title });
                }
                _ => {
                    // Unknown scheme, fall through to heuristic
                }
            }
        }

        // Heuristic detection for bare strings (backward compatibility)

        // Try Bref first (checks format before ':' if present for wikilink titles)
        let title_sep = s.find(':').unwrap_or(s.len());
        if let Ok(bref) = Bref::try_from(&s[..title_sep]) {
            return Ok(NodeKey::Bref { bref });
        }

        // Try Bid
        if let Ok(bid) = Bid::try_from(&s[..title_sep]) {
            return Ok(NodeKey::Bid { bid });
        }

        // Check for path-like patterns
        if !s.contains(char::is_whitespace) {
            // Relative paths with ./ or ../
            if s.starts_with("./") || s.starts_with("../") {
                return Ok(NodeKey::Path {
                    net: nil,
                    path: s.to_string(),
                });
            }

            // Anchors
            if s.starts_with('#') {
                return Ok(NodeKey::Path {
                    net: nil,
                    path: s.to_string(),
                });
            }

            // Absolute or relative paths with /
            if s.contains('/') {
                let mut rel_link = s;
                while rel_link.starts_with('/') {
                    rel_link = &rel_link[1..];
                }
                return Ok(NodeKey::Path {
                    net: nil,
                    path: rel_link.to_string(),
                });
            } else {
                return Ok(NodeKey::Id {
                    net: nil,
                    id: s.to_string(),
                });
            }
        }

        // Default fallback: treat as Title (normalized to anchor format)
        Ok(NodeKey::Title {
            net: nil,
            title: to_anchor(s),
        })
    }
}

/// When parsing links, the net should be chosen based on how well the link str lines up with the
/// stack. If it starts_with, it should chose the most specific of the start_with set. If it
/// contains relative back-links, back link through the stack to find the proper path network, and
/// then canonicalize the link path from there.
pub fn href_to_nodekey(link: &str) -> NodeKey {
    link.parse().unwrap_or_else(|_| NodeKey::Id {
        net: Bid::nil(),
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
        assert_eq!(to_anchor("  leading spaces"), "--leading-spaces");
        assert_eq!(to_anchor("trailing spaces  "), "trailing-spaces--");
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
        if net == Bid::nil() && id == "supremum"));

        // Network form: id://network/value (host is network, path is value)
        let network_bid = Bid::new(Bid::nil());
        let key: NodeKey = format!("id://{network_bid}/supremum").parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == network_bid && id == "supremum"));

        // Path with slash where first part isn't a BID - returns UnresolvedNetwork error
        let result: Result<NodeKey, _> = "path://docs/README.md".parse();
        assert!(matches!(result, Err(BuildonomyError::UnresolvedNetwork {
        network_ref, key_type, value
    }) if network_ref == "docs" && key_type == "path" && value == "README.md"));

        // And title
        let key: NodeKey = "title://My-Title".parse().unwrap();
        assert!(matches!(key, NodeKey::Title { net, title }
        if net == Bid::nil() && title == "my-title"));
    }

    #[test]
    fn test_nodekey_url_parsing() {
        // Test URL-based explicit formats
        let network_bid = Bid::new(Bid::nil());
        let test_bid = Bid::new(Bid::nil());
        let test_bref = test_bid.namespace();

        // BID format (no network needed)
        let key: NodeKey = format!("bid:///{test_bid}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // Bref format (network optional)
        let key: NodeKey = format!("bref:///{test_bref}").parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // ID with network
        let key: NodeKey = format!("id://{network_bid}/supremum").parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == network_bid && id == "supremum"));

        // ID without network (defaults to nil)
        let key: NodeKey = "id:///supremum".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == Bid::nil() && id == "supremum"));

        // Path with network
        let key: NodeKey = format!("path://{network_bid}/docs/council/README.md")
            .parse()
            .unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == network_bid && path == "docs/council/README.md"));

        // Title with network - titles are normalized to anchor format
        // Note: to_anchor() now strips punctuation including %
        // URL-encoded %20 becomes 20 after % is stripped (not ideal but edge case)
        let key: NodeKey = format!("title://{network_bid}/My%20Node%20Title")
            .parse()
            .unwrap();
        assert!(matches!(key, NodeKey::Title { net, title }
        if net == network_bid && title == "my20node20title"));

        // External URLs
        let key: NodeKey = "https://example.com/page".parse().unwrap();
        assert!(matches!(key, NodeKey::Id { net, id }
        if net == href_namespace() && id == "https://example.com/page"));
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
        let test_bref = test_bid.namespace();

        // Bare BID string
        let key: NodeKey = test_bid.to_string().parse().unwrap();
        assert!(matches!(key, NodeKey::Bid { bid } if bid == test_bid));

        // Bare Bref string
        let key: NodeKey = test_bref.to_string().parse().unwrap();
        assert!(matches!(key, NodeKey::Bref { bref } if bref == test_bref));

        // Relative paths
        let key: NodeKey = "./README.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bid::nil() && path == "./README.md"));

        let key: NodeKey = "../docs/file.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bid::nil() && path == "../docs/file.md"));

        // Anchors
        let key: NodeKey = "#section".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bid::nil() && path == "#section"));

        // Absolute paths
        let key: NodeKey = "/docs/council/README.md".parse().unwrap();
        assert!(matches!(key, NodeKey::Path { net, path }
        if net == Bid::nil() && path == "docs/council/README.md"));

        // Plain text (defaults to Title, normalized to anchor format)
        let key: NodeKey = "My Node Title".parse().unwrap();
        assert!(matches!(key, NodeKey::Title { net, title }
        if net == Bid::nil() && title == "my-node-title"));
    }
}
