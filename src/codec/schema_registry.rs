// Schema registry for graph field definitions
//
// This module provides a global registry for schema definitions that specify
// how TOML fields map to graph edges. Schemas can be registered at runtime
// by both noet-core and downstream libraries.

use crate::properties::WeightKind;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc, time::Duration};
use toml::Value as TomlValue;

/// Global singleton schema registry with built-in schemas
pub static SCHEMAS: Lazy<SchemaRegistry> = Lazy::new(SchemaRegistry::create);

/// List of all known schema names that have graph field definitions.
/// Used by detect_schema_from_path to match path components.
pub const KNOWN_SCHEMAS: &[(&str, &str)] = &[("intentions", "intention_lattice.intention")];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    Upstream,
    Downstream,
}

#[derive(Debug, Clone)]
pub struct GraphField {
    pub field_name: &'static str,
    pub direction: EdgeDirection,
    pub weight_kind: WeightKind,
    pub required: bool,
    pub payload_fields: Vec<&'static str>, // Fields to extract into edge payload
}

#[derive(Debug, Clone)]
pub struct SchemaDefinition {
    pub graph_fields: Vec<GraphField>,
}

/// Thread-safe registry for schema definitions
///
/// Schemas map document types to graph field definitions, specifying how
/// TOML fields create edges in the belief graph.
///
/// Pattern matches [`CodecMap`](super::CodecMap) for consistency.
pub struct SchemaRegistry(Arc<RwLock<HashMap<String, Arc<SchemaDefinition>>>>);

impl Clone for SchemaRegistry {
    fn clone(&self) -> Self {
        SchemaRegistry(self.0.clone())
    }
}

impl SchemaRegistry {
    /// Create registry with built-in schemas
    pub fn create() -> Self {
        let registry = SchemaRegistry(Arc::new(RwLock::new(HashMap::new())));

        // Register built-in schemas
        registry.register(
            "intention_lattice.intention".to_string(),
            SchemaDefinition {
                graph_fields: vec![GraphField {
                    field_name: "parent_connections",
                    direction: EdgeDirection::Downstream,
                    weight_kind: WeightKind::Pragmatic,
                    payload_fields: vec!["relationship_semantics", "motivation_kinds", "notes"],
                    required: false,
                }],
            },
        );

        // Register network configuration schema (no graph fields)
        registry.register(
            "noet.network_config".to_string(),
            SchemaDefinition {
                graph_fields: vec![],
            },
        );

        registry
    }

