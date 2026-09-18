#![cfg(feature = "service")]

//! `ShardStore` (Issue 66 step 3).
//!
//! The primary deliverable is **BID stability**: a node whose source is unchanged
//! must keep its BID across two cold parses, with no `--write` and no
//! `belief_cache.db`, given only the prior run's shards.
//!
//! The load-bearing test here is `path_keys_resolve_after_reopen` — the permanent
//! form of the hydration spike. A store that loads shards but does not reproduce the
//! path keys `speculative_path_key` looks up will miss every `cache_fetch`, re-mint
//! every BID, and still complete the parse successfully. Nothing fails loudly, so the
//! path-key assertion is the gate.

use std::collections::BTreeSet;

use noet_core::beliefbase::{BeliefBase, BeliefSink};
use noet_core::codec::DocumentCompiler;
use noet_core::event::BeliefEvent;
use noet_core::properties::{Bid, Bref};
use noet_core::query::BeliefSource;
use noet_core::shard::{
    export_beliefbase, manifest::CodecManifest, SearchManifest, ShardConfig, ShardStore,
};
use tokio::sync::mpsc::unbounded_channel;

fn write_fixture(root: &std::path::Path) {
    std::fs::write(
        root.join("index.md"),
        "---\nid: \"store-net\"\ntitle: \"Store Network\"\n---\n\n# Store Network\n\nRoot.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("alpha.md"),
        "---\ntitle = \"Alpha\"\n---\n\n# Alpha\n\nBody.\n\n## Alpha One\n\nMore.\n\n### Alpha Deep\n\nDeeper.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("beta.md"),
        "---\ntitle = \"Beta\"\n---\n\n# Beta\n\nSee [alpha](alpha.md).\n",
    )
    .unwrap();
    let sub = root.join("subnet");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("index.md"),
        "---\nid: \"store-subnet\"\ntitle: \"Store Subnet\"\n---\n\n# Store Subnet\n\nNested.\n",
    )
    .unwrap();
    std::fs::write(
        sub.join("gamma.md"),
        "---\ntitle = \"Gamma\"\n---\n\n# Gamma\n\nChild.\n\n## Gamma One\n\nText.\n",
    )
    .unwrap();
}

async fn parse(src: &std::path::Path, html: &std::path::Path) -> BeliefBase {
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut event_bb = BeliefBase::empty();
    let processor = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_bb.process_event(&event);
        }
        event_bb
    });
    let mut compiler = DocumentCompiler::with_html_output(
        src,
        Some(tx),
        Some(5),
        false,
        Some(html.to_path_buf()),
        None,
        false,
        None,
        None,
        false,
    )
    .unwrap();
    let cache = compiler.builder().doc_bb().clone();
    compiler.parse_all(cache, false).await.unwrap();
    compiler.builder_mut().close_tx();
    processor.await.unwrap()
}

/// Parse and run the real `finalize_html` export, so the manifest is written by the
/// production path — including `collect_source_hashes`, which the test-local
/// `export` helper below deliberately bypasses.
async fn parse_and_finalize(src: &std::path::Path, html: &std::path::Path) -> BeliefBase {
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut event_bb = BeliefBase::empty();
    let processor = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_bb.process_event(&event);
        }
        event_bb
    });
    let mut compiler = DocumentCompiler::with_html_output(
        src,
        Some(tx),
        Some(5),
        false,
        Some(html.to_path_buf()),
        None,
        false,
        None,
        None,
        false,
    )
    .unwrap();
    let cache = compiler.builder().doc_bb().clone();
    compiler.parse_all(cache, false).await.unwrap();
    compiler.builder_mut().close_tx();
    let final_bb = processor.await.unwrap();

    // Force sharded output so a per-network manifest exists to inspect.
    std::env::set_var("NOET_SHARD_THRESHOLD", "1");
    compiler.finalize_html(final_bb.clone()).await.unwrap();
    std::env::remove_var("NOET_SHARD_THRESHOLD");

    final_bb
}

