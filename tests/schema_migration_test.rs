/// Test automatic schema migration from old relationship_profile to new relationship_semantics
use noet_core::codec::lattice_toml::ProtoBeliefNode;
use std::str::FromStr;

#[test]
fn test_relationship_profile_migration() {
    let old_format = r#"id = "test-action"
title = "Test Action"
schema = "intention_lattice.intention"

[[parent_connections]]
parent_id = "asp-test-aspiration"
notes = "This uses old relationship_profile format"

[parent_connections.relationship_profile]
constitutive = 0.9
instrumental = 0.8
tensions_with = 0.5
exploratory = 0.3
"#;

    // Parse the old format
    let mut proto = ProtoBeliefNode::from_str(old_format).expect("Failed to parse old format");
    // Trigger schema traversal (which includes migration) - simulating what parse() does
    // For testing, we manually specify the schema type
    proto.traverse_schema().expect("Failed to traverse schema");

    // Convert back to string to check the migrated format
    let migrated = proto.as_frontmatter();

    // Should contain relationship_semantics
    assert!(
        migrated.contains("relationship_semantics"),
        "Migration should add relationship_semantics field"
    );

    // Should not contain relationship_profile as a TOML key (not just in notes text)
    assert!(
        !migrated.contains("[parent_connections.relationship_profile]")
            && !migrated.contains("relationship_profile ="),
        "Migration should remove relationship_profile field. Output:\n{migrated}"
    );

    // Should contain semantic kinds for non-zero values
    assert!(
        migrated.contains("Constitutive"),
        "Should include Constitutive (0.9 > 0.0)"
    );
    assert!(
        migrated.contains("Instrumental"),
        "Should include Instrumental (0.8 > 0.0)"
    );
    assert!(
        migrated.contains("Exploratory"),
        "Should include Exploratory (0.3 > 0.0)"
    );

    // Should NOT contain computed types (tensions_with)
    assert!(
        !migrated.contains("tensions_with"),
        "Migration should remove computed types like tensions_with"
    );

    // Verify it's a valid array format
    assert!(
        migrated.contains("relationship_semantics = ["),
        "Should use array format for relationship_semantics"
    );
}

#[test]
fn test_no_migration_for_new_format() {
    let new_format = r#"id = "test-action"
title = "Test Action"
schema = "intention_lattice.intention"

[[parent_connections]]
parent_id = "asp-test-aspiration"
relationship_semantics = ["Constitutive", "Instrumental"]
notes = "This uses new relationship_semantics format"
"#;

    // Parse the new format - no migration should occur
    let proto = ProtoBeliefNode::from_str(new_format).expect("Failed to parse new format");
    let output = proto.as_frontmatter();

    // Should still have relationship_semantics
    assert!(
        output.contains("relationship_semantics"),
        "New format should preserve relationship_semantics"
    );

    // Should contain the semantic kinds
    assert!(
        output.contains("Constitutive"),
        "Should preserve Constitutive"
    );
    assert!(
        output.contains("Instrumental"),
        "Should preserve Instrumental"
    );
}

#[test]
fn test_migration_threshold() {
    let old_format = r#"id = "test-action"
title = "Test Action"
schema = "intention_lattice.intention"

[[parent_connections]]
parent_id = "asp-test-aspiration"

[parent_connections.relationship_profile]
constitutive = 0.1
instrumental = 0.0
expressive = 0.01
"#;

    let mut proto = ProtoBeliefNode::from_str(old_format).expect("Failed to parse");
    proto.traverse_schema().expect("Failed to traverse schema");
    let migrated = proto.as_frontmatter();

    // Should include any value > 0.0
    assert!(
        migrated.contains("Constitutive"),
        "Should include Constitutive (0.1 > 0.0)"
    );
    assert!(
        migrated.contains("Expressive"),
        "Should include Expressive (0.01 > 0.0)"
    );

    // Should not include zero values
    assert!(
        !migrated.contains("Instrumental"),
        "Should not include Instrumental (0.0 not > 0.0)"
    );
}

#[test]
fn test_migration_with_multiple_connections() {
    let old_format = r#"id = "test-action"
title = "Test Action"
schema = "intention_lattice.intention"

[[parent_connections]]
parent_id = "asp-first"
[parent_connections.relationship_profile]
constitutive = 0.9

[[parent_connections]]
parent_id = "asp-second"
[parent_connections.relationship_profile]
instrumental = 0.7
expressive = 0.5
"#;

    let mut proto = ProtoBeliefNode::from_str(old_format).expect("Failed to parse");
    proto.traverse_schema().expect("Failed to traverse schema");
    let migrated = proto.as_frontmatter();

    // Both connections should be migrated
    assert!(
        migrated.contains("relationship_semantics"),
        "Should migrate all connections"
    );

    // First connection should have Constitutive
    assert!(
        migrated.contains("Constitutive"),
        "First connection should have Constitutive"
    );

    // Second connection should have Instrumental and Expressive
    assert!(
        migrated.contains("Instrumental"),
        "Second connection should have Instrumental"
    );
    assert!(
        migrated.contains("Expressive"),
        "Second connection should have Expressive"
    );
}
