//! [`BeliefSink`] — trait for applying a coherent batch of [`BeliefEvent`]s to a
//! backing store.
//!
//! ## Motivation
//!
//! [`BeliefAccumulator`](super::accumulator::BeliefAccumulator) collects raw events
//! from the compiler's event channel between [`BeliefEvent::BatchStart`] /
//! [`BeliefEvent::BatchEnd`] sentinels, reorders them (node events first so that
//! relation/path events find their nodes already indexed), and then calls
//! [`BeliefSink::apply_batch`] once per completed batch.
//!
//! This keeps all batch-commit logic out of the backing stores — each impl just
//! processes a flat, already-ordered event slice.
//!
//! ## Impls
//!
//! | Type | Strategy |
//! |---|---|
//! | [`BeliefBase`] | `process_event` per event; derivatives handled internally |
//! | [`DbConnection`] | one [`Transaction`], `add_event` per event, `execute` at end |
//!
//! Both impls are native-only (`#[cfg(not(target_arch = "wasm32"))]`).

use crate::{event::BeliefEvent, BuildonomyError};

use super::BeliefBase;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Apply a coherent, pre-ordered batch of [`BeliefEvent`]s to a backing store.
///
/// Called by [`BeliefAccumulator`](super::accumulator::BeliefAccumulator) after
/// each [`BeliefEvent::BatchEnd`] with the collected events sorted node-events-first.
///
/// `BatchStart` and `BatchEnd` are **not** included in the slice — they are consumed
/// by the accumulator as control flow and never reach `apply_batch`.
///
/// `FileParsed` events **are** included so that `DbConnection` can track mtimes.
pub trait BeliefSink: Send {
    fn apply_batch(
        &mut self,
        events: &[BeliefEvent],
    ) -> impl std::future::Future<Output = Result<(), BuildonomyError>> + Send;
}

// ---------------------------------------------------------------------------
// impl BeliefBase
// ---------------------------------------------------------------------------

impl BeliefSink for BeliefBase {
    /// Apply each event via [`BeliefBase::process_event`].
    ///
    /// Derivative events (path mutations, reindex) are handled inside
    /// `process_event` itself — we discard the return value here.
    ///
    /// `index_sync` is triggered lazily on the next query via the `index_dirty`
    /// flag set by node inserts; no explicit sync call is needed here.
    async fn apply_batch(&mut self, events: &[BeliefEvent]) -> Result<(), BuildonomyError> {
        for event in events {
            // Ignore derivatives — process_event drives PathMapMap internally.
            let _ = self.process_event(event);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// impl DbConnection
// ---------------------------------------------------------------------------

use crate::db::{DbConnection, Transaction};

impl BeliefSink for DbConnection {
    /// Collect all events into a single [`Transaction`] and execute it atomically.
    ///
    /// This mirrors the watch service's transaction task behaviour: events are
    /// batched rather than committed one-by-one, giving SQLite a chance to write
    /// them in a single WAL commit.
    ///
    /// `FileParsed` events are included in the batch so mtime tracking stays
    /// consistent with node/relation writes.
    async fn apply_batch(&mut self, events: &[BeliefEvent]) -> Result<(), BuildonomyError> {
        let mut tx = Transaction::new();
        for event in events {
            tx.add_event(event)?;
        }
        if tx.has_pending() {
            tx.execute(&self.0).await?;
        }
        Ok(())
    }
}
