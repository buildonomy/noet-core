use crate::{
    beliefbase::BeliefContext,
    codec::schema_registry::{migrate_schema, EdgeDirection, SCHEMAS},
    error::BuildonomyError,
    nodekey::NodeKey,
    paths::to_anchor,
    properties::{BeliefKindSet, BeliefNode, Bid, Bref, NodeId, Weight, WeightKind},
};

use std::{mem::replace, ops::Deref, str::FromStr};
use toml::{to_string, Table as TomlTable};
use toml_edit::{value, DocumentMut};

/// Metadata format for document frontmatter and network configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFormat {
    Json,
    Toml,
    Yaml,
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
/// Convert a `toml::Value` to a `serde_json::Value`.
///
/// This is the canonical implementation shared by `wasm.rs::extract_node_context`
/// and `mcp::tools`. The inverse direction is [`json_value_to_toml_value`].
#[cfg(feature = "mcp")]
pub(crate) fn toml_value_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(t) => serde_json::Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
                .collect(),
        ),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

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
pub(crate) fn parse_with_fallback(
    content: &str,
    primary: MetadataFormat,
) -> Result<DocumentMut, BuildonomyError> {
    match primary {
        MetadataFormat::Json => {
            // Try JSON first
            match parse_json_to_document(content) {
                Ok(doc) => Ok(doc),
                Err(json_err) => {
                    // tracing::debug!("JSON parsing failed, trying YAML fallback");
                    match parse_yaml_to_document(content) {
                        Ok(doc) => {
                            Ok(doc)},
                        Err(yaml_err) => {
                            match parse_toml_to_document(content) {
                                Ok(doc) => {
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
                    Ok(doc)
                }
                Err(toml_err) => {
                    match parse_yaml_to_document(content) {
                        Ok(doc) => {
                            Ok(doc)},
                        Err(yaml_err) => {
                            match parse_json_to_document(content) {
                                Ok(doc) => {
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
                Ok(doc) => Ok(doc),
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

/// A single relation entry in an [`IRNode`]'s upstream or downstream list.
///
/// Carries the target key, weight metadata, and — when available — the source
/// location in the document where the relation was declared. The location is used
/// to produce precise diagnostic messages when the relation cannot be resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateRelation {
    /// The key identifying the other node in this relation.
    pub key: NodeKey,
    /// Additional keys to try if the primary key fails to resolve.
    /// These are appended to the cache_fetch key list after the primary key
    /// (and any proximity-narrowed variants). Used by wikilinks to add a
    /// relative-path fallback alongside the primary Id key.
    pub fallback_keys: Vec<NodeKey>,
    /// The weight kind (edge type) of this relation.
    pub kind: WeightKind,
    /// Optional weight payload (e.g., title text, sort data).
    pub weight: Option<Weight>,
    /// Byte offset into the source document where this relation was declared, if known.
    /// Populated by codec parsers that have access to byte ranges (e.g., `MdCodec`).
    /// `None` for relations derived from serialized data (TOML schema fields).
    /// Convert to `(line, col)` at display time via [`crate::codec::byte_offset_to_location`].
    pub location: Option<usize>,
}

impl IntermediateRelation {
    pub fn new(key: NodeKey, kind: WeightKind, weight: Option<Weight>) -> Self {
        Self {
            key,
            fallback_keys: Vec::new(),
            kind,
            weight,
            location: None,
        }
    }

    pub fn with_fallback_keys(mut self, keys: Vec<NodeKey>) -> Self {
        self.fallback_keys = keys;
        self
    }

    pub fn with_location(mut self, byte_offset: usize) -> Self {
        self.location = Some(byte_offset);
        self
    }
}

/// Represents a `{maps_to}` directive parsed from a section node.
/// The owning section node will have `WEIGHT_OWNED_BY` set to its bref in each emitted edge.
///
/// Both `sources` and `sinks` accept either a single node key string or an array of node key
/// strings in the directive body (field names are `source` and `sink` respectively). One edge
/// is emitted for every element of the Cartesian product `sources × sinks`.
#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateMappingRelation {
    /// The source endpoints of the mapping edges (resolved from "source" field in directive body,
    /// which may be a single string or an array). In noet semantics, sources are the more
    /// abstract/parent nodes (e.g. requirements).
    pub sources: Vec<NodeKey>,
    /// The sink endpoints of the mapping edges (resolved from "sink" field in directive body,
    /// which may be a single string or an array). In noet semantics, sinks are the more
    /// concrete/child nodes (e.g. implementors).
    pub sinks: Vec<NodeKey>,
    /// The weight kind for the edges (from info-string arg or "weight_kind" body field).
    pub kind: WeightKind,
    /// Extra payload fields from the directive body (all keys except source, sink, weight_kind).
    pub weight: Option<Weight>,
    /// Byte offset of the directive in the source document, for diagnostics.
    pub location: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct IRNode {
    pub accumulator: Option<String>,
    /// Original TOML content for reference
    pub content: String,
    /// TOML document that preserves key order and formatting
    pub document: DocumentMut,
    pub upstream: Vec<IntermediateRelation>,
    pub downstream: Vec<IntermediateRelation>,
    pub path: String,
    pub kind: BeliefKindSet,
    pub errors: Vec<BuildonomyError>,
    pub heading: usize,
    /// 1-based line number of this node's heading (or 1 for the document root node)
    /// in the source file. Populated by MdCodec during parsing. Used to construct
    /// `#L<n>` source backlinks. `None` when the codec does not track line numbers.
    pub source_line: Option<usize>,
    pub mappings: Vec<IntermediateMappingRelation>,
    /// Additional paths under which this node should be registered in the
    /// parent network's PathMap.  Each alias creates an additional PathMap
    /// entry pointing to the same BID.  Used by codecs that need a node
    /// to be findable under synthetic paths (e.g. C++ include-convention
    /// paths, derived header filenames from YAML code generation).
    ///
    /// These are network-relative paths (same scope as `WEIGHT_DOC_PATHS`).
    /// `push()` appends them to the Section edge's `doc_paths` weight.
    pub path_aliases: Vec<String>,
    /// Additional namespaces under which this node should be registered via
    /// secondary index PathMaps. Each entry is `(namespace_bid, alias_path)`:
    /// the codec-registered namespace's BID (from `Bid::codec_namespace(term)`)
    /// and the path under which this node should be findable in that namespace.
    ///
    /// Unlike `path_aliases` (which register in the node's home network PathMap),
    /// namespace paths register in a separate, cross-network PathMap identified
    /// by the namespace BID.  This enables cross-network resolution — e.g. a
    /// C++ `#include <target/Header.h>` edge can resolve against the "include"
    /// namespace without knowing the target's home network.
    ///
    /// `push()` processes these by lazily creating the namespace network node
    /// and emitting `RelationChange` events to populate the namespace's PathMap.
    pub namespace_paths: Vec<(Bid, String)>,
}

impl PartialEq for IRNode {
    fn eq(&self, other: &Self) -> bool {
        self.document
            .as_table()
            .to_string()
            .eq(&other.document.as_table().to_string())
            && self.kind.eq(&other.kind)
            && self.upstream.eq(&other.upstream)
            && self.downstream.eq(&other.downstream)
            && self.mappings.eq(&other.mappings)
        // source_line intentionally excluded: positional parse-time info, not structural identity
    }
}

impl IRNode {
    pub fn id(&self) -> Option<String> {
        // Mirror BeliefNode::id(): prefer explicit document["id"], fall back to to_anchor(title).
        // An empty string id is a sentinel meaning "collision detected — suppress title fallback".
        // We must check for the sentinel first to short-circuit before the title fallback runs.
        let raw = self.document.get("id");
        // Check for empty-string sentinel first (collision suppression).
        if raw.and_then(|v| v.as_str()) == Some("") {
            return None;
        }
        // Coerce any non-string scalar to string before falling back to title slug.
        if let Some(val) = raw {
            if let Some(s) = val.as_str() {
                return Some(s.to_string());
            }
            if let Some(n) = val.as_integer() {
                return Some(n.to_string());
            }
            if let Some(f) = val.as_float() {
                return Some(f.to_string());
            }
            return Some(format!("{val:?}"));
        }
        // No id field at all — fall back to title slug.
        let slug = to_anchor(self.title().as_deref().unwrap_or(""));
        if slug.is_empty() {
            None
        } else {
            Some(slug)
        }
    }

    pub fn title(&self) -> Option<String> {
        self.document
            .get("title")
            .and_then(|title_val| title_val.as_str().map(|title_str| title_str.to_string()))
    }

    pub fn merge(&mut self, other: &mut IRNode) -> bool {
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

        if !other.namespace_paths.is_empty() {
            self.namespace_paths.append(&mut other.namespace_paths);
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
        // Compare BeliefNode fields directly against the proto's TOML document rather than
        // going through IRNode::try_from(ctx.node) → to_string(BeliefNode) → from_str.
        // That round-trip introduces TOML formatting artifacts (e.g. a leading space before
        // string values: ` "Title"` vs `"Title"`) that cause spurious `changed = true` on
        // every inject_context call, even when the field values are semantically identical.
        // Direct field comparison is both correct and avoids the serialization cost.
        let mut changed = self.merge_from_belief_node(ctx.node);
        // Only update path from context for section nodes (heading > 2)
        // Document nodes already have correct path from IRNode::new()
        // Section nodes need path from PathMap because they don't have independent file paths
        if self.heading > 2 {
            self.path = ctx.root_path.clone();
        }
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

    /// Compare and update this IRNode's document fields against the canonical values from a
    /// `BeliefNode`, without going through TOML string serialization. Returns `true` if any
    /// field was updated.
    ///
    /// Only touches fields that are actually present on `BeliefNode` and relevant to source
    /// files: `bid`, `title`, `id`, `kind`. `schema` and `payload` are left to the
    /// existing per-file frontmatter — they are not part of the runtime `BeliefNode` state
    /// that `inject_context` needs to propagate back.
    fn merge_from_belief_node(&mut self, node: &BeliefNode) -> bool {
        let mut changed = false;

        // bid
        let bid_str = node.bid.to_string();
        let proto_bid = self
            .document
            .get("bid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if proto_bid.as_deref() != Some(&bid_str) {
            self.document.insert("bid", value(bid_str));
            changed = true;
        }

        // title — only update when ctx has a non-empty title that differs
        if !node.title.is_empty() {
            let proto_title = self
                .document
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if proto_title.as_deref() != Some(&node.title) {
                self.document.insert("title", value(node.title.clone()));
                changed = true;
            }
        }

        // id — only write when the proto has no id from source parsing.
        //
        // `update_from_context` (which calls this function) runs BEFORE inject_context's
        // heading-specific id logic reads `proto.document["id"]` as the authoritative
        // source-local id. If we wrote a stale cached bref here (e.g. from global_bb's
        // first-one-wins collision record), inject_context would read that bref as the
        // source-parsed id, producing a wrong [sections."id://bref"] key in finalize().
        //
        // Safe cases where we DO write:
        //   - proto has no id at all (None) — id was just assigned, needs persisting.
        //
        // Cases where we must NOT write:
        //   - proto already has an id from source parsing (Some) — leave it for inject_context.
        if let NodeId::Explicit(ref node_id) = node.id {
            let proto_id = self
                .document
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if proto_id.is_none() {
                self.document.insert("id", value(node_id.clone()));
                changed = true;
            }
        }

        // kind — compare as BeliefKindSet; only write when it differs
        if self.kind != node.kind {
            self.kind = self.kind.union(node.kind.0).into();
            changed = true;
        }

        changed
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
                                net: Bref::default(),
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
                                net: Bref::default(),
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
                            self.downstream.push(IntermediateRelation::new(
                                node_key,
                                weight_kind,
                                payload,
                            ));
                        }
                        EdgeDirection::Upstream => {
                            self.upstream.push(IntermediateRelation::new(
                                node_key,
                                weight_kind,
                                payload,
                            ));
                        }
                    }
                }
            } else if let Some(id_str) = field_value.as_str() {
                // Single naked reference (not in an array)
                let node_key = NodeKey::from_str(id_str).unwrap_or_else(|_| NodeKey::Id {
                    net: Bref::default(),
                    id: id_str.to_string(),
                });
                match graph_field.direction {
                    EdgeDirection::Downstream => {
                        self.downstream.push(IntermediateRelation::new(
                            node_key,
                            weight_kind,
                            None,
                        ));
                    }
                    EdgeDirection::Upstream => {
                        self.upstream
                            .push(IntermediateRelation::new(node_key, weight_kind, None));
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse content with explicit format preference
    pub fn from_str_with_format(
        str: &str,
        preferred_format: MetadataFormat,
    ) -> Result<IRNode, BuildonomyError> {
        let mut proto = IRNode::default();
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
        // Extract url_aliases frontmatter field → populate namespace_paths
        // as (href_namespace(), alias) per entry. These register the node in the
        // href PathMap so that URL links to these aliases resolve to this node.
        if let Some(aliases) = proto.document.get("url_aliases").and_then(|v| v.as_array()) {
            for alias_val in aliases {
                if let Some(alias) = alias_val.as_str() {
                    proto
                        .namespace_paths
                        .push((crate::properties::href_namespace(), alias.to_string()));
                }
            }
        }

        Ok(proto)
    }
}

impl FromStr for IRNode {
    type Err = BuildonomyError;
    // Use JSON-first parsing with TOML fallback for cross-platform compatibility
    // Benefits:
    // 1. Parses parent_connections → downstream
    // 2. Preserves unknown fields for round-trip
    // 3. JSON default enables browser/web tool compatibility
    fn from_str(str: &str) -> Result<IRNode, BuildonomyError> {
        Self::from_str_with_format(str, MetadataFormat::Json)
    }
}

impl TryFrom<&BeliefNode> for IRNode {
    type Error = BuildonomyError;

    fn try_from(src: &BeliefNode) -> Result<Self, Self::Error> {
        let content = to_string(src)?;
        let mut proto = IRNode::from_str(&content)?;
        proto.kind = src.kind.clone();
        // `metadata` is a runtime-only field on `BeliefNode` and must never appear in
        // `IRNode.document`.  If it does, it means the caller serialized a BeliefNode
        // with non-empty metadata through `to_string` and we're about to corrupt a
        // source file.  Warn loudly so we can find the propagation path.
        // `metadata` is runtime-only and must never appear in IRNode.document (which maps
        // to source-file frontmatter). Strip it unconditionally after deserialisation.
        // `to_string(src)` serialises all BeliefNode fields including metadata when non-empty,
        // so this remove is the designated strip point for the BeliefNode → IRNode conversion.
        proto.document.remove("metadata");
        Ok(proto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yaml_whitelist_array_survives_irnode_parse() {
        // Verifies that YAML-format frontmatter with an array value (as commonly
        // used in network index files) round-trips correctly through IRNode::from_str
        // so that prepare_proto_relations and NetworkCodec::parse can read whitelist/blacklist.
        let frontmatter = "id: vast-horizon\ntitle: \"Test\"\nwhitelist: [\"docs/**\", \"src/**\"]";
        let node = IRNode::from_str(frontmatter).expect("YAML IRNode parse failed");
        let wl = node.document.get("whitelist");
        assert!(
            wl.is_some(),
            "whitelist key absent from document after YAML parse"
        );
        let arr = wl.unwrap().as_array();
        assert!(
            arr.is_some(),
            "whitelist is not an array in DocumentMut: {:?}",
            wl
        );
        let items: Vec<&str> = arr.unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            items,
            vec!["docs/**", "src/**"],
            "whitelist items incorrect after YAML→TOML round-trip: {:?}",
            items
        );
    }

    #[test]
    fn test_yaml_blacklist_array_survives_irnode_parse() {
        let frontmatter =
            "id: test\ntitle: \"Test\"\nblacklist: [\"generated/**\", \"scratch/**\"]";
        let node = IRNode::from_str(frontmatter).expect("YAML IRNode parse failed");
        let bl = node.document.get("blacklist");
        assert!(
            bl.is_some(),
            "blacklist key absent from document after YAML parse"
        );
        let arr = bl.unwrap().as_array();
        assert!(arr.is_some(), "blacklist is not an array: {:?}", bl);
        let items: Vec<&str> = arr.unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(items, vec!["generated/**", "scratch/**"]);
    }

    #[test]
    fn test_parse_json_format() {
        let json_content = r#"{
            "bid": "12345678-1234-1234-1234-123456789abc",
            "schema": "intention_lattice.intention",
            "title": "Test Node",
            "parent_connections": []
        }"#;

        let result = IRNode::from_str(json_content);
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

        let result = IRNode::from_str(toml_content);
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

        let result = IRNode::from_str_with_format(json_content, MetadataFormat::Json);
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

        let result = IRNode::from_str_with_format(toml_content, MetadataFormat::Toml);
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
    fn test_reserved_bid_namespace_buildonomy() {
        let toml = r#"
    bid = "6b3d2154-c0a9-437b-9324-b418a9d37ad1"
    id = "test-node"
    title = "Test"
    "#;
        let result = IRNode::from_str(toml);
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
        let result = IRNode::from_str(toml);
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
        let result = IRNode::from_str(&toml);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved"));
    }

    #[test]
    fn test_non_reserved_ids_allowed() {
        let toml = r#"
    id = "my-custom-node"
    title = "Test"
    "#;
        let result = IRNode::from_str(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_reserved_bids_allowed() {
        let toml = r#"
    bid = "a065d82c-9d68-4470-be02-028fb6c507c0"
    id = "my-custom-node"
    title = "Test"
    "#;
        let result = IRNode::from_str(toml);
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

        let result = IRNode::from_str_with_format(yaml_content, MetadataFormat::Yaml);
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

        let result = IRNode::from_str_with_format(yaml_content, MetadataFormat::Yaml);
        assert!(result.is_ok());

        let proto = result.unwrap();
        assert_eq!(
            proto.document.get("bid").and_then(|v| v.as_str()),
            Some("yaml-test")
        );
    }

    #[test]
    fn test_url_aliases_populates_namespace_paths() {
        let content = r#"title = "Test Node"
url_aliases = ["https://example.com/browse/X-1", "/docs/some-page"]"#;
        let node = IRNode::from_str(content).expect("parse failed");
        assert_eq!(
            node.namespace_paths.len(),
            2,
            "Should have 2 namespace_paths entries"
        );
        let href_ns = crate::properties::href_namespace();
        for (ns_bid, _alias) in &node.namespace_paths {
            assert_eq!(*ns_bid, href_ns, "All aliases should target href_namespace");
        }
        assert_eq!(node.namespace_paths[0].1, "https://example.com/browse/X-1");
        assert_eq!(node.namespace_paths[1].1, "/docs/some-page");
    }

    #[test]
    fn test_url_aliases_round_trips_in_frontmatter() {
        let content = r#"title = "Test Node"
url_aliases = ["https://example.com/browse/X-1"]"#;
        let node = IRNode::from_str(content).expect("parse failed");
        let frontmatter = node.as_frontmatter();
        assert!(
            frontmatter.contains("url_aliases"),
            "url_aliases should survive in frontmatter output: {frontmatter}"
        );
        assert!(
            frontmatter.contains("https://example.com/browse/X-1"),
            "alias URL should survive in frontmatter output: {frontmatter}"
        );
    }

    #[test]
    fn test_url_aliases_empty_array() {
        let content = r#"title = "Test Node"
url_aliases = []"#;
        let node = IRNode::from_str(content).expect("parse failed");
        assert!(
            node.namespace_paths.is_empty(),
            "Empty url_aliases should produce no namespace_paths"
        );
    }

    #[test]
    fn test_url_aliases_absent() {
        let content = r#"title = "Test Node""#;
        let node = IRNode::from_str(content).expect("parse failed");
        assert!(
            node.namespace_paths.is_empty(),
            "Missing url_aliases should produce no namespace_paths"
        );
    }
}
