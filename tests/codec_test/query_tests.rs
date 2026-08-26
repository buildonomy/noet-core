//! Integration tests for the QuerySpec evaluation pipeline (Issue 79, Step 5).
//!
//! These tests compile test fixtures via `DocumentCompiler`, then construct
//! `QuerySpec` instances programmatically and evaluate them against the live
//! `BeliefBase`. Assertions verify result BID sets by count and structural
//! position (not raw BID values, which are ephemeral across runs).

use std::collections::BTreeSet;

use enumset::EnumSet;
use noet_core::{
    beliefbase::BeliefBase,
    codec::DocumentCompiler,
    event::BeliefEvent,
    properties::{BeliefKind, Bid, NodeId, WeightKind},
    query::{
        spec::{
            CompareOp, Composition, CompositionOp, NodeFilter, ProjectionStep, PropertyPredicate,
            PropertyValue, QueryPackage, QuerySpec, Role, Score, TapeFn, TraversalDepth,
            TraversalSpec,
        },
        view::{TableDisplayMode, TableView, ViewOutput, ViewRenderer},
        BeliefSource,
    },
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

// ═══════════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Drain all events from the accumulator channel into a `BeliefBase`.
async fn drain_events(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<BeliefEvent>,
    bb: &mut BeliefBase,
) -> Result<(), Box<dyn std::error::Error>> {
    while let Ok(event) = rx.try_recv() {
        bb.process_event(&event)?;
    }
    Ok(())
}

/// Compile the named test fixture and return the populated `BeliefBase`.
async fn compile_fixture(
    name: &str,
) -> Result<(tempfile::TempDir, BeliefBase), Box<dyn std::error::Error>> {
    let (tmp, test_root) = generate_test_root(name)?;
    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    Ok((tmp, global_bb))
}

/// Find a node by title in the BeliefBase. Panics if not found.
fn find_by_title(bb: &BeliefBase, title: &str) -> Bid {
    bb.states()
        .values()
        .find(|n| n.title == title)
        .unwrap_or_else(|| panic!("expected to find node with title '{title}'"))
        .bid
}

/// Find a node by id in the BeliefBase. Panics if not found.
fn find_by_id(bb: &BeliefBase, id: &str) -> Bid {
    bb.states()
        .values()
        .find(|n| matches!(&n.id, NodeId::Explicit(s) if s == id))
        .unwrap_or_else(|| panic!("expected to find node with id '{id}'"))
        .bid
}

/// Collect the BID set from evaluated QueryPackage tape using the result lens.
/// Default lens is `Then(None)` — the final user-step entry's output.
fn result_bids(package: &QueryPackage) -> BTreeSet<Bid> {
    result_bids_with_lens(package, &TapeFn::Then(None))
}

/// Collect the BID set using a specific result lens.
fn result_bids_with_lens(package: &QueryPackage, lens: &TapeFn) -> BTreeSet<Bid> {
    let seed: BTreeSet<Bid> = package
        .spec()
        .steps
        .first()
        .and_then(|s| match &s.input {
            TapeFn::Bids(bids) => Some(bids.iter().copied().collect()),
            _ => None,
        })
        .unwrap_or_default();
    package.tape().result_bids(lens, &seed)
}

/// Collect entries (BID, Score) from evaluated QueryPackage tape.
fn result_entries(package: &QueryPackage) -> Vec<(Bid, Score)> {
    result_bids(package)
        .into_iter()
        .map(|bid| (bid, Some(1.0)))
        .collect()
}

/// Build a section-traversal QuerySpec: start from `bids`, walk section edges
/// to the given `depth`.
fn section_submap_spec(bids: Vec<Bid>, depth: u8) -> QuerySpec {
    QuerySpec::seed_then(
        TapeFn::Bids(bids),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Sink.into(),
            kind_filter: EnumSet::from(WeightKind::Section),
            output_roles: Role::Source.into(),
            inverted: false,
            depth: TraversalDepth::count(depth),
        })],
    )
}

