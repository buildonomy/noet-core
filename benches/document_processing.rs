//! Performance benchmarks for document processing
//!
//! These benchmarks wrap existing test scenarios from codec_test to measure:
//! - Document parsing and compilation
//! - BID generation and caching
//! - Multi-pass reference resolution
//! - Database interaction (when enabled)
//!
//! Run with: cargo bench --features service

use criterion::{criterion_group, criterion_main, Criterion};
use noet_core::{beliefbase::BeliefBase, codec::DocumentCompiler};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;

// Test corpus setup - mirrors codec_test/bid_tests.rs setup
fn setup_network_1() -> Result<(TempDir, PathBuf), Box<dyn std::error::Error>> {
    let test_tempdir = TempDir::new()?;
    let test_root = test_tempdir.path().to_path_buf();

    // Copy network_1 test corpus into temp directory
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/network_1");
    copy_dir_recursive(&source, &test_root)?;

    Ok((test_tempdir, test_root))
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// Benchmark: Full document parsing (mirrors basic_usage.rs)
fn bench_parse_all_documents(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("parse_all_documents", |b| {
        b.to_async(&rt).iter(|| async {
            let (_tempdir, test_root) = setup_network_1().unwrap();
            let mut compiler = DocumentCompiler::simple(test_root).unwrap();

            // Parse all documents (multi-pass compilation)
            let _results = compiler
                .parse_all(BeliefBase::default(), false)
                .await
                .unwrap();

            // Return node count for verification
            compiler.cache().states().len()
        });
    });
}

// Benchmark: BID generation and caching (mirrors bid_tests.rs)
fn bench_bid_generation_and_caching(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("bid_generation_and_caching", |b| {
        b.to_async(&rt).iter(|| async {
            let (_tempdir, test_root) = setup_network_1().unwrap();
            let (accum_tx, _accum_rx) = unbounded_channel();
            let mut compiler =
                DocumentCompiler::new(&test_root, Some(accum_tx), None, false).unwrap();

            // Parse with event accumulation
            let global_bb = BeliefBase::empty();
            let _results = compiler.parse_all(global_bb.clone(), false).await.unwrap();

            // Count nodes and BIDs
            compiler.cache().states().len()
        });
    });
}

// Benchmark: Multi-pass reference resolution
fn bench_multi_pass_compilation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("multi_pass_reference_resolution", |b| {
        b.to_async(&rt).iter(|| async {
            let (_tempdir, test_root) = setup_network_1().unwrap();
            let mut compiler = DocumentCompiler::simple(test_root).unwrap();

            // Force multiple passes by using clean cache
            let _pass1 = compiler
                .parse_all(BeliefBase::default(), false)
                .await
                .unwrap();

            // Second parse should be faster (cached BIDs)
            let _pass2 = compiler
                .parse_all(BeliefBase::default(), false)
                .await
                .unwrap();

            compiler.cache().states().len()
        });
    });
}

// Benchmark: Graph querying after compilation
fn bench_graph_queries(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Pre-compile the corpus once
    let (_tempdir, test_root) = setup_network_1().unwrap();
    let compiler = rt.block_on(async {
        let mut compiler = DocumentCompiler::simple(test_root).unwrap();
        compiler
            .parse_all(BeliefBase::default(), false)
            .await
            .unwrap();
        compiler
    });

    c.bench_function("graph_queries", |b| {
        b.iter(|| {
            // Query all document nodes
            let doc_count = compiler
                .cache()
                .states()
                .values()
                .filter(|node| node.kind.is_document())
                .count();

            // Query edges for each document
            let mut edge_count = 0;
            for node in compiler.cache().states().values() {
                if let Some(idx) = compiler.cache().bid_to_index(&node.bid) {
                    edge_count += compiler.cache().relations().as_graph().edges(idx).count();
                }
            }

            (doc_count, edge_count)
        });
    });
}

// Benchmark group configuration
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)  // Fewer samples for file I/O benchmarks
        .measurement_time(std::time::Duration::from_secs(10));
    targets =
        bench_parse_all_documents,
        bench_bid_generation_and_caching,
        bench_multi_pass_compilation,
        bench_graph_queries
}

criterion_main!(benches);