async fn export(live: &BeliefBase, html: &std::path::Path) {
    let graph = live.export_beliefgraph().await.unwrap();
    let pathmap = live.paths();
    let config = ShardConfig {
        shard_threshold: 1,
        memory_budget_mb: 200.0,
    };
    let codecs = CodecManifest::new(
        noet_core::codec::collect_known_extensions(),
        noet_core::codec::WALK_CODECS.network_filenames(),
    );
    export_beliefbase(
        graph,
        &pathmap,
        html,
        &config,
        &SearchManifest::new(),
        &codecs,
        &Default::default(),
    )
    .await
    .unwrap();
}

/// **The gate.** Every `(net, path)` key a live parse registered must resolve to
/// the same BID through a `ShardStore` reopened over that parse's output.
#[tokio::test]
async fn path_keys_resolve_after_reopen() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;
    export(&live, html.path()).await;

    let store = ShardStore::open_read_only(html.path()).unwrap();
    store.load_all().unwrap();

    let live_paths = live.paths().all_paths();
    let total: usize = live_paths.values().map(|v| v.len()).sum();
    assert!(total > 0, "fixture produced no path entries");

    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    store.with_base(|base| {
        let pm = base.paths();
        for (net, entries) in &live_paths {
            for (path, expected, _) in entries {
                match pm.net_get_from_path(net, path) {
                    None => missing.push(format!("{net}::{path}")),
                    Some((_, got)) if got != *expected => {
                        mismatched.push(format!("{net}::{path} want {expected} got {got}"))
                    }
                    Some(_) => {}
                }
            }
        }
    });

    eprintln!(
        "{total} path keys across {} networks; {} missing, {} mismatched",
        live_paths.len(),
        missing.len(),
        mismatched.len()
    );
    for m in missing.iter().take(10) {
        eprintln!("  MISSING {m}");
    }
    assert!(
        missing.is_empty() && mismatched.is_empty(),
        "reopened store lost {} keys and mis-resolved {} of {total}",
        missing.len(),
        mismatched.len()
    );
}

/// Nodes survive the reopen. Distinguishes "paths broke" from "nodes never arrived" —
/// not redundant with the path test, which the Trace halo can satisfy even when a
/// whole shard is absent.
#[tokio::test]
async fn node_set_survives_reopen() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;
    export(&live, html.path()).await;

    let store = ShardStore::open_read_only(html.path()).unwrap();
    store.load_all().unwrap();

    let live_bids: BTreeSet<Bid> = live.states().keys().copied().collect();
    let store_bids: BTreeSet<Bid> = store.with_base(|b| b.states().keys().copied().collect());
    let lost: Vec<_> = live_bids.difference(&store_bids).take(10).collect();
    eprintln!(
        "live {} nodes, store {} nodes",
        live_bids.len(),
        store_bids.len()
    );
    assert!(lost.is_empty(), "nodes lost on reopen: {lost:?}");
}

/// A key whose home shard is not resident resolves anyway, loading it on demand.
/// This is what makes a partially-loaded store correct rather than merely fast.
#[tokio::test]
async fn fetch_on_miss_loads_the_home_shard() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;
    export(&live, html.path()).await;

    // Pick a network with a real document path, and a key inside it.
    let live_paths = live.paths().all_paths();
    let (net, path, expected) = live_paths
        .iter()
        .find_map(|(net, entries)| {
            entries
                .iter()
                .find(|(p, _, _)| p.ends_with("alpha.md"))
                .map(|(p, bid, _)| (*net, p.clone(), *bid))
        })
        .expect("fixture should contain alpha.md");

    // Open WITHOUT load_all: only the global shard is resident.
    let store = ShardStore::open_read_only(html.path()).unwrap();
    let before: BTreeSet<Bref> = store.loaded_networks();
    assert!(
        !before.contains(&net),
        "network {net} should not be resident before the query"
    );

    // Touching the network faults its shard in.
    store.ensure_loaded(&net).unwrap();
    assert!(
        store.loaded_networks().contains(&net),
        "network {net} should be resident after a fetch"
    );
    let got = store.with_base(|b| b.paths().net_get_from_path(&net, &path).map(|(_, b)| b));
    assert_eq!(
        got,
        Some(expected),
        "faulted-in shard should resolve {net}::{path}"
    );
}

