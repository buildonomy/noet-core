//! Link resolution and formatting tests

use noet_core::{
    beliefbase::BeliefBase,
    codec::DocumentCompiler,
    db::{db_init, DbConnection, Transaction},
    event::BeliefEvent,
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

/// Compile the `network_1` fixture and return `(global_bb, parse_results)`.
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

/// A peer link to a target also cited by a hand-written link in `index.md` must
/// resolve, not be marked permanently unresolved.
///
/// Root cause (confirmed by trace-level `noet_core::codec::compiler` logging
/// against a 3-file minimal repro, single-threaded, no `--jobs`): `index.md`'s
/// own forward reference to the target pushes it onto `remainder_queue` during
/// Phase 1. Phase 2's `leaf_batch` is built with
/// `.filter(|p| !self.processed.contains_key(p) && !self.remainder_queue.contains(p))`,
/// which excludes the target because it is already queued — so `current_batch`
/// (built from `leaf_batch`) never contains it. When the peer document's own
/// link to the same target is processed, `process_unresolved_reference` sees
/// `already_queued=true` (so it skips the "push and self-requeue" branch) and
/// falls through to `self.current_batch.contains(&canonical_dep_path)`, which
/// is `false` — so the reference is immediately marked permanently unresolved,
/// before the target has been parsed even once. This is independent of
/// parallelism: it reproduces identically at `--jobs 1`.
#[test(tokio::test)]
async fn test_index_linked_target_resolves_from_peer_link() -> Result<(), Box<dyn std::error::Error>>
{
    let (_bb, results) = compile_network_1().await?;

    let peer_result = results
        .iter()
        .find(|r| r.path.to_string_lossy().contains("index_linked_peer_test"))
        .expect("index_linked_peer_test.md must appear in parse results");

    // By the time `parse_all` returns, `promote_unresolved_to_warnings` has already
    // converted every `UnresolvedReference` diagnostic into a `Warning` whose message
    // reads "unresolved link — tried [...]" (see `DocumentCompiler::promote_unresolved_to_warnings`).
    // Check the promoted `Warning` form, not `as_unresolved_reference()`, which is always
    // empty here regardless of whether the link actually resolved.
    let unresolved_to_target: Vec<_> = peer_result
        .diagnostics
        .iter()
        .filter_map(|d| match d {
            noet_core::codec::ParseDiagnostic::Warning { message, .. }
                if message.contains("index_linked_target_test") =>
            {
                Some(message.clone())
            }
            _ => None,
        })
        .collect();

    assert!(
        unresolved_to_target.is_empty(),
        "index_linked_peer_test.md's link to index_linked_target_test.md must resolve \
         (a target that index.md also cites by a hand-written link should be reachable \
         from any other document exactly as any other internal link would be); \
         found unresolved: {unresolved_to_target:?}"
    );

    Ok(())
}

/// The index-cited target must stay PathMap-resolvable, so links to it rewrite
/// with a real `dest_url` and a second parse is a no-op.
///
/// Distinct from the unresolved-reference symptom above, and only observable once
/// that one is fixed (before then the peer's reference was abandoned as permanently
/// unresolved, so the rewrite never ran).
///
/// Guards the weight-union clause in `BeliefBase::compute_diff` Phase 4 ("New
/// edges", `src/beliefbase/base.rs`), whose comment carries the full rationale.
/// In short: one `(source, sink)` pair can carry several `WeightKind`s, and a
/// network's `index.md` that also cites one of its own children produces exactly
/// that -- the child owns a structural `Section` edge to the network, and the
/// citation adds an `Epistemic` edge between the same pair. Emitting only the
/// citing document's kinds lets `update_relation` replace the whole `WeightSet`
/// and erase the child's `Section` edge; the child is then unanchorable in any
/// PathMap, `ExtendedRelation::new` falls back to an empty `root_path`, and every
/// link to it is rewritten with an empty destination -- which the next parse
/// rewrites again.
///
/// Reproduces at `--jobs 1`; the DB backend and parallelism widen the window
/// (more reparses) but are not the cause.
#[test(tokio::test)]
async fn test_index_linked_target_is_path_resolvable_from_peer(
) -> Result<(), Box<dyn std::error::Error>> {
    // The in-memory `parse_all` path resolves this correctly, so the assertion must
    // run against a DB cold start -- the configuration that forces the target to
    // arrive through `cache_fetch`'s `GlobalCache` arm. Mirrors `bid_tests`'
    // sequential_db setup: parse 1 populates the DB, parse 2 cold-starts from it.
    let (_tempdir, test_root) = generate_test_root("network_1")?;
    let db_path = test_root.join("belief_cache.db");
    let db = DbConnection(db_init(db_path).await?);

    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, true)?;
    let results1 = compiler
        .parse_sequential(&mut db.clone(), false, None)
        .await?;
    for result in &results1 {
        if let Some(content) = &result.rewritten_content {
            // A network node's `path` is its directory; its source lives in the
            // network index file inside it.
            let target = if result.path.is_dir() {
                match noet_core::codec::network::detect_network_file(&result.path) {
                    Some(p) => p,
                    None => continue,
                }
            } else {
                result.path.clone()
            };
            tokio::fs::write(&target, content).await?;
        }
    }
    let mut transaction = Transaction::default();
    while let Ok(event) = rx.try_recv() {
        transaction.add_event(&event).ok();
    }
    transaction.execute(&db.0).await?;

    // Parse 2: cold start from the DB. Must be a no-op -- any rewrite here means a
    // link was re-emitted with a different destination than parse 1 wrote.
    let (tx2, _rx2) = unbounded_channel::<BeliefEvent>();
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(tx2), None, false)?;
    let results2 = compiler2
        .parse_sequential(&mut db.clone(), false, None)
        .await?;

    let rewritten: Vec<_> = results2
        .iter()
        .filter(|r| r.rewritten_content.is_some())
        .map(|r| r.path.display().to_string())
        .collect();

    assert!(
        rewritten.is_empty(),
        "parse 2 (DB cold start) must not rewrite any file. A rewrite here means the \
         index-cited target was not PathMap-resolvable when a peer's link to it was \
         re-emitted, so the link was written with an empty dest_url. Rewritten: \
         {rewritten:?}"
    );

    Ok(())
}
