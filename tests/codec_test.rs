#![cfg(feature = "service")]

//! Codec integration tests
//!
//! This module contains comprehensive tests for the document codec system,
//! organized by feature area:
//!
//! - `bid_tests`: BID generation and caching
//! - `section_tests`: Section metadata and handling
//! - `anchor_tests`: Anchor generation and collision detection
//! - `link_tests`: Link resolution and formatting
//! - `asset_tests`: Asset tracking and content addressing

#[path = "codec_test/common.rs"]
mod common;

#[path = "codec_test/alias_tests.rs"]
mod alias_tests;
#[path = "codec_test/anchor_tests.rs"]
mod anchor_tests;
#[path = "codec_test/asset_tests.rs"]
mod asset_tests;
#[path = "codec_test/bid_tests.rs"]
mod bid_tests;
#[path = "codec_test/filter_tests.rs"]
mod filter_tests;
#[path = "codec_test/link_tests.rs"]
mod link_tests;
#[path = "codec_test/mapping_tests.rs"]
mod mapping_tests;
#[path = "codec_test/query_directive_tests.rs"]
mod query_directive_tests;
#[path = "codec_test/query_tests.rs"]
mod query_tests;
#[path = "codec_test/section_tests.rs"]
mod section_tests;

#[cfg(feature = "xlsx")]
#[path = "codec_test/xlsx_tests.rs"]
mod xlsx_tests;
