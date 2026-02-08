use crate::{
    beliefbase::BeliefContext,
    codec::{
        schema_registry::{migrate_schema, EdgeDirection, SCHEMAS},
        DocCodec, CODECS,
    },
    error::BuildonomyError,
    nodekey::{trim_path_sep, NodeKey},
    paths::{path_extension, path_parent},
    properties::{BeliefKind, BeliefKindSet, BeliefNode, Bid, Weight, WeightKind},
};

use std::{
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

/// Metadata format for document frontmatter and network configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFormat {
    Json,
    Toml,
    Yaml,
}

/// Standard filenames designating a directory as the root of a BeliefNetwork.
/// Priority order: YAML → JSON → TOML for consistency with metadata parsing.
/// Supports extension synonyms: .yaml/.yml, .json/.jsn, .toml/.tml
pub const NETWORK_CONFIG_NAMES: &[&str] = &[
    "BeliefNetwork.yaml",
    "BeliefNetwork.yml",
    "BeliefNetwork.json",
    "BeliefNetwork.jsn",
    "BeliefNetwork.toml",
    "BeliefNetwork.tml",
];

/// Iterates through a directory subtree, filtering to return a sorted list of network directories
/// (directories containing a BeliefNetwork.json or BeliefNetwork.toml file), as well as file paths
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
                if p.extension()
                    .and_then(|e| e.to_str())
                    .filter(|&e| CODECS.extensions().iter().any(|ce| ce.as_str() == e))
                    .is_some()
                {
                    if subnets.iter().any(|subnet_path| p.starts_with(subnet_path)) {
                        // Don't include subnet files
                        None
                    } else if let Some(file_name) = p.file_name() {
                        let file_name_str = file_name.to_string_lossy();
                        if NETWORK_CONFIG_NAMES
                            .iter()
                            .any(|&name| name == file_name_str.as_ref())
                        {
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
        let converted: Option<Vec<toml::Value>> =
            arr.iter().map(toml_edit_value_to_toml_value).collect();
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
        let converted: Option<Vec<toml::Value>> =
            arr.iter().map(toml_edit_value_to_toml_value).collect();
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

/// Network-level configuration for metadata format preferences
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub default_metadata_format: MetadataFormat,
    pub strict_format: bool,
    pub validate_on_parse: bool,
    pub auto_normalize: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            default_metadata_format: MetadataFormat::Json,
            strict_format: false,
            validate_on_parse: true,
            auto_normalize: true,
        }
    }
}

impl NetworkConfig {
    /// Extract network config from a TOML document
    pub fn from_document(doc: &DocumentMut) -> Option<Self> {
        let config_item = doc.get("config")?;
        let config_table = config_item.as_table()?;

        let default_format = config_table
            .get("default_metadata_format")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "json" => Some(MetadataFormat::Json),
                "toml" => Some(MetadataFormat::Toml),
                _ => {
                    tracing::warn!("Unknown metadata format '{}', defaulting to JSON", s);
                    None
                }
            })
            .unwrap_or(MetadataFormat::Json);

        let strict_format = config_table
            .get("strict_format")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let validate_on_parse = config_table
            .get("validate_on_parse")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let auto_normalize = config_table
            .get("auto_normalize")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Some(NetworkConfig {
            default_metadata_format: default_format,
            strict_format,
            validate_on_parse,
            auto_normalize,
        })
    }
}

/// Parse content as JSON and convert to TOML DocumentMut
fn parse_json_to_document(json_str: &str) -> Result<DocumentMut, BuildonomyError> {
    // Parse JSON string to serde_json::Value
    let json_value: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| BuildonomyError::Codec(format!("Failed to parse JSON: {e}")))?;

    // Convert JSON to TOML via intermediate serialization
    // This handles type conversions (null, datetime, etc.)
    let toml_string = json_to_toml_string(&json_value)?;

    // Parse as TOML DocumentMut
    toml_string
        .parse::<DocumentMut>()
        .map_err(|e| BuildonomyError::Codec(format!("Failed to convert JSON to TOML: {e}")))
}

/// Parse content as TOML DocumentMut
fn parse_toml_to_document(toml_str: &str) -> Result<DocumentMut, BuildonomyError> {
    toml_str
        .parse::<DocumentMut>()
        .map_err(|e| BuildonomyError::Codec(format!("Failed to parse TOML: {e}")))
}

