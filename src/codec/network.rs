use crate::{
    beliefbase::BeliefContext,
    codec::{
        md::{build_title_attribute, MdCodec},
        DocCodec, ProtoBeliefNode, CODECS,
    },
    error::BuildonomyError,
    nodekey::NodeKey,
    paths::{os_path_to_string, AnchorPath},
    properties::{BeliefKind, BeliefNode, Bref, Weight, WeightKind},
};
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

/// Standard filename designating a directory as the root of a BeliefNetwork.
///
/// The `index.md` file can contain YAML, JSON, or TOML format metadata in its frontmatter.
/// Format is auto-detected via fallback parsing (YAML → JSON → TOML).
pub const NETWORK_NAME: &str = "index.md";

/// Iterates through a directory subtree, filtering to return a sorted list of network directories
/// (directories containing an index.md file), as well as file paths
/// matching known codec extensions.
fn iter_net_docs<P: AsRef<Path>>(path: P) -> Vec<PathBuf> {
    fn is_hidden(entry: &DirEntry) -> bool {
        entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with("."))
            .unwrap_or(false)
    }
    let mut subnets = Vec::default();
    let mut sorted_files = WalkDir::new(&path)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) || e.path() == path.as_ref())
        .filter_map(|e| e.ok().map(|e| e.into_path()))
        .filter_map(|mut p| {
            if p.is_file() {
                // First check if this is a network config file (.noet)
                let p_str = os_path_to_string(&p);
                let p_ap = AnchorPath::new(&p_str);
                if NETWORK_NAME == p_ap.filename() {
                    // This is a network config file - return its parent directory
                    p.pop();
                    if !p.eq(&path.as_ref()) {
                        subnets.push(p.clone());
                        return Some(p);
                    } else {
                        return None;
                    }
                }

                // Then check if this has a registered codec

                if CODECS.get(&p_ap).is_some() {
                    if subnets.iter().any(|subnet_path| p.starts_with(subnet_path)) {
                        // Don't include subnet files
                        None
                    } else {
                        Some(p)
                    }
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<PathBuf>>();
    // Collect parent directories, ordered from deepest to shallowest
    sorted_files.sort_by(|a, b| a.components().cmp(b.components()));
    sorted_files.dedup();
    sorted_files
}

/// Detect network file in directory and return path to that file.
pub fn detect_network_file(dir: &Path) -> Option<PathBuf> {
    if dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|&name| name == NETWORK_NAME)
        .is_some()
    {
        return Some(dir.to_path_buf());
    }
    let mut path = dir.to_path_buf();
    if !path.is_dir() {
        path.pop();
    }
    path.push(NETWORK_NAME);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[derive(Debug, Default, Clone)]
pub struct NetworkCodec(MdCodec);

impl DocCodec for NetworkCodec {
    /// Parse a path into a proto network node, if detect_network_file returns Some, then populate
    /// that ProtoBeliefNode, discovering direct filesystem descendants and setting path and kind
    /// correctly.
    ///
    /// This method handles filesystem traversal to discover a network's direct children.
    /// Per the graph design, each network owns a **flat list** of 'document' or 'network' nodes
    /// that are its **direct filesystem descendants**. This means:
    ///
    /// - **Prune subdirectories** containing BeliefNetwork files (they are sub-networks)
    /// - **Flatten all other files** matching CODEC extensions as direct source→sink connections
    /// - The parent network treats the entire non-network filetree as its direct children
    ///
    /// ## Alternative Implementations via Codec Swapping
    ///
    /// This filesystem-based implementation is just one strategy. The [`crate::codec::CODECS`] map
    /// allows swapping implementations at runtime for different environments:
    ///
    /// - **Native/Desktop**: Use this `ProtoBeliefNode` with direct filesystem access
    /// - **Browser/WASM**: Swap in a `BrowserProtoBeliefNode` that reads from IndexedDB
    /// - **Testing**: Swap in a `MockProtoBeliefNode` with in-memory content
    ///
    /// The codec abstraction provides this flexibility without changing the compiler or
    /// builder layers. See [crate::codec] for details on how to swap out `CODECS`.
    fn proto(
        &self,
        repo_path: &Path,
        path: &Path,
    ) -> Result<Option<ProtoBeliefNode>, BuildonomyError> {
        let file_path = repo_path.join(path);
        let Some(network_filepath) = detect_network_file(&file_path) else {
            return Ok(None);
        };
        let network_dir = network_filepath.parent().expect(
            "detect network file returns a path where path.is_file() is true, \
            therefore path.parent() must succeed.",
        );
        let rel_path = network_filepath.strip_prefix(repo_path)?;
        let Some(mut proto) = MdCodec::new().proto(repo_path, rel_path)? else {
            return Ok(None);
        };
        if proto.id().is_none() {
            return Err(BuildonomyError::Codec(format!(
                "Network nodes require a semantic ID. Received: {proto:?}"
            )));
        }
        let rel_str = os_path_to_string(rel_path);
        proto.path = AnchorPath::new(&rel_str).dir().to_string();
        proto.kind.insert(BeliefKind::Network);
        proto.heading = 1;
        for doc_path in iter_net_docs(network_dir) {
            if let Ok(relative_path) = doc_path.strip_prefix(network_dir) {
                let path_str = os_path_to_string(relative_path);
                if !path_str.is_empty() {
                    let node_key = NodeKey::Path {
                        // net will be resolved during processing by calling Key::regularize
                        net: Bref::default(),
                        path: path_str.clone(),
                    };
                    let mut weight = Weight::default();
                    weight.set_doc_paths(vec![path_str]).ok();
                    proto
                        .upstream
                        .push((node_key, WeightKind::Section, Some(weight)));
                }
            }
        }

        Ok(Some(proto))
    }

    fn parse(&mut self, content: &str, current: ProtoBeliefNode) -> Result<(), BuildonomyError> {
        self.0.parse(content, current)?;
        let Some(first_tuple) = self.0.current_events.first_mut() else {
            return Err(BuildonomyError::Codec(
                "Network file has no content".to_string(),
            ));
        };
        first_tuple.0.heading = 1;
        first_tuple.0.kind.insert(BeliefKind::Network);
        Ok(())
    }

    fn nodes(&self) -> Vec<ProtoBeliefNode> {
        self.0.nodes()
    }

    fn inject_context(
        &mut self,
        node: &ProtoBeliefNode,
        ctx: &BeliefContext<'_>,
    ) -> Result<Option<BeliefNode>, BuildonomyError> {
        self.0.inject_context(node, ctx)
    }

    fn finalize(&mut self) -> Result<Vec<(ProtoBeliefNode, BeliefNode)>, BuildonomyError> {
        self.0.finalize()
    }

    fn generate_source(&self) -> Option<String> {
        self.0.generate_source()
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn generate_deferred_html(
        &self,
        ctx: &BeliefContext<'_>,
    ) -> Result<Vec<(String, String)>, BuildonomyError> {
        use crate::properties::{WeightKind, WEIGHT_SORT_KEY};

        // Only generate index.html for Network nodes
        if !ctx.node.kind.is_network() {
            return Ok(vec![]);
        }

        // Query child documents via Section (subsection) edges
        let sources = ctx.sources();
        let mut children: Vec<_> = sources
            .iter()
            .filter_map(|edge| {
                // Check if this edge has a Section weight (subsection relationship)
                edge.weight.get(&WeightKind::Section).map(|section_weight| {
                    let sort_key: u16 = section_weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
                    (edge, sort_key)
                })
            })
            .collect();

        // Sort by WEIGHT_SORT_KEY
        children.sort_by_key(|(_, sort_key)| *sort_key);

        let mut html = String::new();
        if let Some(description) = ctx.node.payload.get("description").and_then(|v| v.as_str()) {
            html.push_str(&format!("<p>{}</p>\n", description));
        }

        if children.is_empty() {
            html.push_str("<p><em>No documents in this network yet.</em></p>\n");
        } else {
            html.push_str("<ul>\n");
            let mut last_subdir: Option<String> = None;
            for (edge, _sort_key) in children {
                // Convert home_path to HTML link (replace extension with .html)
                let mut link_path = edge.root_path.clone();
                let link_ap = AnchorPath::from(&edge.root_path);
                // Normalize document links to .html extension
                if CODECS.get(&link_ap).is_some() {
                    link_path = link_ap.replace_extension("html");
                }

                let title = edge.other.display_title();
                if link_ap.dir().is_empty() {
                    if last_subdir.is_some() {
                        html.push_str("</ul></li>");
                        last_subdir = None;
                    }
                } else if let Some(ref last_dir) = last_subdir {
                    if link_ap.dir() != last_dir {
                        html.push_str(&format!("</ul></li><li><span>{}</span><ul>", link_ap.dir()));
                        last_subdir = Some(link_ap.dir().to_string());
                    }
                } else {
                    html.push_str(&format!("<li><span>{}</span><ul>", link_ap.dir()));
                    last_subdir = Some(link_ap.dir().to_string());
                }

                // Get bref for the child node to add to title attribute
                let bref_attr = ctx
                    .belief_set()
                    .brefs()
                    .iter()
                    .find_map(|(bref, bid)| {
                        if bid == &edge.other.bid {
                            Some(format!(
                                " title=\"{}\"",
                                build_title_attribute(&format!("bref://{}", bref), false, None)
                            ))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                html.push_str(&format!(
                    "  <li><a href=\"/{}\"{}>{}</a></li>\n",
                    link_path, bref_attr, title
                ));
            }
            if last_subdir.is_some() {
                html.push_str("</ul></li>\n");
            }
            html.push_str("</ul>\n");
        }

        // Output filename is index.html (caller handles directory path)
        Ok(vec![("index.html".to_string(), html)])
    }
}

impl std::ops::Deref for NetworkCodec {
    type Target = MdCodec;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for NetworkCodec {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