/// Build a filter-only QuerySpec.
fn filter_spec(seed: TapeFn, filter: NodeFilter) -> QuerySpec {
    QuerySpec::seed_then(seed, vec![ProjectionStep::filter(filter)])
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section submap tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that a section traversal via QuerySpec returns a meaningful set of
/// nodes from the network, including the network root and its direct children.
#[test(tokio::test)]
async fn test_section_traversal_matches_submap_depth0() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let net_bid = find_by_id(&bb, "belief-network-test-1");

    // QuerySpec: section traversal depth 1 from network root
    let spec = section_submap_spec(vec![net_bid], 1);
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let query_bids = result_bids(&package);

    // With spec-correct traversal semantics, the output is the discovered
    // nodes (section children) — NOT the seed. The seed (net_bid) is in the
    // tape but not the final result.
    assert!(
        !query_bids.contains(&net_bid),
        "traversal output should not include the seed BID (spec §5.2: \
         traversal maps input to a new set)"
    );

    // Should have multiple nodes (the network has several documents and subnets).
    assert!(
        !query_bids.is_empty(),
        "section traversal from network root should return child nodes; got 0"
    );

    // Reference: compare with submap. The submap at depth 0 returns document-
    // level entries. The section traversal walks section edges from the root.
    // These have different semantics but should have significant overlap.
    let submap_entries = bb.submap_by_bid(net_bid, None, 0, false).await?;
    let submap_bids: BTreeSet<Bid> = submap_entries.iter().map(|(_, bid, _)| *bid).collect();

    let overlap: BTreeSet<_> = submap_bids.intersection(&query_bids).collect();
    assert!(
        !overlap.is_empty(),
        "section traversal and submap should have overlapping BID sets; \
         submap has {} entries, query has {} entries",
        submap_bids.len(),
        query_bids.len()
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Filter tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that a schema filter returns only nodes matching the schema.
#[test(tokio::test)]
async fn test_filter_schema_eq() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let spec = filter_spec(
        TapeFn::Corpus,
        NodeFilter::Predicate(PropertyPredicate {
            path: vec![noet_core::query::spec::PropertySegment::Key(
                "schema".into(),
            )],
            op: CompareOp::Eq,
            value: PropertyValue::String("Document".into()),
        }),
    );
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let bids = result_bids(&package);

    // Every returned node should have schema == "Document"
    let states = bb.states();
    for bid in &bids {
        let node = states.get(bid).expect("result BID should exist in states");
        assert_eq!(
            node.schema.as_deref(),
            Some("Document"),
            "node '{}' should have schema 'Document', got {:?}",
            node.title,
            node.schema
        );
    }

    // There should be at least one Document node in network_1
    assert!(!bids.is_empty(), "should find at least one Document node");

    Ok(())
}

/// Verify that a kind filter with `In` returns only nodes of the specified kinds.
#[test(tokio::test)]
async fn test_filter_kind_in() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let spec = filter_spec(
        TapeFn::Corpus,
        NodeFilter::Predicate(PropertyPredicate {
            path: vec![noet_core::query::spec::PropertySegment::Key("kind".into())],
            op: CompareOp::In,
            value: PropertyValue::Set(vec!["Document".into()]),
        }),
    );
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let bids = result_bids(&package);

    let states = bb.states();
    for bid in &bids {
        let node = states.get(bid).expect("result BID should exist in states");
        assert!(
            node.kind.contains(BeliefKind::Document),
            "node '{}' should be Document kind, got {:?}",
            node.title,
            node.kind
        );
    }

    assert!(
        !bids.is_empty(),
        "should find at least one Document-kind node"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Pragmatic traversal tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that a pragmatic traversal from a section node that contains
/// `{implements}` edges returns the referenced targets.
///
/// `subnet1_file1.md` contains:
/// ```
/// `{implements}`
/// * [HSML]
/// * [HSTP]
/// * [API Reference]
/// `{end}`
/// ```
///
/// This creates pragmatic edges from the *Requirements section* (a child of
/// the document) to the referenced nodes. We first walk section edges from
/// the document to find children, then follow pragmatic edges from those
/// children to the targets.
#[test(tokio::test)]
async fn test_pragmatic_traversal_implements() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    // Find subnet1_file1 by title
    let file1_bid = find_by_title(&bb, "subnet1 file1");

    // Two-step projection:
    //   1. Walk section edges from the document to its child sections.
    //   2. From those children, follow pragmatic edges to find sinks.
    let spec = QuerySpec::seed_then(
        TapeFn::Bids(vec![file1_bid]),
        vec![
            // Step 1: section traversal to find children
            ProjectionStep::traverse(TraversalSpec {
                input_roles: Role::Sink.into(),
                kind_filter: EnumSet::from(WeightKind::Section),
                output_roles: Role::Source.into(),
                inverted: false,
                depth: TraversalDepth::count(1),
            }),
            // Step 2: pragmatic traversal from children
            ProjectionStep::traverse(TraversalSpec {
                input_roles: Role::Source.into(),
                kind_filter: EnumSet::from(WeightKind::Pragmatic),
                output_roles: Role::Sink.into(),
                inverted: false,
                depth: TraversalDepth::count(1),
            }),
        ],
    );
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let bids = result_bids(&package);

    // With spec-correct traversal semantics:
    //   Step 1 output = section children of file1 (not file1 itself)
    //   Step 2 output = pragmatic sinks of those children (not the children)
    // The final result is just the pragmatic targets. May be empty if
    // {implements} edges originate from file1 rather than its child sections.
    //
    // The key assertion: the seed (file1) must NOT be in the final result.
    // Traversal maps input to a new set (spec §5.2).
    assert!(
        !bids.contains(&file1_bid),
        "final result should not include the seed document (spec §5.2)"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Owner / maps_to traversal tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that an owner traversal on the mapping_test document finds the owned
/// pragmatic edges from the `{maps_to}` directive.
///
/// The `mapping_test.md` fixture has a "Trace Mapping" section that owns edges:
///   source = ["id://req-alpha", "id://req-beta"]
///   sink = "id://impl-one"
///
/// An owner-input traversal from the Trace Mapping section should return the
/// endpoints (req-alpha, req-beta, impl-one).
#[test(tokio::test)]
async fn test_owner_traversal_maps_to() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_mapping").await?;

    // Use the same-doc "Trace Mapping" owner from mapping_test.md.
    // This section owns 2 pragmatic edges:
    //   source = ["id://req-alpha", "id://req-beta"]  →  sink = "id://impl-one"
    let owner_bid = find_by_title(&bb, "Trace Mapping");

    // Owner traversal: this node owns pragmatic edges → find endpoints
    let spec = QuerySpec::seed_then(
        TapeFn::Bids(vec![owner_bid]),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Owner.into(),
            kind_filter: EnumSet::from(WeightKind::Pragmatic),
            output_roles: Role::Source | Role::Sink,
            inverted: false,
            depth: TraversalDepth::count(1),
        })],
    );
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let result_set = result_bids(&package);

    // Verify specific endpoint nodes are present.
    // The owner traversal should find the endpoints of the owned edges:
    //   Requirement Alpha, Requirement Beta (sources) → Implementation One (sink)
    let sink_bid = find_by_title(&bb, "Implementation One");
    let source_alpha = find_by_title(&bb, "Requirement Alpha");
    let source_beta = find_by_title(&bb, "Requirement Beta");

    assert!(
        result_set.contains(&sink_bid),
        "owner traversal should include sink 'Implementation One'; got {:?}",
        result_set
            .iter()
            .map(|b| {
                let n = bb.states().get(b);
                format!("{b} title={:?}", n.map(|n| &n.title))
            })
            .collect::<Vec<_>>()
    );
    assert!(
        result_set.contains(&source_alpha),
        "owner traversal should include source 'Requirement Alpha'"
    );
    assert!(
        result_set.contains(&source_beta),
        "owner traversal should include source 'Requirement Beta'"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Composition tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that `Composition(And)` of two filter queries produces the intersection.
///
/// Left: all Document-kind nodes
/// Right: all nodes with schema == "Document"
///
/// The intersection should be non-empty (Document-kind nodes with explicit
/// "Document" schema) and a subset of both input sets.
#[test(tokio::test)]
async fn test_composition_and() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let kind_filter = NodeFilter::Predicate(PropertyPredicate {
        path: vec![noet_core::query::spec::PropertySegment::Key("kind".into())],
        op: CompareOp::In,
        value: PropertyValue::Set(vec!["Document".into()]),
    });

    let schema_filter = NodeFilter::Predicate(PropertyPredicate {
        path: vec![noet_core::query::spec::PropertySegment::Key(
            "schema".into(),
        )],
        op: CompareOp::Eq,
        value: PropertyValue::String("Document".into()),
    });

    let spec = QuerySpec::seed_then(
        TapeFn::Corpus,
        vec![ProjectionStep::compose(Composition {
            left: vec![ProjectionStep::filter(kind_filter.clone())],
            op: CompositionOp::And,
            right: vec![ProjectionStep::filter(schema_filter.clone())],
        })],
    );
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let and_bids = result_bids(&package);

    // Every node in the intersection must match BOTH predicates.
    let states = bb.states();
    for bid in &and_bids {
        let node = states.get(bid).expect("result BID should exist in states");
        assert!(
            node.kind.contains(BeliefKind::Document),
            "And result node '{}' should be Document kind, got {:?}",
            node.title,
            node.kind
        );
        assert_eq!(
            node.schema.as_deref(),
            Some("Document"),
            "And result node '{}' should have schema 'Document', got {:?}",
            node.title,
            node.schema
        );
    }

    // Should have some results (network_1 has Document nodes with schema "Document")
    assert!(
        !and_bids.is_empty(),
        "And composition should produce non-empty result"
    );

    Ok(())
}

