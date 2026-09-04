//! URL alias resolution integration tests (Issue 86)
//!
//! Tests that `url_aliases` frontmatter and `alias-template` network config
//! cause URL/path links to resolve to internal nodes instead of creating
//! `External|Trace` stubs.

use noet_core::{
    beliefbase::{BeliefAccumulator, BeliefBase},
    codec::DocumentCompiler,
    event::BeliefEvent,
    properties::BeliefKind,
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

/// Compile the `network_url_alias` fixture at `test_root` with a fresh
/// `BeliefBase` and `DocumentCompiler`, as if this were a brand new process.
/// `write` controls whether rewritten content is persisted back to disk.
async fn compile_url_alias_root(
    test_root: &std::path::Path,
    write: bool,
) -> Result<BeliefBase, Box<dyn std::error::Error>> {
    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(test_root, Some(tx), None, write)?;
    compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    Ok(global_bb)
}

/// Compile the `network_url_alias` test fixture, returning the populated BeliefBase.
async fn compile_url_alias_fixture(
) -> Result<(tempfile::TempDir, BeliefBase), Box<dyn std::error::Error>> {
    let (tmp, test_root) = generate_test_root("network_url_alias")?;
    let bb = compile_url_alias_root(&test_root, false).await?;
    Ok((tmp, bb))
}

/// Find a node by title in the BeliefBase.
fn find_by_title(bb: &BeliefBase, title: &str) -> Option<noet_core::properties::Bid> {
    bb.states()
        .values()
        .find(|n| n.title == title)
        .map(|n| n.bid)
}

/// Extract the `bref://<hex>` token from the first line of `text` containing `needle`.
fn extract_bref_from_line(text: &str, needle: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("Text should contain a line with {needle:?}, got:\n{text}"));
    let start = line
        .find("bref://")
        .unwrap_or_else(|| panic!("Line should contain a bref:// annotation, got:\n{line}"))
        + "bref://".len();
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(rest.len());
    rest[..end].to_string()
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

// ── Bug 1 + Bug 2 regression: same-compile convergence for a raced citation ──
//
// Bug 1 (intra-epoch race): `doc_linking_to_alias.md` sorts lexically before
// `doc_with_url_alias.md`, so — absent the fix — its Phase 4 link-rewrite runs
// before the claimant registers its url_aliases, and the citation gets
// written pointing at the (locally-unclaimed) href stub's BID.
//
// Bug 2 (self-perpetuation): once a citation's title attribute carries
// `bref://<X>`, `check_for_link_and_push` used to trust that bref
// unconditionally, even once `<X>` no longer resolves to anything useful
// (e.g. because it named a stub that has since been absorbed). Fixed by
// falling back to URL-derivation whenever the embedded bref misses entirely,
// or resolves to an External+Trace stub.
//
// With both (A) [Bug 2 fix in `check_for_link_and_push`] and (B) [same-compile
// requeue of citing documents on absorption, in `resolve_merge_keys`] in
// place, the two bugs no longer even need two separate compiles to observe
// convergence — the citing document gets transparently requeued and
// re-resolved within the *same* `parse_all` call, before Bug 1 ever reaches
// disk. This test asserts that stronger, single-compile outcome.
//
// Wrapping the backing `BeliefBase` in a `BeliefAccumulator` is what makes
// `resolve_merge_keys`/`expand_renames` (and therefore the (B) requeue
// signal) run at all — a bare `BeliefBase` passed directly to `parse_all`
// never exercises absorption at all (see `AGENTS.md`-referenced scratchpad,
// "session_bb replay gaps"/`BeliefAccumulator` docs for why).
#[tokio::test]
async fn test_raced_citation_converges_within_one_compile() {
    let (_tmp, test_root) = generate_test_root("network_url_alias").unwrap();
    let citing_path = test_root.join("doc_linking_to_alias.md");

    let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
    let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
    let global_handle = accum.query_handle();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, true).unwrap();
    let _parse_results = compiler.parse_all(global_handle, false).await.unwrap();
    let bb = accum.into_inner().await.unwrap();

    let claimant_bid =
        find_by_title(&bb, "Issue Tracker Item").expect("Should find 'Issue Tracker Item'");
    let claimant = bb.states().get(&claimant_bid).unwrap();
    assert!(
        !claimant.kind.contains(BeliefKind::External),
        "sanity check: the claimant should be a real content node"
    );

    let final_text = std::fs::read_to_string(&citing_path).unwrap();
    let final_bref = extract_bref_from_line(&final_text, "example.com/browse/PROJ-42");

    // The stub's bref is deterministic (UUIDv5 of the URL) — provably rule out
    // "resolved to the stub" without any BID comparison across runs. See
    // ".scratchpad/url_alias_resolution_gap.md" § "Why a given bref is
    // provably the stub, not a real content node".
    let stub_bref =
        noet_core::properties::buildonomy_href_bid("https://example.com/browse/PROJ-42")
            .bref()
            .to_string();

    assert_eq!(
        final_bref,
        claimant_bid.bref().to_string(),
        "citation should resolve to the real claimant within this single compile, \
         but the on-disk citation reads:\n{}",
        final_text
            .lines()
            .find(|l| l.contains("example.com/browse/PROJ-42"))
            .unwrap_or("<line not found>")
    );
    assert_ne!(
        final_bref, stub_bref,
        "citation should not resolve to the href stub"
    );

    // The href-aliased link must preserve the original external URL, not get
    // rewritten to a document-relative path (this was the second-order bug
    // this test originally caught: converging on the right bref while
    // silently corrupting the href itself, which caused an unstable
    // requeue loop until max_reparse_count truncated it).
    assert!(
        final_text.contains("https://example.com/browse/PROJ-42"),
        "href-aliased citation must preserve the original URL as the href, got:\n{final_text}"
    );
}

