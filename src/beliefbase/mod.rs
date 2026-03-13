//! BeliefBase module: Structured belief management system.
//!
//! This module provides the core belief system infrastructure for managing
//! states, relationships, and queries across belief nodes.
//!
//! # Module Organization
//!
//! - `graph`: Graph data structures (BidGraph, BidRefGraph, BeliefGraph)
//! - `context`: Context types for navigating relationships (BeliefContext, ExtendedRelation)
//! - `base`: Main BeliefBase implementation with state management
//!
//! # Public API
//!
//! The module re-exports all public types to maintain API compatibility:
//!
//! ```rust
//! use noet_core::beliefbase::{BeliefBase, BeliefGraph, BidGraph};
//! ```

mod base;
#[cfg(not(target_arch = "wasm32"))]
mod cached;
mod context;
mod graph;
#[cfg(not(target_arch = "wasm32"))]
mod sink;

#[cfg(test)]
mod tests;

// Re-export public types to maintain existing API
pub use base::BeliefBase;
#[cfg(not(target_arch = "wasm32"))]
pub use cached::CachedBeliefSource;
pub use context::{BeliefContext, ExtendedRelation};
pub use graph::{BeliefGraph, BidGraph, BidRefGraph, BidSubGraph};
#[cfg(not(target_arch = "wasm32"))]
pub use sink::BeliefSink;