/// Parse content as YAML and convert to TOML DocumentMut
fn parse_yaml_to_document(yaml_str: &str) -> Result<DocumentMut, BuildonomyError> {
    // Parse YAML string to serde_json::Value (serde_yaml::Value is compatible)
    let yaml_value: serde_json::Value = serde_yaml::from_str(yaml_str)
        .map_err(|e| BuildonomyError::Codec(format!("Failed to parse YAML: {e}")))?;

    // Convert YAML (as JSON Value) to TOML via intermediate serialization
    let toml_string = json_to_toml_string(&yaml_value)?;

    // Parse as TOML DocumentMut
    toml_string
        .parse::<DocumentMut>()
        .map_err(|e| BuildonomyError::Codec(format!("Failed to convert YAML to TOML: {e}")))
}

/// Convert JSON value to TOML string
fn json_to_toml_string(json: &serde_json::Value) -> Result<String, BuildonomyError> {
    // Convert JSON to TOML via toml::Value
    let toml_value = json_value_to_toml_value(json)?;
    toml::to_string(&toml_value)
        .map_err(|e| BuildonomyError::Codec(format!("Failed to serialize to TOML: {e}")))
}

/// Convert serde_json::Value to toml::Value
fn json_value_to_toml_value(json: &serde_json::Value) -> Result<toml::Value, BuildonomyError> {
    match json {
        serde_json::Value::Null => {
            // TOML doesn't have null - skip or use empty string
            // For now, treat as empty string to preserve structure
            Ok(toml::Value::String(String::new()))
        }
        serde_json::Value::Bool(b) => Ok(toml::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(toml::Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(toml::Value::Float(f))
            } else {
                Err(BuildonomyError::Codec(format!(
                    "Unsupported JSON number: {n}"
                )))
            }
        }
        serde_json::Value::String(s) => Ok(toml::Value::String(s.clone())),
        serde_json::Value::Array(arr) => {
            let toml_arr: Result<Vec<toml::Value>, BuildonomyError> =
                arr.iter().map(json_value_to_toml_value).collect();
            Ok(toml::Value::Array(toml_arr?))
        }
        serde_json::Value::Object(obj) => {
            let mut toml_table = toml::map::Map::new();
            for (key, value) in obj {
                // Skip null values in objects
                if !value.is_null() {
                    toml_table.insert(key.clone(), json_value_to_toml_value(value)?);
                }
            }
            Ok(toml::Value::Table(toml_table))
        }
    }
}