    /// Register a schema definition
    ///
    /// If a schema with this name already exists, it will be overwritten and a log message emitted.
    pub fn register(&self, schema_name: String, definition: SchemaDefinition) {
        while self.0.is_locked() {
            tracing::info!(
                "[SchemaRegistry::register] Waiting for write access to schema registry"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        let mut writer = self.0.write();

        if writer.contains_key(&schema_name) {
            tracing::info!(
                "[SchemaRegistry::register] Overwriting existing schema: {}",
                schema_name
            );
        }

        writer.insert(schema_name, Arc::new(definition));
    }

    /// Retrieve a schema definition by name
    ///
    /// Returns a cheap Arc clone if the schema exists.
    pub fn get(&self, schema_name: &str) -> Option<Arc<SchemaDefinition>> {
        while self.0.is_locked_exclusive() {
            tracing::info!("[SchemaRegistry::get] Waiting for read access to schema registry");
            std::thread::sleep(Duration::from_millis(100));
        }

        let reader = self.0.read();
        reader.get(schema_name).cloned()
    }

    /// List all registered schema names
    pub fn list_schemas(&self) -> Vec<String> {
        while self.0.is_locked_exclusive() {
            tracing::info!(
                "[SchemaRegistry::list_schemas] Waiting for read access to schema registry"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        let reader = self.0.read();
        reader.keys().cloned().collect()
    }
}

/// Migrate old relationship_profile format to new relationship_semantics format
///
/// Converts:
/// - relationship_profile (object with numeric intensities) -> relationship_semantics (array)
/// - Removes computed types (tensions_with, trades_off, contextual)
/// - Applies threshold: intensity > 0.0 -> include semantic kind
/// - Converts to PascalCase enum values
pub fn migrate_relationship_profile(item: &mut TomlValue) -> bool {
    let mut migrated = false;

    // Check if this is a table with relationship_profile
    if let TomlValue::Table(table) = item {
        if let Some(TomlValue::Table(profile)) = table.get("relationship_profile") {
            let mut semantics = Vec::new();

            // Map any non-zero intensity to semantic kinds
            if let Some(TomlValue::Float(v)) = profile.get("constitutive") {
                if *v > 0.0 {
                    semantics.push(TomlValue::String("Constitutive".to_string()));
                }
            }
            if let Some(TomlValue::Float(v)) = profile.get("instrumental") {
                if *v > 0.0 {
                    semantics.push(TomlValue::String("Instrumental".to_string()));
                }
            }
            if let Some(TomlValue::Float(v)) = profile.get("expressive") {
                if *v > 0.0 {
                    semantics.push(TomlValue::String("Expressive".to_string()));
                }
            }
            if let Some(TomlValue::Float(v)) = profile.get("exploratory") {
                if *v > 0.0 {
                    // Include exploratory if present at all
                    semantics.push(TomlValue::String("Exploratory".to_string()));
                }
            }

            // Only migrate if we found semantic kinds
            if !semantics.is_empty() {
                table.insert(
                    "relationship_semantics".to_string(),
                    TomlValue::Array(semantics),
                );
                table.remove("relationship_profile");
                migrated = true;
            }
        }
    }

    migrated
}

/// Recursively migrate all items in an array field
pub fn migrate_array_field(array: &mut [TomlValue]) -> bool {
    let mut migrated = false;
    for item in array.iter_mut() {
        if migrate_relationship_profile(item) {
            migrated = true;
        }
    }
    migrated
}

/// Apply schema-specific migrations to a document
///
/// This function is called during TOML parsing to automatically migrate
/// old schema formats to current versions.
pub fn migrate_schema(schema_name: &str, document: &mut TomlValue) -> bool {
    match schema_name {
        "intention_lattice.intention" => {
            let mut migrated = false;

            // Migrate parent_connections array
            if let TomlValue::Table(table) = document {
                if let Some(TomlValue::Array(connections)) = table.get_mut("parent_connections") {
                    if migrate_array_field(connections) {
                        migrated = true;
                    }
                }
            }

            migrated
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_registration() {
        let registry = SchemaRegistry::create();

        // Built-in schema should be present
        assert!(registry.get("intention_lattice.intention").is_some());

        // Register custom schema
        registry.register(
            "test.schema".to_string(),
            SchemaDefinition {
                graph_fields: vec![],
            },
        );

        assert!(registry.get("test.schema").is_some());
    }

    #[test]
    fn test_schema_overwrite() {
        let registry = SchemaRegistry::create();

        let schema1 = SchemaDefinition {
            graph_fields: vec![GraphField {
                field_name: "field1",
                direction: EdgeDirection::Upstream,
                weight_kind: WeightKind::Pragmatic,
                required: true,
                payload_fields: vec![],
            }],
        };

        let schema2 = SchemaDefinition {
            graph_fields: vec![GraphField {
                field_name: "field2",
                direction: EdgeDirection::Downstream,
                weight_kind: WeightKind::Epistemic,
                required: false,
                payload_fields: vec![],
            }],
        };

        registry.register("test.overwrite".to_string(), schema1);
        registry.register("test.overwrite".to_string(), schema2);

        let retrieved = registry.get("test.overwrite").unwrap();
        assert_eq!(retrieved.graph_fields.len(), 1);
        assert_eq!(retrieved.graph_fields[0].field_name, "field2");
    }

    #[test]
    fn test_list_schemas() {
        let registry = SchemaRegistry::create();
        let schemas = registry.list_schemas();

        assert!(schemas.contains(&"intention_lattice.intention".to_string()));
    }

    #[test]
    fn test_arc_clone_cheap() {
        let registry = SchemaRegistry::create();

        let schema1 = registry.get("intention_lattice.intention").unwrap();
        let schema2 = registry.get("intention_lattice.intention").unwrap();

        // Arc clones should point to same allocation
        assert!(Arc::ptr_eq(&schema1, &schema2));
    }

    #[test]
    fn test_global_schemas_singleton() {
        // Demonstrate downstream library usage via global SCHEMAS
        SCHEMAS.register(
            "downstream.custom".to_string(),
            SchemaDefinition {
                graph_fields: vec![GraphField {
                    field_name: "related_items",
                    direction: EdgeDirection::Downstream,
                    weight_kind: WeightKind::Epistemic,
                    required: false,
                    payload_fields: vec!["tags", "priority"],
                }],
            },
        );

        // Verify registration succeeded
        let retrieved = SCHEMAS.get("downstream.custom").unwrap();
        assert_eq!(retrieved.graph_fields.len(), 1);
        assert_eq!(retrieved.graph_fields[0].field_name, "related_items");

        // Verify built-in schemas still accessible
        assert!(SCHEMAS.get("intention_lattice.intention").is_some());
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let handles: Vec<_> = (0..5)
            .map(|i| {
                thread::spawn(move || {
                    // Register schema from thread
                    SCHEMAS.register(
                        format!("concurrent.test{i}"),
                        SchemaDefinition {
                            graph_fields: vec![],
                        },
                    );

                    // Read schema from thread
                    SCHEMAS.get("intention_lattice.intention")
                })
            })
            .collect();

        for handle in handles {
            assert!(handle.join().unwrap().is_some());
        }

        // Verify all registrations succeeded
        for i in 0..5 {
            assert!(SCHEMAS.get(&format!("concurrent.test{i}")).is_some());
        }
    }
}