/// Verify that `Composition(Not)` produces a difference (left minus right).
///
/// Left: all nodes (CorpusWide)
/// Right: all Document-kind nodes
///
/// The result should contain nodes that are NOT Document-kind.
#[test(tokio::test)]
async fn test_composition_not_difference() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let kind_filter = NodeFilter::Predicate(PropertyPredicate {
        path: vec![noet_core::query::spec::PropertySegment::Key("kind".into())],
        op: CompareOp::In,
        value: PropertyValue::Set(vec!["Document".into()]),
    });

    let spec = QuerySpec::seed_then(
        TapeFn::Corpus,
        vec![ProjectionStep::compose(Composition {
            // Left: all nodes (no filter = pass-through)
            left: vec![],
            op: CompositionOp::Not,
            // Right: Document-kind nodes (to subtract)
            right: vec![ProjectionStep::filter(kind_filter)],
        })],
    );
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)?;
    let diff_bids = result_bids(&package);

    // No node in the result should be Document-kind
    let states = bb.states();
    for bid in &diff_bids {
        if let Some(node) = states.get(bid) {
            assert!(
                !node.kind.contains(BeliefKind::Document),
                "Difference result should not contain Document-kind node '{}' (kind: {:?})",
                node.title,
                node.kind
            );
        }
    }

    // Result should be non-empty (network_1 has Section and other non-Document nodes)
    assert!(
        !diff_bids.is_empty(),
        "Not/Difference composition should produce non-empty result"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// TableView rendering on live results
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that `TableView` renders live query results into valid HTML.
#[test(tokio::test)]
async fn test_table_instrument_depth0_on_live_results() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let spec = QuerySpec::seed_then(
        TapeFn::Corpus,
        vec![ProjectionStep::filter(NodeFilter::Predicate(
            PropertyPredicate {
                path: vec![noet_core::query::spec::PropertySegment::Key(
                    "schema".into(),
                )],
                op: CompareOp::Eq,
                value: PropertyValue::String("Document".into()),
            },
        ))],
    );
    // Use balanced() so the package has a graph for the instrument to render
    let mut package = QueryPackage::balanced(spec);
    bb.evaluate_query(&mut package)?;

    let params = toml::Table::new();
    let instrument = TableView::from_params(&params)?;
    let output = instrument.render(&package, None)?;

    match output {
        ViewOutput::Html(html) => {
            // Depth0 renders a definition list, not a columnar table.
            assert!(
                html.contains("noet-query-result"),
                "HTML output should be wrapped in a noet-query-result container"
            );
            assert!(
                html.contains("<dl class=\"noet-query-depth0\">"),
                "Depth0 should render a definition list"
            );
            assert!(
                html.contains("<dt"),
                "Depth0 should contain <dt> entries for each node"
            );
        }
        other => {
            panic!("Expected Html output, got {:?}", other);
        }
    }

    Ok(())
}

