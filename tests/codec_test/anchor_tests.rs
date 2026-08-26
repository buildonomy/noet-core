//! Anchor management and collision detection tests, including `{#__continue}` magic ID.

use noet_core::{
    beliefbase::BeliefBase,
    codec::DocumentCompiler,
    event::BeliefEvent,
    properties::{BeliefNode, WeightKind},
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compile the network_1 fixture and return (global_bb, parse_results).
async fn compile_network_1(
) -> Result<(BeliefBase, Vec<noet_core::codec::compiler::ParseResult>), Box<dyn std::error::Error>>
{
    let (_tempdir, test_root) = generate_test_root("network_1")?;
    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    let results = compiler.parse_all(global_bb.clone(), false).await?;
    while let Ok(event) = rx.try_recv() {
        global_bb.process_event(&event)?;
    }
    Ok((global_bb, results))
}

/// Return all section nodes (heading > 2, i.e. not network/doc) that have a
/// Section edge pointing at `parent_bid` in `bb`.
fn section_children_of(
    bb: &BeliefBase,
    parent_bid: noet_core::properties::Bid,
) -> Vec<&BeliefNode> {
    let Some(parent_idx) = bb.bid_to_index(&parent_bid) else {
        return vec![];
    };
    bb.states()
        .values()
        .filter(|node| {
            let Some(node_idx) = bb.bid_to_index(&node.bid) else {
                return false;
            };
            bb.relations()
                .as_graph()
                .edges_connecting(node_idx, parent_idx)
                .any(|e| e.weight().weights.contains_key(&WeightKind::Section))
        })
        .collect()
}

/// Find the document node for `magic_continue_test.md` by its stable title.
fn find_magic_continue_doc(bb: &BeliefBase) -> Option<&BeliefNode> {
    bb.states()
        .values()
        .find(|n| n.title == "Magic Continue Test Document")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A heading annotated with `{#__continue}` must not produce a new section node.
/// "Single Continue" + its `{#__continue}` continuation must appear as exactly
/// one section node, not two.
#[test(tokio::test)]
async fn test_continue_basic_merge() -> Result<(), Box<dyn std::error::Error>> {
    let (bb, _results) = compile_network_1().await?;

    let doc =
        find_magic_continue_doc(&bb).expect("magic_continue_test.md document node must exist");

    // Walk to the H1 ("Magic Continue Test Root") which is the single child of the doc node.
    let h1_nodes = section_children_of(&bb, doc.bid);
    assert_eq!(
        h1_nodes.len(),
        1,
        "Expected one H1 child of the document node"
    );
    let h1 = h1_nodes[0];

    let sections = section_children_of(&bb, h1.bid);

    // Expected section nodes (in terms of titles):
    //   1. "Single Continue"         ← merged with its {#__continue} sibling
    //   2. "Independent Section"
    //   3. "First Of Chain"          ← merged with two {#__continue} siblings
    //   4. "After Chain"
    //   5. "Section With Explicit Anchor"  ← merged with its {#__continue} continuation
    //
    // Total: 5 nodes. The three {#__continue} headings must NOT appear as nodes.
    assert_eq!(
        sections.len(),
        5,
        "Expected 5 section nodes; got {}. Titles: {:?}",
        sections.len(),
        sections.iter().map(|n| &n.title).collect::<Vec<_>>(),
    );

    // "Single Continue" must appear exactly once.
    let single_continue_nodes: Vec<_> = sections
        .iter()
        .filter(|n| n.title == "Single Continue")
        .collect();
    assert_eq!(
        single_continue_nodes.len(),
        1,
        "'Single Continue' must be exactly one node (its {{#__continue}} sibling must be merged in)"
    );

    // No section node should carry "__continue" as its title or id.
    for node in &sections {
        assert_ne!(
            node.title, "__continue",
            "No section node should have title '__continue'"
        );
        assert_ne!(
            node.id(),
            "__continue",
            "No section node should have id '__continue' (node title: {})",
            node.title
        );
    }

    Ok(())
}

/// `{#__continue}` applied multiple times in a row (a chain of continuations)
/// must still produce exactly one section node for the original heading.
#[test(tokio::test)]
async fn test_continue_chain() -> Result<(), Box<dyn std::error::Error>> {
    let (bb, _results) = compile_network_1().await?;

    let doc =
        find_magic_continue_doc(&bb).expect("magic_continue_test.md document node must exist");
    let h1_nodes = section_children_of(&bb, doc.bid);
    let h1 = h1_nodes[0];
    let sections = section_children_of(&bb, h1.bid);

    // "First Of Chain" has two {#__continue} continuations — must be one node.
    let chain_nodes: Vec<_> = sections
        .iter()
        .filter(|n| n.title == "First Of Chain")
        .collect();
    assert_eq!(
        chain_nodes.len(),
        1,
        "'First Of Chain' with two continuations must produce exactly one section node"
    );

    Ok(())
}

/// A section immediately following a chain of `{#__continue}` headings must be
/// its own independent node — continuation does not bleed past a normal heading.
#[test(tokio::test)]
async fn test_continue_does_not_bleed_past_normal_heading() -> Result<(), Box<dyn std::error::Error>>
{
    let (bb, _results) = compile_network_1().await?;

    let doc =
        find_magic_continue_doc(&bb).expect("magic_continue_test.md document node must exist");
    let h1_nodes = section_children_of(&bb, doc.bid);
    let h1 = h1_nodes[0];
    let sections = section_children_of(&bb, h1.bid);

    // "Independent Section" follows "Single Continue" + its continuation.
    let independent: Vec<_> = sections
        .iter()
        .filter(|n| n.title == "Independent Section")
        .collect();
    assert_eq!(
        independent.len(),
        1,
        "'Independent Section' must exist as its own node"
    );

    // "After Chain" follows "First Of Chain" + two continuations.
    let after_chain: Vec<_> = sections
        .iter()
        .filter(|n| n.title == "After Chain")
        .collect();
    assert_eq!(
        after_chain.len(),
        1,
        "'After Chain' must exist as its own node after the continuation chain"
    );

    Ok(())
}

/// A heading with a real explicit anchor (not `{#__continue}`) must produce its own
/// node with the explicit anchor preserved as its id.
#[test(tokio::test)]
async fn test_explicit_anchor_not_treated_as_continue() -> Result<(), Box<dyn std::error::Error>> {
    let (bb, _results) = compile_network_1().await?;

    let doc =
        find_magic_continue_doc(&bb).expect("magic_continue_test.md document node must exist");
    let h1_nodes = section_children_of(&bb, doc.bid);
    let h1 = h1_nodes[0];
    let sections = section_children_of(&bb, h1.bid);

    let explicit: Vec<_> = sections
        .iter()
        .filter(|n| n.title == "Section With Explicit Anchor")
        .collect();
    assert_eq!(
        explicit.len(),
        1,
        "'Section With Explicit Anchor' must be its own node"
    );
    assert_eq!(
        explicit[0].id(),
        "explicit-anchor",
        "The explicit anchor must be preserved as the node id"
    );

    Ok(())
}

/// A `{#__continue}` heading that follows a node with an explicit anchor must
/// fold into that node without clobbering its anchor.
#[test(tokio::test)]
async fn test_continue_preserves_prior_node_anchor() -> Result<(), Box<dyn std::error::Error>> {
    let (bb, _results) = compile_network_1().await?;

    let doc =
        find_magic_continue_doc(&bb).expect("magic_continue_test.md document node must exist");
    let h1_nodes = section_children_of(&bb, doc.bid);
    let h1 = h1_nodes[0];
    let sections = section_children_of(&bb, h1.bid);

    // After folding, "Section With Explicit Anchor" absorbs the {#__continue}
    // heading that follows it. Its own explicit anchor must be unchanged.
    let anchored: Vec<_> = sections
        .iter()
        .filter(|n| n.id() == "explicit-anchor")
        .collect();
    assert_eq!(
        anchored.len(),
        1,
        "Exactly one node should carry 'explicit-anchor' as its id"
    );
    assert_eq!(
        anchored[0].title, "Section With Explicit Anchor",
        "The node with 'explicit-anchor' id must be 'Section With Explicit Anchor'"
    );

    Ok(())
}

/// Parse results for `magic_continue_test.md` must contain no diagnostics
/// related to `__continue` (no collision warnings, no unknown anchor warnings).
#[test(tokio::test)]
async fn test_continue_produces_no_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let (_bb, results) = compile_network_1().await?;

    let magic_result = results
        .iter()
        .find(|r| r.path.to_string_lossy().contains("magic_continue_test"));
    assert!(
        magic_result.is_some(),
        "magic_continue_test.md must appear in parse results"
    );

    let diags = &magic_result.unwrap().diagnostics;
    let continue_diags: Vec<_> = diags
        .iter()
        .filter(|d| {
            let msg = format!("{d:?}");
            msg.contains("__continue") || msg.contains("continue")
        })
        .collect();

    assert!(
        continue_diags.is_empty(),
        "No diagnostics should mention '__continue'. Got: {:?}",
        continue_diags
    );

    Ok(())
}