/// Parse content with format preference and three-way fallback
fn parse_with_fallback(
    content: &str,
    primary: MetadataFormat,
) -> Result<DocumentMut, BuildonomyError> {
    match primary {
        MetadataFormat::Json => {
            // Try JSON first
            match parse_json_to_document(content) {
                Ok(doc) => {
                    tracing::debug!("Parsed as JSON");
                    Ok(doc)
                }
                Err(json_err) => {
                    // tracing::debug!("JSON parsing failed, trying YAML fallback");
                    match parse_yaml_to_document(content) {
                        Ok(doc) => {
                            tracing::debug!("Parsed as YAML");
                            Ok(doc)},
                        Err(yaml_err) => {
                            match parse_toml_to_document(content) {
                                Ok(doc) => {
                                    tracing::debug!("Parsed as TOML");
                                    Ok(doc)},
                                Err(toml_err) => Err(BuildonomyError::Codec(format!(
                                    "Failed to parse as JSON, YAML, or TOML.\nJSON: {json_err}\nYAML: {yaml_err}\nTOML: {toml_err}"
                                ))),
                            }
                        }
                    }
                }
            }
        }
        MetadataFormat::Toml => {
            // Try TOML first
            match parse_toml_to_document(content) {
                Ok(doc) => {
                    tracing::debug!("Parsed as TOML");
                    Ok(doc)
                }
                Err(toml_err) => {
                    match parse_yaml_to_document(content) {
                        Ok(doc) => {
                            tracing::debug!("Parsed as YAML");
                            Ok(doc)},
                        Err(yaml_err) => {
                            match parse_json_to_document(content) {
                                Ok(doc) => {
                                    tracing::debug!("Parsed as JSON");
                                    Ok(doc)
                                },
                                Err(json_err) => Err(BuildonomyError::Codec(format!(
                                    "Failed to parse as TOML, YAML, or JSON.\nTOML: {toml_err}\nYAML: {yaml_err}\nJSON: {json_err}"
                                ))),
                            }
                        }
                    }
                }
            }
        }
        MetadataFormat::Yaml => {
            // Try YAML first
            match parse_yaml_to_document(content) {
                Ok(doc) => {
                    tracing::debug!("Parsed as YAML");
                    Ok(doc)
                }
                Err(yaml_err) => {
                    tracing::debug!("YAML parsing failed, trying JSON fallback");
                    match parse_json_to_document(content) {
                        Ok(doc) => Ok(doc),
                        Err(json_err) => {
                            tracing::debug!("JSON parsing failed, trying TOML fallback");
                            match parse_toml_to_document(content) {
                                Ok(doc) => Ok(doc),
                                Err(toml_err) => Err(BuildonomyError::Codec(format!(
                                    "Failed to parse as YAML, JSON, or TOML.\nYAML: {yaml_err}\nJSON: {json_err}\nTOML: {toml_err}"
                                ))),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Detect network file in directory and return (path, format)
///
/// Priority order: YAML → JSON → TOML (consistent with metadata parsing)
/// Supports extension synonyms for all three formats.
pub fn detect_network_file(dir: &Path) -> Option<(PathBuf, MetadataFormat)> {
    // Helper to map extension to format
    let extension_to_format = |ext: &str| -> Option<MetadataFormat> {
        match ext {
            "json" | "jsn" => Some(MetadataFormat::Json),
            "yaml" | "yml" => Some(MetadataFormat::Yaml),
            "toml" | "tml" => Some(MetadataFormat::Toml),
            _ => None,
        }
    };

    // Check each network config name in priority order
    for filename in NETWORK_CONFIG_NAMES {
        let path = dir.join(filename);
        if path.exists() {
            // Extract extension and map to format
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if let Some(format) = extension_to_format(ext) {
                    return Some((path, format));
                }
            }
        }
    }

    None
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

/// Builds a title attribute for HTML links containing bref and optional metadata.
///
/// The title attribute format is: "bref://[bref] [metadata] [user_words]"
/// where metadata and user_words are optional.
///
/// # Arguments
/// * `bref` - The bref string (should already include "bref://" prefix)
/// * `auto_title` - If true, adds {"auto_title":true} metadata
/// * `user_words` - Optional user-provided text to append
///
/// # Examples
/// ```
/// use noet_core::codec::belief_ir::build_title_attribute;
/// let attr = build_title_attribute("bref://abc123", false, None);
/// assert_eq!(attr, "bref://abc123");
///
/// let attr = build_title_attribute("bref://abc123", true, Some("My Note"));
/// assert_eq!(attr, "bref://abc123 {\"auto_title\":true} My Note");
/// ```
pub fn build_title_attribute(bref: &str, auto_title: bool, user_words: Option<&str>) -> String {
    let mut parts = vec![bref.to_string()];

    if auto_title {
        parts.push("{\"auto_title\":true}".to_string());
    }

    if let Some(words) = user_words {
        parts.push(words.to_string());
    }

    parts.join(" ")
}

/// Detects the schema type based on the file path.
/// Returns the schema name that can be looked up in the schema registry.
///
/// This function searches path components (directory names) for matches against
/// known schema names from the registry. It looks for the closest/most specific
/// match by checking each path component.
pub fn detect_schema_from_path(path: &str) -> Option<String> {
    // Split path into components and search for matches
    let mut path_components: Vec<&str> = path.split('/').collect();

    while let Some(path_part) = path_components.pop() {
        // Check each known schema to see if any of its parts match path components
        for schema_name in SCHEMAS.list_schemas().into_iter() {
            if path_part == schema_name {
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
    /// Explicit ID from heading anchor syntax (e.g., {#my-id})
    /// This is the raw, unnormalized ID as parsed from markdown
    pub id: Option<String>,
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

// impl Eq for BeliefBase {}

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

    /// Parse a file or directory into a ProtoBeliefNode, discovering direct filesystem descendants.
    ///
    /// # Filesystem Discovery Design
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
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<ProtoBeliefNode, BuildonomyError> {
        let mut file_path = PathBuf::from(path.as_ref());
        let mut is_net = false;
        if file_path.is_dir() {
            // Try to detect network file (JSON or TOML)
            if let Some((detected_path, _format)) = detect_network_file(&file_path) {
                file_path = detected_path;
            } else {
                // Default to first in NETWORK_CONFIG_NAMES (JSON)
                file_path.push(NETWORK_CONFIG_NAMES[0]);
            }
        }

        let mut proto = ProtoBeliefNode::default();
        if !file_path.exists() {
            return Err(BuildonomyError::NotFound(format!(
                "Node {file_path:?} does not exist"
            )));
        }

        if let Some(file_name) = file_path.file_name() {
            let file_name_string = file_name.to_string_lossy();
            if NETWORK_CONFIG_NAMES
                .iter()
                .any(|&name| name == file_name_string.as_ref())
            {
                is_net = true;
            }
        }

        if is_net {
            let content = fs::read_to_string(&file_path)?;
            let mut file_proto = ProtoBeliefNode::from_str(&content)?;
            proto.merge(&mut file_proto);
            if !proto.document.contains_key("id") {
                return Err(BuildonomyError::Codec(format!(
                    "Network nodes require a semantic ID. Received: {proto:?}"
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
                        weight.set_doc_paths(vec![path_str]).ok();
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
            // Detect existing network file or default to JSON
            if let Some((detected_path, _format)) = detect_network_file(path.as_ref()) {
                detected_path
            } else {
                // Default to first in NETWORK_CONFIG_NAMES (JSON)
                path.as_ref().join(NETWORK_CONFIG_NAMES[0])
            }
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

        let mut other_upstream = std::mem::take(&mut other.upstream);
        if self.upstream != other.upstream && !other.upstream.is_empty() {
            self.upstream.append(&mut other_upstream);
            changed = true;
        }

        if self.downstream != other.downstream && !other.downstream.is_empty() {
            let mut other_downstream = std::mem::take(&mut other.downstream);
            self.downstream.append(&mut other_downstream);
            changed = true;
        }

        if other.heading != usize::default() {
            self.heading = other.heading;
            changed = true;
        }

        if self.errors != other.errors && !other.errors.is_empty() {
            let mut other_errors = std::mem::take(&mut other.errors);
            self.errors.append(&mut other_errors);
            changed = true;
        }

        if self.path != other.path && !other.path.is_empty() {
            self.path = std::mem::take(&mut other.path);
            changed = true;
        }

        // Merge id field - this is critical for collision detection
        // If other.id is None, it means the ID was cleared due to collision
        if self.id != other.id {
            self.id = other.id.clone();
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
        // I don't think this needs to be tracked as a change
        self.path = ctx.relative_path.clone();
        // We need to fold in and updates to the references stored in our schema so that we can write them out to file here.
        if self.update_schema(ctx)? {
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
            .and_then(|item| item.as_str().map(|str| str.to_string()));

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
            BuildonomyError::Codec(format!("Failed to convert to toml::Value: {e}"))
        })?;

        if migrate_schema(&schema_name, &mut toml_value) {
            // Migration occurred - convert back and update document
            let migrated_string = toml::to_string(&toml_value).map_err(|e| {
                BuildonomyError::Codec(format!("Failed to serialize migrated TOML: {e}"))
            })?;
            self.document = migrated_string
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| {
                    BuildonomyError::Codec(format!("Failed to parse migrated TOML: {e}"))
                })?;

            // Update content to reflect the migration (marks as changed for rewrite)
            self.content = self.document.to_string();
        }

        let schema_def = match SCHEMAS.get(&schema_name) {
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

impl FromStr for ProtoBeliefNode {
    type Err = BuildonomyError;
    // Use JSON-first parsing with TOML fallback for cross-platform compatibility
    // Benefits:
    // 1. Parses parent_connections → downstream
    // 2. Preserves unknown fields for round-trip
    // 3. JSON default enables browser/web tool compatibility
    fn from_str(str: &str) -> Result<ProtoBeliefNode, BuildonomyError> {
        Self::from_str_with_format(str, MetadataFormat::Json)
    }
}

impl ProtoBeliefNode {
    /// Parse content with explicit format preference
    pub fn from_str_with_format(
        str: &str,
        preferred_format: MetadataFormat,
    ) -> Result<ProtoBeliefNode, BuildonomyError> {
        let mut proto = ProtoBeliefNode::default();
        proto.content = str.trim().to_string();

        // Parse with format preference and fallback
        proto.document = parse_with_fallback(&proto.content, preferred_format)?;

        // Validate reserved BIDs - user files cannot use BIDs in the Buildonomy API namespace
        if let Some(bid_value) = proto.document.get("bid") {
            if let Some(bid_str) = bid_value.as_str() {
                if let Ok(bid) = crate::properties::Bid::try_from(bid_str) {
                    if bid.is_reserved() {
                        return Err(BuildonomyError::Codec(format!(
                            "BID '{}' is reserved for system use (falls within Buildonomy API namespace) and cannot be used in user files. \
                             Reserved BIDs include UUID_NAMESPACE_BUILDONOMY, UUID_NAMESPACE_HREF, and all BIDs derived from the Buildonomy namespace. \
                             Please remove the 'bid' field to auto-generate a unique BID, or use a different UUID outside the reserved namespace.",
                            bid_str
                        )));
                    }
                }
            }
        }

        // Validate reserved IDs
        if let Some(id_value) = proto.document.get("id") {
            if let Some(id_str) = id_value.as_str() {
                if id_str == "buildonomy_api" {
                    return Err(BuildonomyError::Codec("ID 'buildonomy_api' is reserved for the system API node and cannot be used in user files. \
                         Please choose a different ID that does not start with 'buildonomy_'.".to_string()));
                }
                if id_str == "buildonomy_href_network" {
                    return Err(BuildonomyError::Codec("ID 'buildonomy_href_network' is reserved for the system href tracking network and cannot be used in user files. \
                         Please choose a different ID that does not start with 'buildonomy_'.".to_string()));
                }
                if id_str.starts_with("buildonomy_") {
                    return Err(BuildonomyError::Codec(format!(
                        "ID '{}' uses the reserved 'buildonomy_' prefix which is reserved for system use. \
                         Please choose a different ID that does not start with 'buildonomy_'.",
                        id_str
                    )));
                }
            }
        }

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

    fn should_defer(&self) -> bool {
        // Networks need full context to list child documents
        self.kind.contains(BeliefKind::Network)
    }

    fn generate_deferred_html(
        &self,
        ctx: &crate::beliefbase::BeliefContext<'_>,
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

        // Generate HTML list of child documents
        let title = ctx.node.display_title();

        let mut html = String::new();
        html.push_str(&format!("<h1>{} Index</h1>\n", title));

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
                let mut link_path = edge.relative_path.clone();

                // Normalize document links to .html extension
                let codec_extensions = crate::codec::CODECS.extensions();
                for ext in codec_extensions.iter() {
                    if path_extension(&link_path)
                        .filter(|link_ext| link_ext == ext)
                        .is_some()
                    {
                        link_path = link_path.replace(&format!(".{}", ext), ".html");
                        break;
                    }
                }

                let title = edge.other.display_title();
                let parent_dir = path_parent(link_path.as_ref()).to_string();
                if parent_dir.is_empty() {
                    if last_subdir.is_some() {
                        last_subdir = None;
                        html.push_str("</ul>");
                    }
                } else {
                    if let Some(ref last_dir) = last_subdir {
                        if &parent_dir != last_dir {
                            html.push_str("</ul><ul>");
                            last_subdir = Some(parent_dir);
                        }
                    }
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
                html.push_str("</ul>\n");
            }
            html.push_str("</ul>\n");
        }

        // Output filename is index.html (caller handles directory path)
        Ok(vec![("index.html".to_string(), html)])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_format() {
        let json_content = r#"{
            "bid": "12345678-1234-1234-1234-123456789abc",
            "schema": "intention_lattice.intention",
            "title": "Test Node",
            "parent_connections": []
        }"#;

        let result = ProtoBeliefNode::from_str(json_content);
        assert!(result.is_ok(), "JSON parsing should succeed");

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("title").and_then(|v| v.as_str()),
            Some("Test Node")
        );
    }

    #[test]
    fn test_parse_toml_format() {
        let toml_content = r#"
bid = "12345678-1234-1234-1234-123456789abc"
schema = "intention_lattice.intention"
title = "Test Node"
parent_connections = []
"#;

        let result = ProtoBeliefNode::from_str(toml_content);
        assert!(result.is_ok(), "TOML parsing should succeed via fallback");

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("title").and_then(|v| v.as_str()),
            Some("Test Node")
        );
    }

    #[test]
    fn test_parse_with_format_json_first() {
        let json_content = r#"{"title": "JSON Test"}"#;

        let result = ProtoBeliefNode::from_str_with_format(json_content, MetadataFormat::Json);
        assert!(result.is_ok());

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("title").and_then(|v| v.as_str()),
            Some("JSON Test")
        );
    }

    #[test]
    fn test_parse_with_format_toml_first() {
        let toml_content = r#"title = "TOML Test""#;

        let result = ProtoBeliefNode::from_str_with_format(toml_content, MetadataFormat::Toml);
        assert!(result.is_ok());

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("title").and_then(|v| v.as_str()),
            Some("TOML Test")
        );
    }

    #[test]
    fn test_json_to_toml_conversion() {
        let json_value = serde_json::json!({
            "string": "hello",
            "number": 42,
            "float": 3.0123,
            "bool": true,
            "array": [1, 2, 3],
            "null": null
        });

        let toml_value = json_value_to_toml_value(&json_value);
        assert!(toml_value.is_ok());

        let toml = toml_value.unwrap();
        assert_eq!(toml.get("string").and_then(|v| v.as_str()), Some("hello"));
        assert_eq!(toml.get("number").and_then(|v| v.as_integer()), Some(42));
        assert_eq!(toml.get("bool").and_then(|v| v.as_bool()), Some(true));
        // null values are skipped in TOML (TOML doesn't support null)
        assert_eq!(toml.get("null"), None);
    }

    #[test]
    fn test_network_config_extraction() {
        let toml_str = r#"
bid = "12345678-1234-1234-1234-123456789abc"
schema = "noet.network_config"

[config]
default_metadata_format = "toml"
strict_format = true
validate_on_parse = false
auto_normalize = true
"#;

        let document = toml_str.parse::<DocumentMut>().unwrap();
        let config = NetworkConfig::from_document(&document);

        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.default_metadata_format, MetadataFormat::Toml);
        assert!(config.strict_format);
        assert!(!config.validate_on_parse);
        assert!(config.auto_normalize);
    }

    #[test]
    fn test_network_config_defaults() {
        let toml_str = r#"
bid = "12345678-1234-1234-1234-123456789abc"
schema = "noet.network_config"

[config]
"#;

        let document = toml_str.parse::<DocumentMut>().unwrap();
        let config = NetworkConfig::from_document(&document);

        assert!(config.is_some());
        let config = config.unwrap();
        // Should use defaults
        assert_eq!(config.default_metadata_format, MetadataFormat::Json);
        assert!(!config.strict_format);
        assert!(config.validate_on_parse);
        assert!(config.auto_normalize);
    }

    #[test]
    fn test_detect_network_file_json() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let json_path = temp_dir.path().join("BeliefNetwork.json");
        fs::write(&json_path, r#"{"bid": "test"}"#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(path, json_path);
        assert_eq!(format, MetadataFormat::Json);
    }

    #[test]
    fn test_detect_network_file_toml() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let toml_path = temp_dir.path().join("BeliefNetwork.toml");
        fs::write(&toml_path, r#"bid = "test""#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(path, toml_path);
        assert_eq!(format, MetadataFormat::Toml);
    }

    #[test]
    fn test_detect_network_file_prefers_yaml() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let yaml_path = temp_dir.path().join("BeliefNetwork.yaml");
        let json_path = temp_dir.path().join("BeliefNetwork.json");
        let toml_path = temp_dir.path().join("BeliefNetwork.toml");

        fs::write(&yaml_path, r#"bid: "yaml""#).unwrap();
        fs::write(&json_path, r#"{"bid": "json"}"#).unwrap();
        fs::write(&toml_path, r#"bid = "toml""#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(path, yaml_path, "Should prefer YAML when all three exist");
        assert_eq!(format, MetadataFormat::Yaml);
    }

    #[test]
    fn test_detect_network_file_prefers_json_over_toml() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let json_path = temp_dir.path().join("BeliefNetwork.json");
        let toml_path = temp_dir.path().join("BeliefNetwork.toml");

        fs::write(&json_path, r#"{"bid": "json"}"#).unwrap();
        fs::write(&toml_path, r#"bid = "toml""#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(
            path, json_path,
            "Should prefer JSON over TOML when YAML absent"
        );
        assert_eq!(format, MetadataFormat::Json);
    }

    #[test]
    fn test_parse_fallback_all_formats_invalid() {
        let invalid_content = "this is not valid YAML, JSON or TOML {]";

        let result = ProtoBeliefNode::from_str(invalid_content);
        assert!(result.is_err());

        let err = result.unwrap_err();
        let err_msg = format!("{err}");
        // Should mention all three formats in error message
        assert!(err_msg.contains("YAML") || err_msg.contains("yaml"));
        assert!(err_msg.contains("JSON") || err_msg.contains("json"));
        assert!(err_msg.contains("TOML") || err_msg.contains("toml"));
    }

    #[test]
    fn test_detect_network_file_yml_extension() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let yml_path = temp_dir.path().join("BeliefNetwork.yml");

        fs::write(&yml_path, r#"bid: "yaml-synonym""#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(path, yml_path, ".yml extension should be recognized");
        assert_eq!(format, MetadataFormat::Yaml);
    }

    #[test]
    fn test_detect_network_file_jsn_extension() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let jsn_path = temp_dir.path().join("BeliefNetwork.jsn");

        fs::write(&jsn_path, r#"{"bid": "json-synonym"}"#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(path, jsn_path, ".jsn extension should be recognized");
        assert_eq!(format, MetadataFormat::Json);
    }

    #[test]
    fn test_detect_network_file_tml_extension() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let tml_path = temp_dir.path().join("BeliefNetwork.tml");

        fs::write(&tml_path, r#"bid = "toml-synonym""#).unwrap();

        let result = detect_network_file(temp_dir.path());
        assert!(result.is_some());

        let (path, format) = result.unwrap();
        assert_eq!(path, tml_path, ".tml extension should be recognized");
        assert_eq!(format, MetadataFormat::Toml);
    }
    #[test]
    fn test_reserved_bid_namespace_buildonomy() {
        let toml = r#"
    bid = "6b3d2154-c0a9-437b-9324-5f62adeb9a44"
    id = "test-node"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
        assert!(err_msg.contains("Buildonomy API namespace"));
    }

    #[test]
    fn test_reserved_bid_namespace_href() {
        let toml = r#"
    bid = "5b3d2154-c0a9-437b-9324-5f62adeb9a44"
    id = "test-node"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
    }

    #[test]
    fn test_reserved_bid_derived_from_namespace() {
        // Test that a BID derived via buildonomy_api_bid() is also rejected
        let derived_bid = crate::properties::buildonomy_api_bid("0.1.0");
        let toml = format!(
            r#"
    bid = "{}"
    id = "test-node"
    title = "Test"
    "#,
            derived_bid
        );
        let result = ProtoBeliefNode::from_str(&toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
    }

    #[test]
    fn test_reserved_id_buildonomy_api() {
        let toml = r#"
    id = "buildonomy_api"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
        assert!(err_msg.contains("buildonomy_api"));
    }

    #[test]
    fn test_reserved_id_buildonomy_href_network() {
        let toml = r#"
    id = "buildonomy_href_network"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
    }

    #[test]
    fn test_reserved_id_buildonomy_prefix() {
        let toml = r#"
    id = "buildonomy_custom"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
        assert!(err_msg.contains("buildonomy_"));
    }

    #[test]
    fn test_non_reserved_ids_allowed() {
        let toml = r#"
    id = "my-custom-node"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_reserved_bids_allowed() {
        let toml = r#"
    bid = "a065d82c-9d68-4470-be02-028fb6c507c0"
    id = "my-custom-node"
    title = "Test"
    "#;
        let result = ProtoBeliefNode::from_str(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_yaml_format() {
        let yaml_content = r#"
bid: "12345678-1234-1234-1234-123456789abc"
schema: "intention_lattice.intention"
title: "Test YAML Node"
parent_connections: []
"#;

        let result = ProtoBeliefNode::from_str_with_format(yaml_content, MetadataFormat::Yaml);
        assert!(result.is_ok(), "YAML parsing should succeed");

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("title").and_then(|v| v.as_str()),
            Some("Test YAML Node")
        );
    }

    #[test]
    fn test_parse_with_format_yaml_first() {
        let yaml_content = r#"
bid: "yaml-test"
schema: "test.schema"
title: "YAML First Test"
"#;

        let result = ProtoBeliefNode::from_str_with_format(yaml_content, MetadataFormat::Yaml);
        assert!(result.is_ok());

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("bid").and_then(|v| v.as_str()),
            Some("yaml-test")
        );
    }
}