/// (C) parity check: the same same-compile convergence guaranteed by (B) for
/// `parse_all`'s epoch/accumulator path must also hold for `parse_sequential`,
/// which drives absorption via `apply_absorbing_batch` directly inside its
/// `drain_rx!` macro rather than through a `BeliefAccumulator`. Before (C),
/// `parse_sequential` called `global_bb.apply_batch` directly and never ran
/// `resolve_merge_keys` at all, so a citation racing an absorption would never
/// self-heal within a single `parse_sequential` call.
///
/// Unlike the `parse_all` version of this test, no `BeliefAccumulator` is used:
/// `parse_sequential` takes `global_bb: &mut B` directly, and absorption now
/// happens inline via `apply_absorbing_batch` each time `drain_rx!` fires (which
/// requires `Some(&mut rx)` to be passed in).
#[tokio::test]
async fn test_raced_citation_converges_within_one_compile_via_parse_sequential() {
    let (_tmp, test_root) = generate_test_root("network_url_alias").unwrap();
    let citing_path = test_root.join("doc_linking_to_alias.md");

    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut global_bb = BeliefBase::empty();
    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, true).unwrap();
    let _parse_results = compiler
        .parse_sequential(&mut global_bb, false, Some(&mut rx))
        .await
        .unwrap();
    drain_events(&mut rx, &mut global_bb).await.unwrap();

    let claimant_bid =
        find_by_title(&global_bb, "Issue Tracker Item").expect("Should find 'Issue Tracker Item'");
    let claimant = global_bb.states().get(&claimant_bid).unwrap();
    assert!(
        !claimant.kind.contains(BeliefKind::External),
        "sanity check: the claimant should be a real content node"
    );

    let final_text = std::fs::read_to_string(&citing_path).unwrap();
    let final_bref = extract_bref_from_line(&final_text, "example.com/browse/PROJ-42");

    let stub_bref =
        noet_core::properties::buildonomy_href_bid("https://example.com/browse/PROJ-42")
            .bref()
            .to_string();

    assert_eq!(
        final_bref,
        claimant_bid.bref().to_string(),
        "citation should resolve to the real claimant within this single parse_sequential \
         call, but the on-disk citation reads:\n{}",
        final_text
            .lines()
            .find(|l| l.contains("example.com/browse/PROJ-42"))
            .unwrap_or("<line not found>")
    );
    assert_ne!(
        final_bref, stub_bref,
        "citation should not resolve to the href stub"
    );
    assert!(
        final_text.contains("https://example.com/browse/PROJ-42"),
        "href-aliased citation must preserve the original URL as the href, got:\n{final_text}"
    );
}