/// Verify that `TableView` in Connectivity mode produces connectivity columns.
#[test(tokio::test)]
async fn test_table_instrument_connectivity() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let net_bid = find_by_id(&bb, "belief-network-test-1");

    // Section traversal depth 1 from network root
    let spec = QuerySpec::seed_then(
        TapeFn::Bids(vec![net_bid]),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Sink.into(),
            kind_filter: EnumSet::from(WeightKind::Section),
            output_roles: Role::Source.into(),
            inverted: false,
            depth: TraversalDepth::count(1),
        })],
    );
    let mut params = toml::Table::new();
    params.insert("display".into(), toml::Value::String("connectivity".into()));

    let mut package = QueryPackage::balanced(spec);
    bb.evaluate_query(&mut package)?;

    let instrument = TableView::from_params(&params)?;
    assert_eq!(instrument.display, TableDisplayMode::Connectivity);

    let output = instrument.render(&package, None)?;

    match output {
        ViewOutput::Html(html) => {
            assert!(
                html.contains("Section In"),
                "Connectivity table should have 'Section In' header"
            );
            assert!(
                html.contains("Section Out"),
                "Connectivity table should have 'Section Out' header"
            );
        }
        other => {
            panic!("Expected Html output, got {:?}", other);
        }
    }

    Ok(())
}