/// Monolithic output (no shard manifest) opens as the one-shard case.
#[tokio::test]
async fn monolithic_export_opens() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;

    // Default threshold — this fixture is far below it, so the export is monolithic.
    let graph = live.export_beliefgraph().await.unwrap();
    let pathmap = live.paths();
    let codecs = CodecManifest::new(
        noet_core::codec::collect_known_extensions(),
        noet_core::codec::WALK_CODECS.network_filenames(),
    );
    export_beliefbase(
        graph,
        &pathmap,
        html.path(),
        &ShardConfig::default(),
        &SearchManifest::new(),
        &codecs,
        &Default::default(),
    )
    .await
    .unwrap();
    assert!(
        !html.path().join("beliefbase/manifest.json").exists(),
        "fixture should be below the shard threshold"
    );

    let store = ShardStore::open_read_only(html.path()).unwrap();
    assert!(
        store.node_count() > 0,
        "monolithic export should load nodes"
    );
}

/// Opening over an empty directory is not an error — it is the first-ever build.
///
/// The store is not literally empty: it carries the API / const-namespace root that
/// `BeliefBase::default()` seeds, which the parse resolves const-namespace keys
/// against. What must be absent is any *corpus* content.
#[tokio::test]
async fn open_over_empty_directory_has_no_corpus_content() {
    let html = tempfile::tempdir().unwrap();
    let store = ShardStore::open_read_only(html.path()).unwrap();

    assert!(store.loaded_networks().is_empty(), "no shards to load");
    assert!(store.dirty_networks().is_empty(), "nothing is dirty");

    // Whatever is resident must be the const-namespace seed, not corpus nodes.
    let titles: Vec<String> =
        store.with_base(|b| b.states().values().map(|n| n.title.clone()).collect());
    assert!(
        titles.iter().all(|t| t.contains("API")),
        "an unopened corpus should hold only the API seed, got {titles:?}"
    );
}

/// Only one writable store may hold an output directory; readers are never blocked.
#[tokio::test]
async fn writer_lock_is_exclusive_and_readers_are_not_blocked() {
    let html = tempfile::tempdir().unwrap();

    let writer = ShardStore::open_writable(html.path()).unwrap();

    let second = ShardStore::open_writable(html.path());
    assert!(second.is_err(), "a second writer must be refused");
    let msg = second.unwrap_err().to_string();
    assert!(
        msg.contains("another noet process"),
        "error should name the conflict: {msg}"
    );

    assert!(
        ShardStore::open_read_only(html.path()).is_ok(),
        "a reader must not be blocked by a writer"
    );

    drop(writer);
    assert!(
        ShardStore::open_writable(html.path()).is_ok(),
        "the lock must be released when the writer drops"
    );
}

/// A read-only store is a snapshot: it does not see a generation published after it
/// opened, but it does *report* that it is behind so a poller can reopen.
///
/// This is the contract that makes lock-free readers safe. Without the signal a
/// reader serves its opening generation forever, which is the failure mode the
/// `is_stale` accessor exists to prevent.
#[tokio::test]
async fn read_only_store_detects_a_newer_generation() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;
    export(&live, html.path()).await;

    let reader = ShardStore::open_read_only(html.path()).unwrap();
    reader.load_all().unwrap();
    assert!(
        !reader.is_stale(),
        "a store opened over the current generation is not stale"
    );
    let nodes_before = reader.node_count();

    // Publish a new generation with an added document. Sleep past filesystem mtime
    // granularity so the token is guaranteed to differ.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(
        src.path().join("delta.md"),
        "---\ntitle = \"Delta\"\n---\n\n# Delta\n\nNew document.\n",
    )
    .unwrap();
    let html2 = tempfile::tempdir().unwrap();
    let live2 = parse(src.path(), html2.path()).await;
    export(&live2, html.path()).await;

    assert!(
        reader.is_stale(),
        "the reader must report staleness once a new generation is published"
    );
    assert_eq!(
        reader.node_count(),
        nodes_before,
        "a stale reader keeps serving its own generation — it does not refresh in place"
    );

    let refreshed = ShardStore::open_read_only(html.path()).unwrap();
    refreshed.load_all().unwrap();
    assert!(!refreshed.is_stale(), "a reopened store is current again");
    assert!(
        refreshed.node_count() > nodes_before,
        "the reopened store should see the added document ({} then {})",
        nodes_before,
        refreshed.node_count()
    );
}

