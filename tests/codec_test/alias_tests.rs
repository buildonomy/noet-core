//! URL alias resolution integration tests (Issue 86)
//!
//! Tests that `url_aliases` frontmatter and `alias-template` network config
//! cause URL/path links to resolve to internal nodes instead of creating
//! `External|Trace` stubs.

use noet_core::{
    beliefbase::BeliefBase, codec::DocumentCompiler, event::BeliefEvent, properties::BeliefKind,
};
use tokio::sync::mpsc::unbounded_channel;

use crate::common::generate_test_root;

async fn drain_events(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<BeliefEvent>,
    bb: &mut BeliefBase,
) -> Result<(), Box<dyn std::error::Error>> {
    while let Ok(event) = rx.try_recv() {
        bb.process_event(&event)?;
    }
    Ok(())
}

/// Compile the `network_url_alias` test fixture, returning the populated BeliefBase.
async fn compile_url_alias_fixture(
) -> Result<(tempfile::TempDir, BeliefBase), Box<dyn std::error::Error>> {
    let (tmp, test_root) = generate_test_root("network_url_alias")?;
    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    Ok((tmp, global_bb))
}

/// Find a node by title in the BeliefBase.
fn find_by_title(bb: &BeliefBase, title: &str) -> Option<noet_core::properties::Bid> {
    bb.states()
        .values()
        .find(|n| n.title == title)
        .map(|n| n.bid)
}

// ── alias-template tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_alias_template_registers_slug_in_href_pathmap() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    // The node with slug "Web/JavaScript/Reference" should be findable
    // in the href PathMap under "/en-US/docs/Web/JavaScript/Reference".
    let ref_bid = find_by_title(&bb, "JavaScript Reference")
        .expect("Should find node with title 'JavaScript Reference'");

    let href_pm = bb.paths().href_map();
    let lookup = href_pm.get("/en-US/docs/Web/JavaScript/Reference", &bb.paths());
    assert!(
        lookup.is_some(),
        "href PathMap should contain an entry for '/en-US/docs/Web/JavaScript/Reference'"
    );

    let (_net_bid, alias_bid) = lookup.unwrap();

    // The alias BID should be the content node itself, not an External|Trace stub.
    assert_eq!(
        alias_bid, ref_bid,
        "href PathMap alias should point to the content node, not an External|Trace stub"
    );
    // Verify the node is NOT External|Trace (it's a real document node).
    let node = bb.states().get(&ref_bid).unwrap();
    assert!(
        !node.kind.contains(BeliefKind::External),
        "Aliased content node should not be External"
    );
}

#[tokio::test]
async fn test_alias_template_both_slugs_registered() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    // Both doc_with_slug.md and doc_with_guide_slug.md have slugs and should
    // each be registered in the href PathMap.
    let guide_bid = find_by_title(&bb, "JavaScript Guide").expect("Should find 'JavaScript Guide'");
    let ref_bid =
        find_by_title(&bb, "JavaScript Reference").expect("Should find 'JavaScript Reference'");

    let href_pm = bb.paths().href_map();

    let guide_lookup = href_pm.get("/en-US/docs/Web/JavaScript/Guide", &bb.paths());
    let ref_lookup = href_pm.get("/en-US/docs/Web/JavaScript/Reference", &bb.paths());

    assert!(
        guide_lookup.is_some(),
        "Guide slug should be in href PathMap"
    );
    assert!(
        ref_lookup.is_some(),
        "Reference slug should be in href PathMap"
    );

    assert_eq!(guide_lookup.unwrap().1, guide_bid);
    assert_eq!(ref_lookup.unwrap().1, ref_bid);
}

// ── url_aliases tests ────────────────────────────────────────────────────

#[tokio::test]
async fn test_url_aliases_registers_in_href_pathmap() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    let alias_bid =
        find_by_title(&bb, "Issue Tracker Item").expect("Should find 'Issue Tracker Item'");

    let href_pm = bb.paths().href_map();

    // Both aliases should be registered.
    let lookup1 = href_pm.get("https://example.com/browse/PROJ-42", &bb.paths());
    let lookup2 = href_pm.get("https://example.com/browse/PROJ-042", &bb.paths());

    assert!(
        lookup1.is_some(),
        "href PathMap should contain 'https://example.com/browse/PROJ-42'"
    );
    assert!(
        lookup2.is_some(),
        "href PathMap should contain 'https://example.com/browse/PROJ-042'"
    );

    assert_eq!(
        lookup1.unwrap().1,
        alias_bid,
        "First alias should point to the content node"
    );
    assert_eq!(
        lookup2.unwrap().1,
        alias_bid,
        "Second alias should point to the content node"
    );
}