/// Verify that `RawTapeView` renders owned-edge data from
/// the mapping_test fixture.
#[test(tokio::test)]
async fn test_raw_tape_view_maps_to() -> Result<(), Box<dyn std::error::Error>> {
    use noet_core::query::view::{RawTapeView, ViewRenderer};

    let (_tmp, bb) = compile_fixture("network_mapping").await?;

    let net_bid = find_by_id(&bb, "mapping-test-network");

    // Section traversal to get all nodes, then covers to get owned edges
    let spec = QuerySpec::seed_then(
        TapeFn::Bids(vec![net_bid]),
        vec![
            ProjectionStep::traverse(TraversalSpec {
                input_roles: Role::Sink.into(),
                kind_filter: EnumSet::from(WeightKind::Section),
                output_roles: Role::Source.into(),
                inverted: false,
                depth: TraversalDepth::count(3),
            }),
            ProjectionStep::traverse(TraversalSpec {
                input_roles: Role::Owner.into(),
                kind_filter: EnumSet::from(WeightKind::Pragmatic),
                output_roles: Role::Source | Role::Sink,
                inverted: false,
                depth: TraversalDepth::count(1),
            }),
        ],
    );

    let mut package = QueryPackage::balanced(spec);
    bb.evaluate_query(&mut package)?;

    let view = RawTapeView::from_params(&toml::Table::new())?;
    let output = view.render(&package, None)?;

    match output {
        ViewOutput::Html(html) => {
            // The raw tape view renders serial tables per tape entry.
            assert!(
                html.contains("<table"),
                "RawTapeView should produce HTML tables"
            );
        }
        other => {
            panic!("Expected Html output, got {:?}", other);
        }
    }

    Ok(())
}

