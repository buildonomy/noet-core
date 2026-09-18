//! `payload["text"]` purity (Issue 66 step 1c).
//!
//! `payload["text"]` purports to be the node's text and should be a pure function
//! of the file's content. Today it is gated in `MdCodec::inject_context` on
//! `frontmatter_changed || sections_metadata_merged || link_changed || id_changed`
//! (`src/codec/md.rs:2429-2444`). On a steady-state re-parse of a document whose
//! BIDs are all persisted and whose links all resolve, none of those fire, so
//! `maybe_text` is `None` and `text` is never written.
//!
//! That makes the field a function of parse *history* rather than of content.
//! Consumers affected: search indexing (`shard/search.rs:416`), MCP
//! (`mcp/tools.rs:353`), viewer snippets. Issue 105 additionally cannot soundly
//! hash a payload whose `text` key depends on what happened previously.
//!
//! **Fixture shape matters.** Each markdown file yields two kinds of node: a
//! *document container* (path `settled.md`, carries the `sections` table) and one
//! *section node* per heading (path `settled.md#settled`, carries the body text).
//! These tests address nodes by path key rather than by title, because titles are
//! shared between a document and its H1.

use noet_core::beliefbase::BeliefBase;
use noet_core::codec::DocumentCompiler;
use noet_core::event::BeliefEvent;
use noet_core::properties::{BeliefNode, Bref};
use tokio::sync::mpsc::unbounded_channel;

const BODY_H1: &str = "First paragraph of body text.";
const BODY_S1: &str = "Second paragraph with some words in it.";

fn write_fixture(root: &std::path::Path) {
    std::fs::write(
        root.join("index.md"),
        "---\nid: \"purity-net\"\ntitle: \"Purity Network\"\n---\n\n# Purity Network\n\nRoot.\n",
    )
    .unwrap();

    std::fs::write(
        root.join("settled.md"),
        format!(
            "---\nid: \"settled-doc\"\ntitle: \"Settled\"\n---\n\n# Settled {{#settled}}\n\n{BODY_H1}\n\n## Section One {{#section-one}}\n\n{BODY_S1}\n"
        ),
    )
    .unwrap();
}

/// Parse the tree. `write = true` stamps BIDs and anchors into source, which is
/// how a real corpus reaches the settled state where the text gate stops firing.
async fn parse(src: &std::path::Path, write: bool) -> BeliefBase {
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut event_bb = BeliefBase::empty();
    let processor = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_bb.process_event(&event);
        }
        event_bb
    });

    let mut compiler = DocumentCompiler::new(src, Some(tx), Some(5), write).unwrap();
    let cache = compiler.builder().doc_bb().clone();
    compiler.parse_all(cache, false).await.unwrap();
    compiler.builder_mut().close_tx();
    processor.await.unwrap()
}

/// The bref of the fixture's own network — the one containing `settled.md`.
///
/// Scoping every lookup to this network matters: the repo/API network also has an
/// `index.md` path entry (pointing at the API node, which carries no `text`), and
/// iteration order over networks follows brefs derived from time-based BIDs. An
/// unscoped lookup for `index.md` is therefore nondeterministic across runs.
fn fixture_net(bb: &BeliefBase) -> Bref {
    bb.paths()
        .all_paths()
        .into_iter()
        .find(|(_, entries)| entries.iter().any(|(p, _, _)| p == "settled.md"))
        .map(|(net, _)| net)
        .expect("fixture network containing settled.md should exist")
}

/// Resolve a node by its path key within the fixture's own network.
fn node_at(bb: &BeliefBase, path: &str) -> Option<BeliefNode> {
    let net = fixture_net(bb);
    let entries = bb.paths().all_paths();
    entries
        .get(&net)?
        .iter()
        .find(|(p, _, _)| p == path)
        .and_then(|(_, bid, _)| bb.states().get(bid).cloned())
}