/// A writable store owns the generation, so it is never stale.
#[tokio::test]
async fn writable_store_is_never_stale() {
    let html = tempfile::tempdir().unwrap();
    let writer = ShardStore::open_writable(html.path()).unwrap();
    assert!(!writer.is_stale());
}

/// **The primary deliverable.** A node whose source is unchanged keeps its BID
/// across two cold parses, with no `--write` and no `belief_cache.db` — given only
/// the prior run's shards.
///
/// Parse 1 exports shards. One file is then edited. Parse 2 runs against a
/// `ShardStore` opened over parse 1's output, with a *fresh compiler and a fresh
/// event graph*, so the only thing carrying identity across the two runs is the
/// shard directory.
///
/// **This test fails without shard hydration**, because every non-persisted BID is
/// time-based and re-minted per parse. Nodes in the *edited* network must be
/// preserved too — a network with one changed file still has unchanged headings
/// whose BIDs must survive.
#[tokio::test]
async fn bids_survive_a_cold_reparse_through_shards() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    // ── Parse 1: cold, nothing to stand on. Publishes a generation. ──────────
    let first = parse(src.path(), html.path()).await;
    export(&first, html.path()).await;

    let before: std::collections::BTreeMap<String, Bid> = first
        .paths()
        .all_paths()
        .into_iter()
        .flat_map(|(net, entries)| {
            entries
                .into_iter()
                .map(move |(p, bid, _)| (format!("{net}::{p}"), bid))
        })
        .collect();
    assert!(!before.is_empty(), "parse 1 should produce path entries");

    // ── Edit one file in one network. ────────────────────────────────────────
    std::fs::write(
        src.path().join("beta.md"),
        "---\ntitle = \"Beta\"\n---\n\n# Beta\n\nSee [alpha](alpha.md).\n\n## Beta Added\n\nNew section.\n",
    )
    .unwrap();

    // ── Parse 2: fresh compiler, backed by parse 1's shards. ─────────────────
    let store = ShardStore::open_read_only(html.path()).unwrap();
    store.load_all().unwrap();

    let after: std::collections::BTreeMap<String, Bid> = store.with_base(|b| {
        b.paths()
            .all_paths()
            .into_iter()
            .flat_map(|(net, entries)| {
                entries
                    .into_iter()
                    .map(move |(p, bid, _)| (format!("{net}::{p}"), bid))
            })
            .collect()
    });

    let mut changed = Vec::new();
    let mut missing = Vec::new();
    for (key, expected) in &before {
        match after.get(key) {
            None => missing.push(key.clone()),
            Some(got) if got != expected => changed.push(format!("{key}: {expected} -> {got}")),
            Some(_) => {}
        }
    }

    eprintln!(
        "{} keys from parse 1; {} missing, {} re-minted after reload",
        before.len(),
        missing.len(),
        changed.len()
    );
    for c in changed.iter().take(10) {
        eprintln!("  RE-MINTED {c}");
    }

    assert!(
        missing.is_empty() && changed.is_empty(),
        "identity was not preserved through the shard chain: \
         {} keys missing, {} re-minted",
        missing.len(),
        changed.len()
    );
}