/// Companion to `test_raced_citation_converges_within_one_compile`: pins Bug 2
/// specifically — a citation whose embedded bref is already stale (a stub BID
/// that has genuinely been absorbed and deleted, not merely a coincidental
/// URL-derivation match) must still self-heal on the next compile.
///
/// This drives two compiles against a `BeliefAccumulator`-backed store, same
/// as `test_raced_citation_converges_within_one_compile` — a bare `BeliefBase`
/// passed to `parse_all` never exercises `resolve_merge_keys`, so the stub
/// would never actually be absorbed and a single-pass version of this test
/// would only ever re-derive the very same (still-unclaimed) stub BID,
/// testing nothing.
///
/// Compile #1 establishes the claimant legitimately (no poisoning yet).
/// Between compiles, the citation's on-disk bref is hand-overwritten with the
/// stub's deterministic BID — simulating a stale reference however it might
/// have arisen (a race, a manual edit, a merge) — while the backing store
/// already has the real absorption recorded. Compile #2 must re-derive the
/// real claimant from the URL rather than trusting the now-dangling embedded
/// bref.
#[tokio::test]
async fn test_stale_unresolvable_bref_self_heals_on_reparse() {
    let (_tmp, test_root) = generate_test_root("network_url_alias").unwrap();
    let citing_path = test_root.join("doc_linking_to_alias.md");

    // ── Compile #1: establish the claimant legitimately ───────────────
    let (accum_tx1, accum_rx1) = unbounded_channel::<BeliefEvent>();
    let accum1 = BeliefAccumulator::new(BeliefBase::empty(), accum_rx1);
    let global_handle1 = accum1.query_handle();
    let mut compiler1 = DocumentCompiler::new(&test_root, Some(accum_tx1), None, true).unwrap();
    let _parse_results1 = compiler1.parse_all(global_handle1, false).await.unwrap();
    let bb_after_compile1 = accum1.into_inner().await.unwrap();

    let claimant_bid = find_by_title(&bb_after_compile1, "Issue Tracker Item")
        .expect("Should find 'Issue Tracker Item'");
    let claimant = bb_after_compile1.states().get(&claimant_bid).unwrap();
    assert!(
        !claimant.kind.contains(BeliefKind::External),
        "sanity check: the claimant should be a real content node"
    );

    // ── Poison the on-disk citation with the stub's deterministic BID ─────
    // The stub itself no longer exists in the backing store (it was absorbed
    // during compile #1), so this bref is now a genuine dangling reference —
    // not a coincidental re-derivation, as it would be without a prior compile.
    let stale_bref =
        noet_core::properties::buildonomy_href_bid("https://example.com/browse/PROJ-42").bref();
    let compile1_text = std::fs::read_to_string(&citing_path).unwrap();
    let compile1_bref = extract_bref_from_line(&compile1_text, "example.com/browse/PROJ-42");
    let poisoned = compile1_text.replacen(&compile1_bref, &stale_bref.to_string(), 1);
    assert_ne!(
        poisoned, compile1_text,
        "fixture text should have contained a bref to replace"
    );
    std::fs::write(&citing_path, &poisoned).unwrap();

    // ── Compile #2: same backing store, poisoned on-disk citation ────────
    let (accum_tx2, accum_rx2) = unbounded_channel::<BeliefEvent>();
    let accum2 = BeliefAccumulator::new(bb_after_compile1, accum_rx2);
    let global_handle2 = accum2.query_handle();
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx2), None, true).unwrap();
    let _parse_results2 = compiler2.parse_all(global_handle2, false).await.unwrap();
    let _bb_after_compile2 = accum2.into_inner().await.unwrap();

    let healed_text = std::fs::read_to_string(&citing_path).unwrap();
    let healed_bref = extract_bref_from_line(&healed_text, "example.com/browse/PROJ-42");

    assert_eq!(
        healed_bref,
        claimant_bid.bref().to_string(),
        "citation should have re-derived the real claimant's bref from the URL \
         after the embedded bref failed to resolve, but got:\n{}",
        healed_text
            .lines()
            .find(|l| l.contains("example.com/browse/PROJ-42"))
            .unwrap_or("<line not found>")
    );
    assert_ne!(
        healed_bref,
        stale_bref.to_string(),
        "citation should not still carry the stale, unresolvable bref"
    );
}

