use crate::{
    beliefset::BeliefContext,
    codec::{
        schema_registry::{get_schema_definition, migrate_schema, EdgeDirection},
        DocCodec, CODECS,
    },
    error::BuildonomyError,
    nodekey::{trim_path_sep, NodeKey},
    properties::{BeliefKind, BeliefKindSet, BeliefNode, Bid, Weight, WeightKind, WEIGHT_DOC_PATH},
};

use std::{
    ffi::OsStr,
    fs,
    io::Write,
    mem::replace,
    ops::Deref,
    path::{Path, PathBuf},
    str::FromStr,
};
use toml::{to_string, Table as TomlTable};
use toml_edit::{value, DocumentMut};
use walkdir::{DirEntry, WalkDir};

/// Standard filename designating a directory as the root of a BeliefNetwork.
pub const NETWORK_CONFIG_NAME: &str = "BeliefNetwork.toml";

/// Iterates through a directory subtree, filtering to return a sorted list of network directories
/// (directories containing a [toml::NETWORK_CONFIG_NAME] file), as well as file paths matching
/// known codec extensions.
fn iter_net_docs<P: AsRef<Path>>(path: P) -> Vec<PathBuf> {
    fn is_hidden(entry: &DirEntry) -> bool {
        entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with("."))
            .unwrap_or(false)
    }
    let network_dir_name = OsStr::new(NETWORK_CONFIG_NAME);
    let mut subnets = Vec::default();
    let mut sorted_files = WalkDir::new(&path)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) || e.path() == path.as_ref())
        .filter_map(|e| e.ok().map(|e| e.into_path()))
        .filter_map(|mut p| {
            if p.is_file() {
                if p.extension()
                    .map(|e| e.to_str())
                    .flatten()
                    .filter(|&e| {
                        CODECS
                            .extensions()
                            .iter()
                            .find(|ce| ce.as_str() == e)
                            .is_some()
                    })
                    .is_some()
                {
                    if subnets.iter().any(|subnet_path| p.starts_with(subnet_path)) {
                        // Don't include subnet files
                        None
                    } else if p.file_name() == Some(network_dir_name) {
                        p.pop();
                        if !p.eq(&path.as_ref()) {
                            subnets.push(p.clone());

                            Some(p)
                        } else {
                            None
                        }
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

/// Helper function to convert toml_edit::Item to toml::Value
/// This handles the quirks of toml_edit's to_string() method which includes TOML formatting
#[allow(dead_code)] // Kept for potential future use with document-level items
fn toml_edit_to_toml_value(item: &toml_edit::Item) -> Option<toml::Value> {
    // Handle different value types explicitly to avoid quote-wrapping issues
    if let Some(s) = item.as_str() {
        Some(toml::Value::String(s.to_string()))
    } else if let Some(i) = item.as_integer() {
        Some(toml::Value::Integer(i))
    } else if let Some(f) = item.as_float() {
        Some(toml::Value::Float(f))
    } else if let Some(b) = item.as_bool() {
        Some(toml::Value::Boolean(b))
    } else if let Some(arr) = item.as_array() {
        // Recursively convert array items (array contains Value, not Item)
        let converted: Option<Vec<toml::Value>> = arr
            .iter()
            .map(|value| toml_edit_value_to_toml_value(value))
            .collect();
        converted.map(toml::Value::Array)
    } else if let Some(table) = item.as_inline_table() {
        // Convert inline table (inline tables contain Value, not Item)
        let mut map = toml::map::Map::new();
        for (key, value) in table.iter() {
            if let Some(converted_value) = toml_edit_value_to_toml_value(value) {
                map.insert(key.to_string(), converted_value);
            }
        }
        Some(toml::Value::Table(map))
    } else {
        // Fallback: try string serialization as last resort
        // This preserves the original behavior for edge cases
        let value_str = item.to_string();
        toml::from_str::<toml::Value>(&value_str).ok()
    }
}

/// Helper function to convert toml_edit::Value to toml::Value
/// Similar to above but for Value type (used in arrays)
fn toml_edit_value_to_toml_value(value: &toml_edit::Value) -> Option<toml::Value> {
    if let Some(s) = value.as_str() {
        Some(toml::Value::String(s.to_string()))
    } else if let Some(i) = value.as_integer() {
        Some(toml::Value::Integer(i))
    } else if let Some(f) = value.as_float() {
        Some(toml::Value::Float(f))
    } else if let Some(b) = value.as_bool() {
        Some(toml::Value::Boolean(b))
    } else if let Some(arr) = value.as_array() {
        let converted: Option<Vec<toml::Value>> = arr
            .iter()
            .map(|v| toml_edit_value_to_toml_value(v))
            .collect();
        converted.map(toml::Value::Array)
    } else if let Some(table) = value.as_inline_table() {
        let mut map = toml::map::Map::new();
        for (key, val) in table.iter() {
            if let Some(converted_value) = toml_edit_value_to_toml_value(val) {
                map.insert(key.to_string(), converted_value);
            }
        }
        Some(toml::Value::Table(map))
    } else {
        // Fallback
        let value_str = value.to_string();
        toml::from_str::<toml::Value>(&value_str).ok()
    }
}

// pub fn parse_procedure(
//     parent_bid: Bid,
//     procedure: &toml::Value,
// ) -> Result<Vec<ProtoBeliefNode>, BuildonomyError> {
//     let mut protos = Vec::new();
//     if let Some(steps) = procedure.get("steps").and_then(|s| s.as_array()) {
//         for step in steps {
//             protos.extend(parse_step(parent_bid, step)?);
//         }
//     }
//     Ok(protos)
// }

// fn parse_step(
//     parent_bid: Bid,
//     step: &toml::Value,
// ) -> Result<Vec<ProtoBeliefNode>, BuildonomyError> {
//     let mut protos = Vec::new();
//     let step_type = step.get("type").and_then(|t| t.as_str());

//     match step_type {
//         Some("sequence") | Some("parallel") | Some("all_of") | Some("any_of") | Some("avoid") => {
//             let payload = step.as_table().cloned();
//             let operator_node = ProtoBeliefNode {
//                 bid: Some(Bid::new(parent_bid)),
//                 kind: BeliefKind::Ephemeral.into(),
//                 title: Some(step_type.unwrap().to_string()),
//                 upstream: vec![(
//                     NodeKey::Bid { bid: parent_bid },
//                     WeightKind::Pragmatic,
//                     payload,
//                 )],
//                 ..Default::default()
//             };
//             let operator_bid = operator_node.bid.unwrap();
//             protos.push(operator_node);

//             if let Some(steps) = step.get("steps").and_then(|s| s.as_array()) {
//                 for sub_step in steps {
//                     protos.extend(parse_step(operator_bid, sub_step)?);
//                 }
//             }
//         }
//         _ => {
//             // This handles action, prompt, opens_window, and references (string)
//             let payload = step.as_table().cloned();
//             let mut action_node = ProtoBeliefNode {
//                 bid: Some(Bid::new(parent_bid)),
//                 kind: BeliefKind::Ephemeral.into(),
//                 upstream: vec![(
//                     NodeKey::Bid { bid: parent_bid },
//                     WeightKind::Pragmatic,
//                     payload,
//                 )],
//                 ..Default::default()
//             };

//             if let Some(id) = step.get("id").and_then(|i| i.as_str()) {
//                 action_node.title = Some(id.to_string());
//             }

//             if let Some(reference) = step.get("reference").and_then(|r| r.as_str()) {
//                 action_node.upstream.push((
//                     href_to_nodekey(reference),
//                     WeightKind::Pragmatic,
//                     None,
//                 ));
//             } else if let Some(reference) = step.as_str() {
//                 action_node.upstream.push((
//                     href_to_nodekey(reference),
//                     WeightKind::Pragmatic,
//                     None,
//                 ));
//             }

//             protos.push(action_node);
//         }
//     }

//     Ok(protos)
// }

// /// Parse parent_connections from TOML frontmatter and convert to downstream.
// ///
// /// This function traverses the parent_connections array in the TOML tree and creates
// /// one downstream_relation edge for each relationship type with non-zero intensity.
// ///
// /// Example TOML structure:
// /// ```toml
// /// [[parent_connections]]
// /// parent_id = "asp_sarah_embodiment_rest"
// /// notes = "Supporting rest and recovery"
// /// [parent_connections.relationship_profile]
// /// instrumental = 0.7
// /// constitutive = 0.3
// /// ```
// ///
// /// This creates two edges:
// /// - (parent_id, Instrumental, {intensity: 0.7, notes: "..."})
// /// - (parent_id, Constitutive, {intensity: 0.3, notes: "..."})
// fn parse_parent_connections(
//     toml_tree: &toml::Value,
// ) -> Result<Vec<(NodeKey, WeightKind, Option<TomlTable>)>, BuildonomyError> {
//     let mut relations = Vec::new();

//     // Extract parent_connections array
//     let connections = match toml_tree.get("parent_connections") {
//         Some(toml::Value::Array(arr)) => arr,
//         Some(_) => {
//             return Err(BuildonomyError::Codec(
//                 "parent_connections must be an array".to_string(),
//             ))
//         }
//         None => return Ok(relations), // No parent_connections is fine
//     };

//     // Process each connection
//     for conn in connections {
//         let conn_table = match conn.as_table() {
//             Some(table) => table,
//             None => continue, // Skip malformed entries
//         };

//         // Extract parent_id
//         let parent_id = match conn_table.get("parent_id").and_then(|v| v.as_str()) {
//             Some(id) => id,
//             None => continue, // Skip connections without parent_id
//         };

//         // Convert parent_id string to NodeKey
//         let parent_key = href_to_nodekey(parent_id);

//         // Extract relationship_profile
//         let profile = match conn_table.get("relationship_profile") {
//             Some(toml::Value::Table(table)) => table,
//             _ => continue, // Skip connections without valid profile
//         };

//         // Create one edge per relationship type with non-zero intensity
//         for (relationship_type, intensity_value) in profile {
//             let intensity = match intensity_value.as_float() {
//                 Some(f) => f,
//                 None => continue, // Skip non-numeric intensities
//             };

//             // Skip zero intensities
//             if intensity == 0.0 {
//                 continue;
//             }

//             // Map relationship_profile field name to WeightKind
//             let weight_kind = match relationship_type.as_str() {
//                 "constitutive" => WeightKind::Constitutive,
//                 "instrumental" => WeightKind::Instrumental,
//                 "tensions_with" => WeightKind::TensionsWith,
//                 "expressive" => WeightKind::Expresses,
//                 // Note: Some profile fields don't map to WeightKind variants
//                 // (exploratory, trades_off, contextual). We'll store these as
//                 // Instrumental for now with the full profile in payload.
//                 "exploratory" | "trades_off" | "contextual" => WeightKind::Instrumental,
//                 _ => continue, // Skip unknown relationship types
//             };

//             // Build edge payload containing the full connection metadata
//             let mut payload = TomlTable::new();
//             payload.insert("intensity".to_string(), toml::Value::Float(intensity));

//             // Include notes if present
//             if let Some(notes) = conn_table.get("notes") {
//                 payload.insert("notes".to_string(), notes.clone());
//             }

//             // Include the full relationship_profile for reference
//             payload.insert(
//                 "relationship_profile".to_string(),
//                 toml::Value::Table(profile.clone()),
//             );

//             relations.push((parent_key.clone(), weight_kind, Some(payload)));
//         }
//     }

//     Ok(relations)
// }

/// Detects the schema type based on the file path.
/// Returns the schema name that can be looked up in the schema registry.
///
/// This function searches path components (directory names) for matches against
/// known schema names from the registry. It looks for the closest/most specific
/// match by checking each path component.
pub fn detect_schema_from_path(path: &str) -> Option<&'static str> {
    use crate::codec::schema_registry::KNOWN_SCHEMAS;

    // Split path into components and search for matches
    let mut path_components: Vec<&str> = path.split('/').collect();

    while let Some(path_part) = path_components.pop() {
        // Check each known schema to see if any of its parts match path components
        for (dir_name, schema_name) in KNOWN_SCHEMAS {
            if path_part == *dir_name {
                return Some(schema_name);
            }
        }
    }

    None
}

#[derive(Clone, Debug, Default)]
pub struct ProtoBeliefNode {
    pub accumulator: Option<String>,
    /// Original TOML content for reference
    pub content: String,
    /// TOML document that preserves key order and formatting
    pub document: DocumentMut,
    pub upstream: Vec<(NodeKey, WeightKind, Option<Weight>)>,
    pub downstream: Vec<(NodeKey, WeightKind, Option<Weight>)>,
    pub path: String,
    pub kind: BeliefKindSet,
    pub errors: Vec<BuildonomyError>,
    pub heading: usize,
}

impl PartialEq for ProtoBeliefNode {
    fn eq(&self, other: &Self) -> bool {
        self.document
            .as_table()
            .to_string()
            .eq(&other.document.as_table().to_string())
            && self.kind.eq(&other.kind)
            && self.upstream.iter().eq(other.upstream.iter())
            && self.downstream.iter().eq(other.downstream.iter())
    }
}

// impl Eq for BeliefSet {}

impl ProtoBeliefNode {
    pub fn new<P: AsRef<Path>>(repo_path: P, path: P) -> Result<ProtoBeliefNode, BuildonomyError> {
        match (repo_path.as_ref().exists(), repo_path.as_ref().is_dir()) {
            (true, true) => Ok(()),
            _ => Err(BuildonomyError::Codec(format!(
                "[ProtoBeliefState::new] Root repository path does not exist and/or is not a \
                 directory: {:?}",
                repo_path.as_ref()
            ))),
        }?;
        let rel_path = match path.as_ref().is_relative() {
            true => path.as_ref().to_path_buf(),
            false => path
                .as_ref()
                .canonicalize()?
                .strip_prefix(repo_path.as_ref())?
                .to_path_buf(),
        };
        let file_path = repo_path.as_ref().join(&rel_path);
        let mut proto = ProtoBeliefNode::from_file(&file_path)?;

        proto.path =
            trim_path_sep(&file_path.strip_prefix(repo_path)?.to_string_lossy()).to_string();
        if let Some(file_stem) = file_path.file_stem() {
            let file_stem_string = file_stem.to_string_lossy().to_string();
            if proto.document.get("bid").is_none() {
                if let Ok(bid) = Bid::try_from(&file_stem_string[..]) {
                    proto.document.insert("bid", value(bid.to_string()));
                }
            }
            if proto.document.get("title").is_none() {
                proto.document.insert("title", value(file_stem_string));
            }
        }
        Ok(proto)
    }

    // Use TomlCodec for schema-aware frontmatter parsing
    // Benefits:
    // 1. Parses parent_connections → downstream
    // 2. Preserves unknown TOML fields for round-trip
    pub fn from_str(str: &str) -> Result<ProtoBeliefNode, BuildonomyError> {
        let mut proto = ProtoBeliefNode::default();
        proto.content = str.trim().to_string();
        proto.document = proto
            .content
            .parse::<DocumentMut>()
            .map_err(|e| BuildonomyError::Codec(format!("Failed to parse TOML: {}", e)))?;
        // Remove/translate BeliefNode fields into a proto node format.
        proto.document.remove("kind");
        if let Some(mut payload) = proto.document.remove("payload") {
            if let Some(table) = payload.as_table_mut() {
                let keys = table
                    .iter()
                    .map(|(k, _)| k.to_string())
                    .collect::<Vec<String>>();
                for key_str in keys {
                    let (key, item) = table
                        .remove_entry(&key_str)
                        .expect("received key_str from table itself.");
                    proto.document.insert_formatted(&key, item);
                }
            }
        }
        Ok(proto)
    }

    /// Parse a file or directory into a ProtoBeliefNode, discovering direct filesystem descendants.
    ///
    /// # Filesystem Discovery Design
    ///
    /// This method handles filesystem traversal to discover a network's direct children.
    /// Per the graph design, each network owns a **flat list** of 'document' or 'network' nodes
    /// that are its **direct filesystem descendants**. This means:
    ///
    /// - **Prune subdirectories** containing `NETWORK_CONFIG_NAME` (they are sub-networks)
    /// - **Flatten all other files** matching CODEC extensions as direct source→sink connections
    /// - The parent network treats the entire non-network filetree as its direct children
    ///
    /// ## Alternative Implementations via Codec Swapping
    ///
    /// This filesystem-based implementation is just one strategy. The `CODECS` map allows
    /// swapping implementations at runtime for different environments:
    ///
    /// - **Native/Desktop**: Use this `ProtoBeliefNode` with direct filesystem access
    /// - **Browser/WASM**: Swap in a `BrowserProtoBeliefNode` that reads from IndexedDB
    /// - **Testing**: Swap in a `MockProtoBeliefNode` with in-memory content
    ///
    /// The codec abstraction provides this flexibility without changing the parser or
    /// accumulator layers:
    ///
    /// ```ignore
    /// // In browser context
    /// CODECS.insert::<BrowserProtoBeliefNode>("toml".to_string());
    ///
    /// // Parser and accumulator work unchanged
    /// let parser = BeliefSetParser::new(entry_point, tx, None)?;
    /// parser.parse_all(cache).await?;
    /// ```
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<ProtoBeliefNode, BuildonomyError> {
        let mut file_path = PathBuf::from(path.as_ref());
        let mut is_net = false;
        if file_path.is_dir() {
            file_path.push(NETWORK_CONFIG_NAME);
        }

        let mut proto = ProtoBeliefNode::default();
        if !file_path.exists() {
            return Err(BuildonomyError::NotFound(format!(
                "Node {:?} does not exist",
                file_path
            )));
        }

        if let Some(file_name) = file_path.file_name() {
            let file_name_string = file_name.to_string_lossy().to_string();
            if file_name_string[..] == NETWORK_CONFIG_NAME[..] {
                is_net = true;
            }
        }

        if is_net {
            let content = fs::read_to_string(&file_path)?;
            let mut file_proto = ProtoBeliefNode::from_str(&content)?;
            proto.merge(&mut file_proto);
            if !proto.document.contains_key("id") {
                return Err(BuildonomyError::Codec(format!(
                    "Network nodes require a semantic ID. Received: {:?}",
                    proto
                )));
            }

            // Enumerate child documents in this network directory
            let network_dir = file_path.parent().ok_or_else(|| {
                BuildonomyError::Codec("Network file has no parent directory".to_string())
            })?;

            // Add Path references for each child document as Subsection relations
            for doc_path in iter_net_docs(path) {
                if let Ok(relative_path) = doc_path.strip_prefix(network_dir) {
                    let path_str = trim_path_sep(&relative_path.to_string_lossy()).to_string();
                    if !path_str.is_empty() {
                        let node_key = NodeKey::Path {
                            net: Bid::nil(), // Will be resolved during processing by calling Key::regularize
                            path: path_str.clone(),
                        };
                        let mut weight = Weight::default();
                        weight.set::<String>(WEIGHT_DOC_PATH, path_str).ok();
                        proto
                            .upstream
                            .push((node_key, WeightKind::Section, Some(weight)));
                    }
                }
            }
            proto.kind.insert(BeliefKind::Network);
            proto.heading = 1;
        } else {
            proto.kind.insert(BeliefKind::Document);
            proto.heading = 2;
        }

        Ok(proto)
    }

    pub fn write<P: AsRef<Path>>(&self, path: P) -> Result<(), BuildonomyError> {
        let file_path = if path.as_ref().is_dir() {
            path.as_ref().join(NETWORK_CONFIG_NAME)
        } else {
            path.as_ref().to_path_buf()
        };
        let mut file = fs::File::create(&file_path)?;
        file.write_all(&self.document.to_string().into_bytes())?;
        Ok(())
    }

    pub fn merge(&mut self, other: &mut ProtoBeliefNode) -> bool {
        let mut changed = false;
        if self.kind != other.kind {
            changed = true;
            self.kind = self.kind.union(other.kind.0).into();
        }

        let mut other_document = replace(&mut other.document, DocumentMut::new());
        let other_document_keys = other_document
            .iter()
            .map(|(k, _)| k.to_string())
            .collect::<Vec<String>>();
        for key_str in other_document_keys.iter() {
            let (key, item) = other_document
                .remove_entry(key_str)
                .expect("Key is from the table itself.");

            let other_unformatted_value = toml_edit_to_toml_value(&item);
            let mut maybe_item = Some(item);
            let maybe_current_item = self.document.get(&key);
            if let Some(current_item) = maybe_current_item {
                let current_unformatted_value = toml_edit_to_toml_value(current_item);
                if current_unformatted_value == other_unformatted_value {
                    maybe_item = None;
                }
            }
            if let Some(item) = maybe_item.take() {
                self.document.insert_formatted(&key, item);
                changed = true;
            }
        }

        let mut other_upstream = replace(&mut other.upstream, Vec::new());
        if self.upstream != other.upstream && !other.upstream.is_empty() {
            self.upstream.append(&mut other_upstream);
            changed = true;
        }

        if self.downstream != other.downstream && !other.downstream.is_empty() {
            let mut other_downstream = replace(&mut other.downstream, Vec::new());
            self.downstream.append(&mut other_downstream);
            changed = true;
        }

        if other.heading != usize::default() {
            self.heading = other.heading;
            changed = true;
        }

        if self.errors != other.errors && !other.errors.is_empty() {
            let mut other_errors = replace(&mut other.errors, Vec::default());
            self.errors.append(&mut other_errors);
            changed = true;
        }

        if self.path != other.path && !other.path.is_empty() {
            self.path = replace(&mut other.path, String::default());
            changed = true;
        }

        changed
    }

    pub fn as_frontmatter(&self) -> String {
        let mut doc = self.document.clone();
        doc.remove("text");
        doc.to_string()
    }

    pub fn as_subsection(&self) -> String {
        let mut doc = self.document.clone();
        doc.remove("text");
        doc.remove("title");
        doc.to_string()
    }

    /// Update the TOML document with values from BeliefContext.
    /// This is used by MdCodec to inject BID/title/id into the frontmatter before serialization.
    /// Uses toml_edit to preserve key order and formatting.
    pub fn update_from_context(
        &mut self,
        ctx: &BeliefContext<'_>,
    ) -> Result<Option<BeliefNode>, BuildonomyError> {
        let mut changed = self.merge(&mut ProtoBeliefNode::try_from(ctx.node)?);
        // We need to fold in and updates to the references stored in our schema so that we can write them out to file here.
        if self.update_schema(&ctx)? {
            changed = true;
        }
        if changed {
            Ok(Some(BeliefNode::try_from(self.deref())?))
        } else {
            Ok(None)
        }
    }

    /// Updates the schema-defined fields in the TOML document based on BeliefContext relationships.
    /// This syncs the document's parent_connections array with the actual graph state.
    fn update_schema(&mut self, _ctx: &BeliefContext<'_>) -> Result<bool, BuildonomyError> {
        // TODO: Implement reverse traversal - sync graph edges back to TOML fields
        // For now, this is a no-op stub
        Ok(false)
    }

    /// Traverses schema-defined graph fields and populates upstream/downstream edge lists.
    /// Uses the schema registry to determine which fields create graph edges.
    pub fn traverse_schema(&mut self) -> Result<(), BuildonomyError> {
        let schema_type = self
            .document
            .get("schema")
            .map(|item| item.as_str().map(|str| str.to_string()))
            .flatten();

        let schema_name = match schema_type {
            Some(name) => name,
            None => return Ok(()), // No schema to traverse
        };
        if self.document.get("schema").is_none() {
            self.document.insert("schema", value(schema_name.clone()));
        }
        // Apply schema migrations before traversal
        // Convert document to toml::Value for migration, then back to toml_edit
        let toml_string = self.document.to_string();
        let mut toml_value: toml::Value = toml::from_str(&toml_string).map_err(|e| {
            BuildonomyError::Codec(format!("Failed to convert to toml::Value: {}", e))
        })?;

        if migrate_schema(&schema_name, &mut toml_value) {
            // Migration occurred - convert back and update document
            let migrated_string = toml::to_string(&toml_value).map_err(|e| {
                BuildonomyError::Codec(format!("Failed to serialize migrated TOML: {}", e))
            })?;
            self.document = migrated_string
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| {
                    BuildonomyError::Codec(format!("Failed to parse migrated TOML: {}", e))
                })?;

            // Update content to reflect the migration (marks as changed for rewrite)
            self.content = self.document.to_string();
        }

        let schema_def = match get_schema_definition(&schema_name) {
            Some(def) => def,
            None => return Ok(()), // Schema not found in registry
        };

        // Traverse each graph field defined in the schema using toml::Value (simpler than toml_edit)
        for graph_field in schema_def.graph_fields.iter() {
            let field_value = match toml_value.get(graph_field.field_name) {
                Some(v) => v,
                None => {
                    if graph_field.required {
                        tracing::warn!("Field '{}' not found in document", graph_field.field_name);
                    }
                    continue; // Field not present in this document
                }
            };

            let weight_kind = graph_field.weight_kind;

            // Handle both naked references (strings) and full objects (tables/arrays)
            if let Some(array) = field_value.as_array() {
                for item in array {
                    // Each item could be a string (naked reference) or a table (full object)
                    let (node_key, payload) = if let Some(id_str) = item.as_str() {
                        // Naked reference: parse using NodeKey::from_str to handle all formats
                        (
                            NodeKey::from_str(id_str).unwrap_or_else(|_| NodeKey::Id {
                                net: Bid::nil(),
                                id: id_str.to_string(),
                            }),
                            None,
                        )
                    } else if let Some(table) = item.as_table() {
                        // Full object: extract parent_id and build payload from other fields
                        let id_str = match table.get("parent_id").and_then(|v| v.as_str()) {
                            Some(s) => s,
                            None => continue, // Skip if no parent_id
                        };

                        // Build payload from specified fields
                        let mut payload_table = TomlTable::new();
                        for payload_field in graph_field.payload_fields.iter() {
                            if let Some(payload_value) = table.get(*payload_field) {
                                payload_table
                                    .insert(payload_field.to_string(), payload_value.clone());
                            }
                        }

                        let payload = if payload_table.is_empty() {
                            None
                        } else {
                            Some(Weight {
                                payload: payload_table,
                            })
                        };

                        (
                            NodeKey::from_str(id_str).unwrap_or_else(|_| NodeKey::Id {
                                net: Bid::nil(),
                                id: id_str.to_string(),
                            }),
                            payload,
                        )
                    } else {
                        tracing::warn!("unknown item type! Received item: {:?}", item);
                        continue; // Unknown item type
                    };
                    // Add to appropriate edge list based on direction enum
                    match graph_field.direction {
                        EdgeDirection::Downstream => {
                            self.downstream.push((node_key, weight_kind, payload));
                        }
                        EdgeDirection::Upstream => {
                            self.upstream.push((node_key, weight_kind, payload));
                        }
                    }
                }
            } else if let Some(id_str) = field_value.as_str() {
                // Single naked reference (not in an array)
                let node_key = NodeKey::from_str(id_str).unwrap_or_else(|_| NodeKey::Id {
                    net: Bid::nil(),
                    id: id_str.to_string(),
                });
                match graph_field.direction {
                    EdgeDirection::Downstream => {
                        self.downstream.push((node_key, weight_kind, None));
                    }
                    EdgeDirection::Upstream => {
                        self.upstream.push((node_key, weight_kind, None));
                    }
                }
            }
        }

        Ok(())
    }
}

impl TryFrom<&BeliefNode> for ProtoBeliefNode {
    type Error = BuildonomyError;

    fn try_from(src: &BeliefNode) -> Result<Self, Self::Error> {
        let content = to_string(src)?;
        let mut proto = ProtoBeliefNode::from_str(&content)?;
        proto.kind = src.kind.clone();
        Ok(proto)
    }
}

impl DocCodec for ProtoBeliefNode {
    /// Returns all ProtoBeliefNodes parsed from the TOML document.
    fn nodes(&self) -> Vec<ProtoBeliefNode> {
        vec![self.clone()]
    }

    fn inject_context(
        &mut self,
        _node: &ProtoBeliefNode,
        ctx: &BeliefContext<'_>,
    ) -> Result<Option<BeliefNode>, BuildonomyError> {
        // For standalone TOML files, just delegate to update_from_context
        self.update_from_context(ctx)
    }

    fn generate_source(&self) -> Option<String> {
        Some(self.document.to_string())
    }

    fn parse(&mut self, content: String, current: ProtoBeliefNode) -> Result<(), BuildonomyError> {
        let mut content_proto = ProtoBeliefNode::from_str(&content)?;
        *self = current;
        self.merge(&mut content_proto);
        if self.document.get("schema").is_none() {
            let maybe_path_schema = detect_schema_from_path(&self.path);
            if let Some(path_schema) = maybe_path_schema {
                self.document.insert("schema", value(path_schema));
            }
        }
        self.traverse_schema()?;

        Ok(())
    }
}