#[tokio::test]
async fn test_url_alias_content_node_not_external() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    // The node that declares url_aliases should be a regular document node,
    // not an External|Trace stub.
    let alias_bid =
        find_by_title(&bb, "Issue Tracker Item").expect("Should find 'Issue Tracker Item'");
    let node = bb.states().get(&alias_bid).unwrap();

    assert!(
        !node.kind.contains(BeliefKind::External),
        "Node declaring url_aliases should not be External"
    );
    assert!(
        !node.kind.contains(BeliefKind::Trace),
        "Node declaring url_aliases should not be Trace"
    );
}

// ── Composition: url_aliases + alias-template on same network ────────────

#[tokio::test]
async fn test_both_mechanisms_coexist() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    // Verify that both alias mechanisms work in the same network:
    // - alias-template derived slugs are in href PathMap
    // - url_aliases explicit entries are in href PathMap
    let href_pm = bb.paths().href_map();

    let slug_entry = href_pm.get("/en-US/docs/Web/JavaScript/Reference", &bb.paths());
    let url_entry = href_pm.get("https://example.com/browse/PROJ-42", &bb.paths());

    assert!(slug_entry.is_some(), "slug-derived alias should be present");
    assert!(url_entry.is_some(), "url_aliases entry should be present");

    // They should point to different content nodes.
    let slug_bid = slug_entry.unwrap().1;
    let url_bid = url_entry.unwrap().1;
    assert_ne!(
        slug_bid, url_bid,
        "Different alias mechanisms should point to different nodes"
    );
}

// ── Step 5: HTML link annotation for href-aliased links ─────────────────

#[tokio::test]
async fn test_href_aliased_link_gets_bref_title_in_source() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    // The "Referencing Document" links to alias URLs:
    //   [PROJ-42](https://example.com/browse/PROJ-42)   → url_aliases on "Issue Tracker Item"
    //   [JavaScript Reference](/en-US/docs/...)           → alias-template slug
    // After inject_context, both links should have bref:// title attributes.
    let ref_doc_bid =
        find_by_title(&bb, "Referencing Document").expect("Should find 'Referencing Document'");

    let node = bb.states().get(&ref_doc_bid).unwrap();
    let text = node
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .expect("Referencing Document should have a 'text' payload after inject_context");

    // Both links should have bref:// annotations (the exact BID depends on
    // parse order — may be the content node or the href stub).  Check that
    // the PROJ-42 link line contains a bref annotation.
    let proj42_line = text
        .lines()
        .find(|l| l.contains("example.com/browse/PROJ-42"))
        .expect("Text should contain the PROJ-42 link");
    assert!(
        proj42_line.contains("bref://"),
        "PROJ-42 link should have a bref annotation, but got:\n{proj42_line}"
    );

    // The original URL should be preserved as the href (not rewritten to a
    // document-relative path).
    assert!(
        text.contains("https://example.com/browse/PROJ-42"),
        "Href-aliased link should preserve the original URL as the href, got:\n{text}"
    );

    // The slug link should also be annotated now that absolute paths
    // route to href_namespace.
    let slug_line = text
        .lines()
        .find(|l| l.contains("/en-US/docs/Web/JavaScript/Reference"))
        .expect("Text should contain the slug link");
    assert!(
        slug_line.contains("bref://"),
        "Slug-aliased link should have a bref annotation, but got:\n{slug_line}"
    );
}

#[tokio::test]
async fn test_href_aliased_self_reference_gets_bref_title() {
    let (_tmp, bb) = compile_url_alias_fixture().await.unwrap();

    // The "Issue Tracker Item" document contains a self-referencing Jira link:
    //   [PROJ-42](https://example.com/browse/PROJ-42)
    // where the URL is one of its own url_aliases.  After inject_context, the
    // generated text should contain a bref:// title annotation even though the
    // link resolves to the document itself (self-reference).
    let alias_bid =
        find_by_title(&bb, "Issue Tracker Item").expect("Should find 'Issue Tracker Item'");

    let node = bb.states().get(&alias_bid).unwrap();
    let text = node
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .expect("Issue Tracker Item should have a 'text' payload after inject_context");

    let expected_bref = format!("bref://{}", alias_bid.bref());
    assert!(
        text.contains(&expected_bref),
        "Self-referencing href-aliased link should contain bref annotation \
         '{expected_bref}', but got:\n{text}"
    );

    // The original URL should be preserved.
    assert!(
        text.contains("https://example.com/browse/PROJ-42"),
        "Self-referencing link should preserve the original URL, got:\n{text}"
    );
}