/// A citation's href must never be overwritten with a bare bref.
///
/// When two aliases of the same node are cited from one consumer document, both
/// href stubs absorb into the same claimant. The claimant's alias registrations
/// share a single `(claimant -> href_namespace)` Section edge, and
/// `WeightSet::union` has rhs-overwrite semantics, so one alias's
/// `WEIGHT_DOC_PATHS` clobbers the other. The un-claimed path is then won by the
/// surviving stub, whose own PathMap entry has degraded to its bref.
///
/// Before the fix, `check_for_link_and_push` copied that degraded `root_path`
/// straight into the href slot, producing `[text](0f04decee3ed "bref://...")`.
/// That is **irreversible**: on the next parse the bare bref token resolves as
/// `NodeKey::Bref` rather than an href-namespace path, so the link permanently
/// leaves the href-alias code path and can never re-derive the URL. Three
/// further compiles were measured leaving the damage byte-identical.
///
/// This test pins the containment guarantee only: **the href survives**, and
/// therefore the link stays re-derivable. It deliberately does *not* assert the
/// bref is the claimant's — that is the separate absorption defect (the
/// duplicate-path overwrite above), tracked in
/// `.scratchpad/url_alias_resolution_gap.md`. Asserting it here would couple
/// this regression test to a bug it is not responsible for.
///
/// The consumer directory must sort *before* the target directory (`aaa/` vs
/// `target/`) so the citation is parsed before the alias claim is registered.
#[tokio::test]
async fn test_citation_href_is_never_overwritten_with_a_bref() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("aaa")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();

    std::fs::write(
        root.join("index.md"),
        "---\ntitle = \"Alias Net\"\nid = \"alias-net\"\n---\n",
    )
    .unwrap();
    std::fs::write(
        root.join("target/doc.md"),
        "---\ntitle = \"Target Doc\"\n\
         url_aliases = [\"https://example.com/full/a\", \"/bare/a2\"]\n---\n\nbody\n",
    )
    .unwrap();
    let citing_path = root.join("aaa/consumer.md");
    std::fs::write(
        &citing_path,
        "---\ntitle = \"Consumer Doc\"\n---\n\n\
         [full-a](https://example.com/full/a)\n\n[bare-a2](/bare/a2)\n",
    )
    .unwrap();

    // Compile three times. Once is enough to expose the overwrite, but the
    // original defect's defining property was that it never self-healed, so
    // repeat to prove the href is stable rather than merely intact on pass 1.
    for _ in 0..3 {
        let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
        let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
        let handle = accum.query_handle();
        let mut compiler = DocumentCompiler::new(root, Some(accum_tx), None, true).unwrap();
        compiler.parse_all(handle, false).await.unwrap();
        let _ = accum.into_inner().await.unwrap();
    }

    let text = std::fs::read_to_string(&citing_path).unwrap();

    // The href stub's BID is UUIDv5 of the URL, so its bref is stable across
    // runs and can be asserted on directly (see "Why a given bref is provably
    // the stub" in the scratchpad).
    let stub_bref = noet_core::properties::buildonomy_href_bid("/bare/a2")
        .bref()
        .to_string();

    assert!(
        text.contains("(/bare/a2"),
        "bare-path citation must keep its original href, got:\n{text}"
    );
    assert!(
        !text.contains(&format!("({stub_bref}")),
        "a bref must never be written into the href slot (found {stub_bref} as an \
         href, which is unrecoverable on reparse):\n{text}"
    );
    assert!(
        text.contains("(https://example.com/full/a"),
        "full-URL citation must keep its original href, got:\n{text}"
    );
}