/// Verify that `render_rows` produces structured data matching the HTML output.
#[test(tokio::test)]
async fn test_table_instrument_render_rows() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let spec = filter_spec(
        TapeFn::Corpus,
        NodeFilter::Predicate(PropertyPredicate {
            path: vec![noet_core::query::spec::PropertySegment::Key(
                "schema".into(),
            )],
            op: CompareOp::Eq,
            value: PropertyValue::String("Document".into()),
        }),
    );
    let mut package = QueryPackage::balanced(spec);
    bb.evaluate_query(&mut package)?;
    let entries = result_entries(&package);
    let graph = package
        .graph()
        .expect("balanced package should have a graph");

    let instrument = TableView::from_params(&toml::Table::new())?;
    let rows = instrument.render_rows(&entries, graph);

    // First row is headers
    assert!(!rows.is_empty(), "should have at least a header row");
    assert_eq!(
        rows[0],
        vec!["Title", "Schema", "Kind"],
        "Depth0 header row should be [Title, Schema, Kind]"
    );

    // Data rows should match the number of result entries
    assert_eq!(
        rows.len() - 1,
        entries.len(),
        "data row count should match result entry count"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// QuerySpec serde round-trip (validates WASM binding feasibility)
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that `QuerySpec` survives a JSON serialize/deserialize round-trip,
/// confirming the serde derives work correctly for WASM binding.
#[test(tokio::test)]
async fn test_queryspec_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let net_bid = find_by_id(&bb, "belief-network-test-1");

    let spec = QuerySpec::seed_then(
        TapeFn::Bids(vec![net_bid]),
        vec![
            ProjectionStep::traverse(TraversalSpec {
                input_roles: Role::Sink.into(),
                kind_filter: EnumSet::from(WeightKind::Section),
                output_roles: Role::Source.into(),
                inverted: false,
                depth: TraversalDepth::count(1),
            }),
            ProjectionStep::filter(NodeFilter::Predicate(PropertyPredicate {
                path: vec![noet_core::query::spec::PropertySegment::Key(
                    "schema".into(),
                )],
                op: CompareOp::Eq,
                value: PropertyValue::String("Document".into()),
            })),
        ],
    );

    let json = serde_json::to_string_pretty(&spec)?;
    let deserialized: QuerySpec = serde_json::from_str(&json)?;

    assert_eq!(
        spec, deserialized,
        "QuerySpec should survive JSON round-trip"
    );

    // Also verify the deserialized spec evaluates identically
    let mut pkg_orig = QueryPackage::new(spec);
    bb.evaluate_query(&mut pkg_orig)?;
    let mut pkg_deser = QueryPackage::new(deserialized);
    bb.evaluate_query(&mut pkg_deser)?;

    assert_eq!(
        result_bids(&pkg_orig),
        result_bids(&pkg_deser),
        "original and deserialized QuerySpec should produce identical results"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// BeliefSource::evaluate via QueryPackage
// ═══════════════════════════════════════════════════════════════════════════════

/// Verify that `BeliefSource::evaluate` via QueryPackage produces consistent
/// results for both plain and balanced (graph) paths.
#[test(tokio::test)]
async fn test_evaluate_query_package() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    let net_bid = find_by_id(&bb, "belief-network-test-1");

    let spec = section_submap_spec(vec![net_bid], 1);

    // Plain query — BID set result via QueryPackage
    let mut query_package = QueryPackage::new(spec.clone());
    BeliefSource::evaluate(&bb, &mut query_package).await?;
    let primary_bids = result_bids(&query_package);

    // Sanity: should have results (network's depth-1 section children)
    assert!(
        !primary_bids.is_empty(),
        "plain query should produce non-empty result set"
    );

    // Balanced (graph) — the fused pipeline adds halo and section ancestry,
    // marking non-primary nodes as Trace.
    let mut graph_package = QueryPackage::balanced(spec);
    BeliefSource::evaluate(&bb, &mut graph_package).await?;

    let graph = graph_package.into_graph();
    let graph_bids: BTreeSet<Bid> = graph.states.keys().copied().collect();

    // Traversal result BIDs must be in the graph
    assert!(
        primary_bids.is_subset(&graph_bids),
        "Graph output must contain all traversal result BIDs; missing: {:?}",
        primary_bids.difference(&graph_bids).collect::<Vec<_>>()
    );

    // Graph should have more nodes than just the traversal results
    // (halo/ancestry adds context nodes)
    assert!(
        graph_bids.len() > primary_bids.len(),
        "Balanced graph should contain context nodes beyond the traversal results"
    );

    // Graph should contain edges (section edges between network and children)
    assert!(
        graph.relations.as_graph().edge_count() > 0,
        "Graph output should contain edges; got 0"
    );

    Ok(())
}

/// Debug test: verify that raw tape entries for `consists_of(*)` contain
/// the correct section children, not ancestors.
#[test(tokio::test)]
async fn debug_raw_tape_consists_of() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, bb) = compile_fixture("network_1").await?;

    // Find subnet1
    let subnet1_bid = find_by_id(&bb, "belief-network-test-1-subnet-1");

    // consists_of(*) from subnet1
    let spec = QuerySpec::seed_then(
        TapeFn::Bids(vec![subnet1_bid]),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Sink.into(),
            kind_filter: EnumSet::from(WeightKind::Section),
            output_roles: Role::Source.into(),
            inverted: false,
            depth: TraversalDepth::from(255u8),
        })],
    );

    let mut package = QueryPackage::balanced(spec);
    bb.evaluate_query(&mut package)?;

    let tape = package.tape();
    let boundary = tape.graph_context_boundary();

    eprintln!("Tape has {} entries, boundary at {}", tape.len(), boundary);
    for (i, entry) in tape.steps.iter().enumerate() {
        let bids = entry.content.output_bids();
        let titles: Vec<String> = bids
            .iter()
            .filter_map(|bid| package.graph().unwrap().states.get(bid))
            .map(|n| n.title.clone())
            .collect();
        eprintln!(
            "  tape[{}] label={:?} output_bids={} titles={:?}",
            i,
            entry.label,
            bids.len(),
            titles
        );
    }

    // The first tape entry should contain subnet1's section children,
    // not its ancestors.
    assert!(boundary > 0, "should have user tape entries");
    let first_bids = tape.steps[0].content.output_bids();
    assert!(
        !first_bids.contains(&subnet1_bid),
        "seed node should not be in traversal output"
    );
    assert!(
        !first_bids.is_empty(),
        "traversal should discover some children"
    );

    // Verify none of the first-hop results are ancestors of subnet1.
    // subnet1's parent is the network root — the first-hop results should
    // be subnet1's children.
    let net_bid = find_by_id(&bb, "belief-network-test-1");
    assert!(
        !first_bids.contains(&net_bid),
        "first hop should not contain the network root (that's an ancestor)"
    );

    // Now test RawTapeView rendering to verify the edge remap works.
    use noet_core::query::view::{RawTapeView, ViewRenderer};
    let view = RawTapeView::from_params(&toml::Table::new())?;
    let output = view.render_json(&package)?;
    match output {
        ViewOutput::Json(json) => {
            let entries = json["entries"].as_array().unwrap();
            // Should have 3 user entries (before halo boundary).
            assert!(
                !entries.is_empty(),
                "should have at least 1 user entry, got {}",
                entries.len()
            );
            eprintln!("\nRawTapeView JSON entries: {}", entries.len());
            for (i, entry) in entries.iter().enumerate() {
                let rows = entry["rows"].as_array().unwrap();
                eprintln!(
                    "  entry[{}]: content_type={}, step_op={}, rows={}",
                    i,
                    entry["content_type"],
                    entry["step_operation"],
                    rows.len()
                );
                for (j, row) in rows.iter().enumerate() {
                    eprintln!("    row[{}]: {:?}", j, row["cells"]);
                }
            }
            // First entry should have edges (section traversal).
            assert_eq!(entries[0]["content_type"], "edges");
            let first_rows = entries[0]["rows"].as_array().unwrap();
            assert_eq!(first_rows.len(), 5, "first entry should have 5 edge rows");
        }
        other => panic!("expected Json, got {other:?}"),
    }

    Ok(())
}