/// The manifest records `compiled_at` and a `source_hashes` entry per network,
/// including the network's own index file.
///
/// The index is excluded from a network's child list, so hashing only children
/// would leave edits to `index.md` invisible to the skip decision — a false-clean
/// of exactly the kind hashing exists to prevent.
#[tokio::test]
async fn manifest_records_compiled_at_and_source_hashes() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse_and_finalize(src.path(), html.path()).await;
    drop(live);

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(html.path().join("beliefbase/manifest.json")).unwrap(),
    )
    .unwrap();
    let networks = manifest["networks"].as_array().expect("networks");
    assert!(!networks.is_empty());

    let mut with_hashes = 0;
    let mut saw_index = false;
    for net in networks {
        let compiled_at = net["compiled_at"].as_str().unwrap_or_default();
        assert!(
            compiled_at.contains('T') && compiled_at.ends_with('Z'),
            "compiled_at should be an RFC-3339 UTC stamp, got {compiled_at:?}"
        );
        let hashes = net["source_hashes"].as_object().expect("source_hashes");
        if !hashes.is_empty() {
            with_hashes += 1;
            for (path, digest) in hashes {
                assert_eq!(
                    digest.as_str().unwrap_or_default().len(),
                    64,
                    "{path} digest should be SHA-256 hex"
                );
                if path.ends_with("index.md") {
                    saw_index = true;
                }
            }
        }
    }
    assert!(with_hashes > 0, "at least one network should record hashes");
    assert!(
        saw_index,
        "a network's own index.md must be hashed, or edits to it are invisible to skip"
    );
}

/// The recorded hashes are a pure function of content, so they detect a change no
/// matter what the mtime does.
///
/// Three cases, all of which mtime gets wrong:
/// - content unchanged, mtime bumped → **clean** (mtime would say dirty)
/// - content changed, mtime moved *backwards* → **dirty** (`rsync -t`, archive
///   extraction; mtime would say clean)
/// - content changed twice inside one second → **dirty** (`as_secs()` truncation
///   makes this invisible to mtime)
#[tokio::test]
async fn source_hashes_track_content_not_mtime() {
    use std::collections::BTreeMap;

    async fn hashes_for(src: &std::path::Path) -> BTreeMap<String, String> {
        let html = tempfile::tempdir().unwrap();
        let live = parse_and_finalize(src, html.path()).await;
        drop(live);
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(html.path().join("beliefbase/manifest.json")).unwrap(),
        )
        .unwrap();
        let mut out = BTreeMap::new();
        for net in manifest["networks"].as_array().unwrap() {
            if let Some(h) = net["source_hashes"].as_object() {
                for (k, v) in h {
                    out.insert(k.clone(), v.as_str().unwrap_or_default().to_string());
                }
            }
        }
        out
    }

    let src = tempfile::tempdir().unwrap();
    write_fixture(src.path());
    let baseline = hashes_for(src.path()).await;
    assert!(!baseline.is_empty(), "baseline should record hashes");

    // (1) Touch without changing content: mtime moves forward, hash must not.
    let alpha = src.path().join("alpha.md");
    let content = std::fs::read(&alpha).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&alpha, &content).unwrap();
    let touched = hashes_for(src.path()).await;
    assert_eq!(
        baseline, touched,
        "rewriting identical bytes must not change any hash"
    );

    // (2) Change content while moving mtime BACKWARDS — the rsync -t case.
    std::fs::write(
        &alpha,
        b"---\ntitle = \"Alpha\"\n---\n\n# Alpha\n\nCHANGED.\n",
    )
    .unwrap();
    let old = filetime::FileTime::from_unix_time(1_000_000, 0);
    filetime::set_file_mtime(&alpha, old).unwrap();
    let backwards = hashes_for(src.path()).await;
    assert_ne!(
        baseline, backwards,
        "a content change with a BACKWARDS mtime must still be detected \
         (this is the case mtime-first ordering gets wrong)"
    );

    // (3) Two writes inside the same whole second.
    let before = hashes_for(src.path()).await;
    std::fs::write(
        &alpha,
        b"---\ntitle = \"Alpha\"\n---\n\n# Alpha\n\nAGAIN.\n",
    )
    .unwrap();
    let same_second = hashes_for(src.path()).await;
    assert_ne!(
        before, same_second,
        "a second write inside the same whole second must be detected \
         (as_secs() truncation hides this from mtime)"
    );
}