/// A citation must resolve to the *claimant*, not to the stub that was absorbed
/// into it — even when the citing document is parsed before the alias is declared.
///
/// `session_bb` is a pure producer of the belief-event stream; absorption is
/// resolved on the far side of that channel, against `global_bb`. So `global_bb`
/// converged correctly while `session_bb` kept the retired stub for the rest of
/// the run — and `cache_fetch`'s StackCache probe consults `session_bb` first,
/// with a filter that deliberately admits `External` nodes. The retired stub was
/// therefore returned in preference to the claimant that absorbed it, the citing
/// document kept its edge to the stub and re-emitted it, and the duplicate path
/// was recreated on the next PathMap rebuild — a self-sustaining loop.
///
/// `DocumentCompiler::apply_absorptions_to_session` closes it by replaying the
/// accumulator's absorptions into `session_bb` as `NodeRenamed`.
///
/// This is the companion to `test_citation_href_is_never_overwritten_with_a_bref`:
/// that one pins *containment* (the href survives, so the link stays
/// re-derivable); this one pins *correctness* (it resolves to the right node).
#[tokio::test]
async fn test_citation_resolves_to_claimant_not_absorbed_stub() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("aaa")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();

    std::fs::write(
        root.join("index.md"),
        "---\ntitle = \"Alias Net\"\nid = \"alias-net\"\n---\n",
    )
    .unwrap();
    std::fs::write(
        root.join("target/doc.md"),
        "---\ntitle = \"Target Doc\"\n\
         url_aliases = [\"https://example.com/full/a\", \"/bare/a2\"]\n---\n\nbody\n",
    )
    .unwrap();
    // `aaa/` sorts before `target/`, so the citations are parsed before the
    // alias claim is registered — this is what mints the stubs in the first place.
    let citing_path = root.join("aaa/consumer.md");
    std::fs::write(
        &citing_path,
        "---\ntitle = \"Consumer Doc\"\n---\n\n\
         [full-a](https://example.com/full/a)\n\n[bare-a2](/bare/a2)\n",
    )
    .unwrap();

    let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
    let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
    let handle = accum.query_handle();
    let mut compiler = DocumentCompiler::new(root, Some(accum_tx), None, true).unwrap();
    // Exercise the parallel dispatch path explicitly. This bug was *only* visible
    // with jobs > 1: a spawned task parses into a private copy-on-write `session_bb`
    // clone, so `requeue_reparse_bids`' BID -> path lookup missed, the requeue was
    // silently dropped, and the citing document was never reparsed (3 parses instead
    // of 4). Sequential passed throughout. Note the parallel failure produced *no*
    // collision warning either, so it was silent in logs as well.
    compiler.set_jobs(4);
    compiler.parse_all(handle, false).await.unwrap();
    let bb = accum.into_inner().await.unwrap();

    let claimant_bid = find_by_title(&bb, "Target Doc").expect("Should find 'Target Doc'");
    let claimant_bref = claimant_bid.bref().to_string();

    let text = std::fs::read_to_string(&citing_path).unwrap();

    // Both stub BIDs are UUIDv5 of their URL, hence stable across runs.
    for url in ["/bare/a2", "https://example.com/full/a"] {
        let stub_bref = noet_core::properties::buildonomy_href_bid(url)
            .bref()
            .to_string();
        let got = extract_bref_from_line(&text, url);
        assert_ne!(
            got, stub_bref,
            "citation to {url:?} resolved to the absorbed stub instead of the \
             claimant that replaced it:\n{text}"
        );
        assert_eq!(
            got, claimant_bref,
            "citation to {url:?} should resolve to the claimant ({claimant_bref}):\n{text}"
        );
    }
}

/// An alias declared with a trailing slash must be reachable, and reachable by
/// *either* spelling of the citation.
///
/// A citation reaches the href PathMap through `NodeKey::regularize_unchecked`,
/// which runs `AnchorPath::normalize()` on href-namespace paths. `normalize`
/// rebuilds the path from its non-empty components, so a trailing slash is
/// dropped. Registration, however, used the raw frontmatter string. An alias
/// written `https://example.com/dir/` was therefore indexed *with* the slash
/// while every citation of it looked up the key *without* one — the two could
/// never meet, so the citation minted an `External|Trace` stub and the alias
/// sat in the namespace permanently unreachable.
///
/// This shape is not exotic: it is how every static-site generator addresses a
/// directory index (`.../power/`), so it is the normal spelling for any alias
/// pointing at a section landing page.
///
/// The fix normalizes the alias at the single point where `namespace_paths` is
/// consumed in `GraphBuilder::push`, so `url_aliases`, `alias-template`, and
/// any future alias producer share one invariant rather than each remembering
/// to normalize.
///
/// Both citation spellings are asserted because normalization must make them
/// converge on one key — that convergence is the actual guarantee, and testing
/// only the slash form would still pass if registration and lookup happened to
/// agree on the *wrong* key.
#[tokio::test]
async fn test_trailing_slash_alias_resolves_under_either_spelling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("aaa")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();

    std::fs::write(
        root.join("index.md"),
        "---\ntitle = \"Alias Net\"\nid = \"alias-net\"\n---\n",
    )
    .unwrap();
    // Alias declared WITH a trailing slash, in both URL and bare-path form.
    std::fs::write(
        root.join("target/doc.md"),
        "---\ntitle = \"Target Doc\"\n\
         url_aliases = [\"https://example.com/dir/\", \"/bare/dir/\"]\n---\n\nbody\n",
    )
    .unwrap();
    // `aaa/` sorts before `target/`, so citations are parsed before the claim.
    let citing_path = root.join("aaa/consumer.md");
    std::fs::write(
        &citing_path,
        "---\ntitle = \"Consumer Doc\"\n---\n\n\
         [url-slash](https://example.com/dir/)\n\n\
         [url-bare](https://example.com/dir)\n\n\
         [path-slash](/bare/dir/)\n\n\
         [path-bare](/bare/dir)\n",
    )
    .unwrap();

    let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
    let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
    let handle = accum.query_handle();
    let mut compiler = DocumentCompiler::new(root, Some(accum_tx), None, true).unwrap();
    compiler.set_jobs(4);
    compiler.parse_all(handle, false).await.unwrap();
    let bb = accum.into_inner().await.unwrap();

    let claimant_bid = find_by_title(&bb, "Target Doc").expect("Should find 'Target Doc'");
    let claimant_bref = claimant_bid.bref().to_string();

    let text = std::fs::read_to_string(&citing_path).unwrap();

    // Match on the link *label*, not the URL: `extract_bref_from_line` finds the
    // first line containing the needle, and "/bare/dir" is a substring of
    // "/bare/dir/", so a URL needle would silently read the wrong line.
    for (label, spelling) in [
        ("url-slash", "https://example.com/dir/"),
        ("url-bare", "https://example.com/dir"),
        ("path-slash", "/bare/dir/"),
        ("path-bare", "/bare/dir"),
    ] {
        let got = extract_bref_from_line(&text, &format!("[{label}]"));
        assert_eq!(
            got, claimant_bref,
            "citation {label} ({spelling}) should resolve to the claimant \
             ({claimant_bref}) regardless of trailing-slash spelling:\n{text}"
        );
    }
}