fn text_at(bb: &BeliefBase, path: &str) -> Option<String> {
    node_at(bb, path)
        .and_then(|n| n.payload.get("text").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// **The defect.** `text` on a *document container* node is present after an
/// unsettled parse and absent after a settled one — same content, different
/// payload, decided by parse history.
///
/// Section nodes are unaffected: they receive text through a different path
/// (`md.rs:2597`) that is not gated on the mutation flags. The document container
/// and the network node are the nodes the gate at `md.rs:2429-2444` governs.
///
/// **Expected to FAIL before the step 1c fix.**
#[tokio::test]
async fn document_text_does_not_depend_on_prior_parse_state() {
    // Tree A: one parse of a fresh tree. The mutation flags fire, so text is written.
    let src_a = tempfile::tempdir().unwrap();
    write_fixture(src_a.path());
    let unsettled = parse(src_a.path(), false).await;

    // Tree B: identical content, settled first (BIDs + anchors persisted to source),
    // so the parse we observe has nothing to mutate.
    let src_b = tempfile::tempdir().unwrap();
    write_fixture(src_b.path());
    let _seeded = parse(src_b.path(), true).await;
    let settled = parse(src_b.path(), false).await;

    for path in ["settled.md", "index.md"] {
        let a = text_at(&unsettled, path);
        let b = text_at(&settled, path);
        eprintln!("{path}: unsettled={a:?} settled={b:?}");
        assert_eq!(
            a, b,
            "payload[\"text\"] at {path} differs by parse history, not by content: \
             an unsettled parse yields {a:?}, a settled parse yields {b:?}"
        );
    }
}

/// Re-parsing a settled tree must not drop a key an earlier parse wrote.
///
/// This is the same defect stated as a regression rather than a comparison: the
/// first parse writes `text`, the second finds nothing to mutate and omits it, so
/// a consumer reading the node after parse 2 sees less than after parse 1.
///
/// **Expected to FAIL before the step 1c fix.**
#[tokio::test]
async fn reparse_does_not_drop_document_text() {
    let src = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let first = parse(src.path(), true).await;
    let second = parse(src.path(), false).await;

    for path in ["settled.md", "index.md"] {
        let before = text_at(&first, path);
        let after = text_at(&second, path);
        eprintln!("{path}: before={before:?} after={after:?}");
        assert_eq!(
            before, after,
            "payload[\"text\"] at {path} was present after parse 1 and changed on \
             a no-op re-parse"
        );
    }
}

/// Section nodes keep their body text in the settled state. Not the defect — a
/// guard so the step 1c fix does not regress the path that already works.
#[tokio::test]
async fn settled_section_still_has_body_text() {
    let src = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let _seeded = parse(src.path(), true).await;
    let steady = parse(src.path(), false).await;

    for (path, expected) in [
        ("settled.md#settled", BODY_H1),
        ("settled.md#section-one", BODY_S1),
    ] {
        let text = text_at(&steady, path).unwrap_or_default();
        assert!(
            text.contains(expected),
            "payload[\"text\"] at {path} lost its body on a settled re-parse: {text:?}"
        );
    }
}

/// Two independent parses of byte-identical source must agree — the property
/// Issue 105 needs before it can hash a payload.
#[tokio::test]
async fn two_parses_of_identical_source_agree_on_text() {
    let src_a = tempfile::tempdir().unwrap();
    let src_b = tempfile::tempdir().unwrap();
    write_fixture(src_a.path());
    write_fixture(src_b.path());

    let bb_a = parse(src_a.path(), false).await;
    let bb_b = parse(src_b.path(), false).await;

    for path in [
        "settled.md#settled",
        "settled.md#section-one",
        "index.md#purity-network",
    ] {
        let a = text_at(&bb_a, path);
        let b = text_at(&bb_b, path);
        assert_eq!(a, b, "payload[\"text\"] at {path} differs between parses");
        assert!(
            a.as_deref().is_some_and(|s| !s.is_empty()),
            "payload[\"text\"] at {path} absent or empty in both parses"
        );
    }
}