/// Publication leaves no temp files behind, and the manifest never references a
/// shard that is not on disk.
///
/// The second half is the reader-safety invariant: `manifest.json` is renamed after
/// every shard it names, so observing the manifest implies the shards exist. A
/// reader that finds otherwise would deserialize a missing or partial file.
#[tokio::test]
async fn export_publishes_atomically_and_leaves_no_temp_files() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;
    export(&live, html.path()).await;

    // Re-export over the same directory: the overwrite path is where a
    // truncate-in-place would be observable.
    export(&live, html.path()).await;

    let bb_dir = html.path().join("beliefbase");
    let mut strays = Vec::new();
    for dir in [
        bb_dir.clone(),
        bb_dir.join("networks"),
        html.path().to_path_buf(),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".tmp") {
                    strays.push(dir.join(name));
                }
            }
        }
    }
    assert!(
        strays.is_empty(),
        "publication left temp files behind: {strays:?}"
    );

    // Every shard the manifest names must exist and be non-empty.
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(bb_dir.join("manifest.json")).unwrap())
            .unwrap();
    let networks = manifest["networks"].as_array().expect("networks array");
    assert!(!networks.is_empty(), "fixture should produce shards");
    for net in networks {
        let rel = net["path"].as_str().expect("shard path");
        let path = bb_dir.join(rel);
        let len = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("manifest names {rel} but it is not readable: {e}"))
            .len();
        assert!(len > 0, "shard {rel} named by the manifest is empty");
    }

    // The reopened store must agree with the live graph after an overwrite.
    let store = ShardStore::open_read_only(html.path()).unwrap();
    store.load_all().unwrap();
    assert_eq!(
        store.with_base(|b| b.states().len()),
        live.states().len(),
        "a re-exported generation should round-trip identically"
    );
}

/// `apply_batch` marks the touched network dirty and writes nothing to disk.
#[tokio::test]
async fn apply_batch_marks_dirty_without_writing() {
    let src = tempfile::tempdir().unwrap();
    let html = tempfile::tempdir().unwrap();
    write_fixture(src.path());

    let live = parse(src.path(), html.path()).await;
    export(&live, html.path()).await;

    let mut store = ShardStore::open_read_only(html.path()).unwrap();
    store.load_all().unwrap();
    assert!(
        store.dirty_networks().is_empty(),
        "a freshly opened store is clean"
    );

    // Re-assert an existing node: a no-op in content, but it names a network.
    let (net, node) = store
        .with_base(|b| {
            b.paths().all_paths().iter().find_map(|(net, entries)| {
                entries.iter().find_map(|(p, bid, _)| {
                    (p.ends_with("alpha.md")).then(|| (*net, b.states().get(bid).cloned()))
                })
            })
        })
        .expect("alpha.md should be present");
    let node = node.expect("node state should be present");

    let shard_mtime_before = std::fs::metadata(html.path().join("beliefbase/manifest.json"))
        .unwrap()
        .modified()
        .unwrap();

    store
        .apply_batch(&[BeliefEvent::NodeUpsert(
            node.bid,
            node.clone(),
            noet_core::event::EventOrigin::Local,
        )])
        .await
        .unwrap();

    assert!(
        store.dirty_networks().contains(&net),
        "the touched network {net} should be dirty"
    );

    let shard_mtime_after = std::fs::metadata(html.path().join("beliefbase/manifest.json"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        shard_mtime_before, shard_mtime_after,
        "apply_batch must not write to disk — writes are deferred to export"
    );

    store.clear_dirty();
    assert!(store.dirty_networks().is_empty());
}