/// `{{ __html_path }}` lets one `alias-template` alias a whole tree by location,
/// without any per-file frontmatter.
///
/// Before this, a template could only interpolate frontmatter fields, so a
/// network could alias only those documents carrying a hand-maintained slug. Most
/// real documentation trees have no such field — pages are addressed by *where
/// they are*. The variables are computed relative to the directory of the network
/// that declared the template, so one line on a root `index.md` covers every
/// descendant.
///
/// Asserts the two properties that make the feature usable:
///
/// 1. A citation of the derived URL resolves to the document (`.md` is mapped to
///    its rendered `.html` form).
/// 2. The variables do **not** leak into the source file. They are evaluated
///    against a scratch copy of the frontmatter, never the node's own document,
///    which `generate_source` writes back to disk. Leaking them would also freeze
///    them: injection only fires when the key is absent, so a stale value written
///    on one run would survive a later file move and silently alias the node to
///    its old location.
#[tokio::test]
async fn test_html_path_template_var_aliases_by_location() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("guide")).unwrap();
    std::fs::create_dir_all(root.join("aaa")).unwrap();

    // The template lives only here; no document below carries alias frontmatter.
    std::fs::write(
        root.join("index.md"),
        "---\ntitle = \"Root\"\nid = \"root\"\n\
         alias-template = \"https://site.example/x/{{ __html_path }}\"\n---\n",
    )
    .unwrap();
    std::fs::write(
        root.join("guide/index.md"),
        "---\ntitle = \"Guide\"\nid = \"guide\"\n---\n",
    )
    .unwrap();
    let target_path = root.join("guide/setup.md");
    std::fs::write(&target_path, "---\ntitle = \"Setup\"\n---\n\nbody\n").unwrap();

    let citing_path = root.join("aaa/consumer.md");
    std::fs::write(
        &citing_path,
        "---\ntitle = \"Consumer Doc\"\n---\n\n\
         [setup](https://site.example/x/guide/setup.html)\n",
    )
    .unwrap();

    let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
    let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
    let handle = accum.query_handle();
    let mut compiler = DocumentCompiler::new(root, Some(accum_tx), None, true).unwrap();
    compiler.set_jobs(4);
    compiler.parse_all(handle, false).await.unwrap();
    let bb = accum.into_inner().await.unwrap();

    let claimant_bid = find_by_title(&bb, "Setup").expect("Should find 'Setup'");
    let claimant_bref = claimant_bid.bref().to_string();

    let citing_text = std::fs::read_to_string(&citing_path).unwrap();
    let got = extract_bref_from_line(&citing_text, "[setup]");
    assert_eq!(
        got, claimant_bref,
        "citation of the __html_path-derived URL should resolve to the document \
         it names ({claimant_bref}):\n{citing_text}"
    );

    let target_text = std::fs::read_to_string(&target_path).unwrap();
    for var in ["__path", "__html_path"] {
        assert!(
            !target_text.contains(var),
            "synthetic template variable {var:?} leaked into source frontmatter; \
             it must be evaluated against a scratch copy:\n{target_text}"
        );
    }
}
