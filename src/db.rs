use crate::{
    beliefbase::{BeliefBase, BeliefGraph, BidGraph, EpochDrain},
    codec::{network::NETWORK_NAME, WALK_CODECS},
    error::BuildonomyError,
    event::BeliefEvent,
    nodekey::NodeKey,
    paths::{
        path::{os_path_to_string, string_to_os_path, to_anchor},
        pathmap::{parse_order, serialize_order},
        AnchorPath,
    },
    properties::{
        const_namespaces, BeliefKind, BeliefNode, BeliefRelation, Bid, Bref, EnumSet, NodeId,
        WeightKind, WeightSet,
    },
    query::{
        spec::{
            CompositionOp, PackageStage, Role, StepOperation, TapeContent, TapeEntry, TapeFn,
            TapePayload,
        },
        BeliefSource, BoxFuture, QueryPackage, SubmapResult,
    },
};
use rustc_hash::FxHashMap;
use sqlx::Execute;
use sqlx::{
    error::BoxDynError,
    migrate::{MigrateDatabase, Migration as SqlxMigration, MigrationSource, Migrator},
    sqlite::{Sqlite, SqliteConnectOptions},
    ConnectOptions, Row,
};
use sqlx::{migrate::MigrationType, pool::PoolOptions, Pool, QueryBuilder};
use std::{collections::BTreeMap, result::Result};
use std::{
    collections::BTreeSet,
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
    time::SystemTime,
};

pub const BELIEF_CACHE_DB: &str = "sqlite:belief_cache.db";

pub struct Transaction<'a> {
    qb: QueryBuilder<'a, Sqlite>,
    pub staged: usize,
    /// Running count of bind variables pushed into `qb`. Used to flush
    /// before hitting SQLite's `SQLITE_MAX_VARIABLE_NUMBER` limit.
    bind_count: usize,
    /// Overflow buffer: when `bind_count` approaches the limit, the current
    /// `qb` is flushed here and a fresh one is started.
    overflow: Vec<QueryBuilder<'a, Sqlite>>,
    mtime_updates: BTreeMap<String, i64>,
    /// BIDs of network nodes inserted/updated in this transaction.
    /// Used by `execute` to reconcile `paths.is_net` for entries whose
    /// target matches one of these BIDs.
    dirty_net_bids: Vec<String>,
}

impl<'a> Transaction<'a> {
    /// Returns true if there is anything to commit — either staged belief-graph events
    /// or pending mtime updates. Use this instead of checking `staged > 0` directly so
    /// that `FileParsed`-only batches (zero staged events, non-empty mtime_updates) are
    /// not silently dropped.
    pub fn has_pending(&self) -> bool {
        self.staged > 0 || !self.mtime_updates.is_empty()
    }
}

impl<'a> Default for Transaction<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Transaction<'a> {
    pub fn new() -> Transaction<'a> {
        Transaction {
            qb: QueryBuilder::<Sqlite>::new(""),
            staged: 0,
            bind_count: 0,
            overflow: Vec::new(),
            mtime_updates: BTreeMap::new(),
            dirty_net_bids: Vec::new(),
        }
    }

    /// Flush the current QueryBuilder into `overflow` and start a fresh one
    /// when the accumulated bind count is approaching SQLite's limit.
    /// Called after each event is added.
    fn maybe_flush(&mut self) {
        // Flush well before SQLite's bind-variable limit.  The compile-time
        // SQLITE_MAX_VARIABLE_NUMBER varies by build: noet-core's bundled
        // SQLite uses 32,766, but system/brew SQLite defaults to 999.
        // Use 900 as the threshold to stay safe on both.
        if self.bind_count > 900 {
            let full_qb = std::mem::replace(&mut self.qb, QueryBuilder::<Sqlite>::new(""));
            self.overflow.push(full_qb);
            self.bind_count = 0;
        }
    }

    /// Execute all staged statements as a single atomic SQL transaction.
    ///
    /// Wraps the entire body in `BEGIN`/`COMMIT` so that a mid-batch failure
    /// (e.g. one overflow chunk fails) rolls back everything already applied
    /// in this call rather than leaving a partially-committed batch. Before
    /// this wrapping was added, atomicity was provided *only* by the caller
    /// holding a Rust-level lock across the whole call — a correctness gap
    /// once concurrent readers are allowed to interleave with a write (see
    /// noet-core Issue 100).
    pub async fn execute(&mut self, connection: &Pool<Sqlite>) -> Result<(), BuildonomyError> {
        // When network nodes were inserted/updated in this batch, append a
        // fixup UPDATE to reconcile `paths.is_net`.  `add_paths` computes
        // `is_net` via a subquery against `beliefs`, but the `beliefs` row
        // may not exist yet when the `PathAdded` event is processed (both
        // events can arrive in the same batch).  This single UPDATE at the
        // end of the transaction corrects any stale 0 values.
        let dirty_net_bids = std::mem::take(&mut self.dirty_net_bids);

        let mut tx = connection.begin().await?;

        // Execute overflow batches first (flushed mid-transaction to stay
        // within SQLite's bind-variable limit), then the final qb.
        for mut overflow_qb in self.overflow.drain(..) {
            let query = overflow_qb.build();
            query.execute(&mut *tx).await?;
        }
        let query = self.qb.build();
        query.execute(&mut *tx).await?;
        self.qb.reset();
        self.bind_count = 0;

        // Fixup: set is_net = 1 for any paths entry whose target is one of
        // the network BIDs we just inserted/updated.  This corrects entries
        // that were inserted by add_paths before the network's beliefs row
        // existed (cross-batch ordering: PathAdded arrives in an earlier
        // epoch than the network's NodeUpdate).
        // Chunk the dirty_net_bids fixup to stay within bind limits.
        for chunk in dirty_net_bids.chunks(CHUNK_SIZE) {
            let mut fixup_qb = QueryBuilder::<Sqlite>::new(
                "UPDATE paths SET is_net = 1 WHERE is_net = 0 AND path != '' \
                 AND ordering != '65535' AND ordering NOT LIKE '65535.%' \
                 AND target IN (",
            );
            let mut sep = fixup_qb.separated(", ");
            for bid_str in chunk {
                sep.push_bind(bid_str.clone());
            }
            fixup_qb.push(")");
            fixup_qb.build().execute(&mut *tx).await?;
        }

        // Batch insert mtime updates, chunked to stay within bind limits.
        // Each row uses 2 bind variables (path + mtime).
        if !self.mtime_updates.is_empty() {
            let mtime_entries: Vec<_> = std::mem::take(&mut self.mtime_updates)
                .into_iter()
                .collect();
            for chunk in mtime_entries.chunks(CHUNK_SIZE / 2) {
                let mut mtime_qb = QueryBuilder::<Sqlite>::new(
                    "INSERT OR REPLACE INTO file_mtimes (path, mtime) ",
                );
                mtime_qb.push_values(chunk.iter(), |mut b, (path, mtime)| {
                    b.push_bind(path.clone()).push_bind(*mtime);
                });
                mtime_qb.build().execute(&mut *tx).await?;
            }
        }

        tx.commit().await?;

        Ok(())
    }

    pub fn track_file_mtime(&mut self, path: &Path) -> Result<(), BuildonomyError> {
        // Canonicalize to resolve Windows 8.3 short-name aliases and symlinks so that
        // the same file is always stored under a single consistent key regardless of
        // which alias the compiler or file-watcher happened to use at call time.
        let canonical =
            crate::paths::canonicalize_path(path).unwrap_or_else(|_| path.to_path_buf());
        match fs::metadata(&canonical) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified) => match modified.duration_since(SystemTime::UNIX_EPOCH) {
                    Ok(duration) => {
                        let mtime = duration.as_secs() as i64;
                        let path_str = os_path_to_string(&canonical);
                        self.mtime_updates.insert(path_str.clone(), mtime);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[Transaction]   ✗ Failed to get duration since epoch for {:?}: {}",
                            path,
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "[Transaction]   ✗ Failed to get modified time for {:?}: {}",
                        path,
                        e
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    "[Transaction]   ✗ Failed to get metadata for {:?} (canonical: {:?}): {} (path may not exist or be inaccessible)",
                    path,
                    canonical,
                    e
                );
                tracing::warn!("[Transaction]   errno/kind: {:?}", e.kind());
            }
        }
        Ok(())
    }

    // Bool return value lets the caller know whether rewrite paths should be called.
    pub fn add_event(&mut self, event: &BeliefEvent) -> Result<(), BuildonomyError> {
        // Flush the current QueryBuilder if we're approaching SQLite's
        // bind-variable limit, before appending the next event's SQL.
        self.maybe_flush();

        match event {
            BeliefEvent::NodeUpdate(_keys, node, _) => {
                // Merge keys are intentionally not resolved here.
                //
                // A `NodeUpdate`'s keys can name some *other* node that this one
                // absorbs. That resolution happens once, upstream, in
                // `resolve_merge_keys` at `BatchEnd`, which turns each absorption into
                // explicit `NodeRenamed` + `NodesRemoved` events. By the time a batch
                // reaches this sink the decision is already encoded in the event
                // stream, so both sinks agree without either interpreting keys.
                //
                // Doing it here instead would mean a SELECT per key inside transaction
                // assembly, and `Transaction` is deliberately write-only and batched
                // (see `maybe_flush` and the bind budgeting).
                self.update_node(node);
                self.bind_count += 9; // beliefs table: 9 columns
            }
            BeliefEvent::NodeUpsert(_bid, node, _) => {
                self.update_node(node);
                self.bind_count += 9;
            }
            BeliefEvent::NodesRemoved(bids, _) => {
                self.remove_nodes(bids);
                self.bind_count += bids.len();
            }
            BeliefEvent::PathsRemoved(net, paths, _) => {
                self.remove_paths(net, paths);
                self.bind_count += 1 + paths.len();
            }
            BeliefEvent::PathAdded(net, path, bid, order, _) => {
                self.add_paths(net, vec![(path, *bid, order)]);
                self.bind_count += 7; // net, path, target, ordering, is_net subquery binds
            }
            BeliefEvent::PathUpdate(net, path, bid, order, _) => {
                self.add_paths(net, vec![(path, *bid, order)]);
                self.bind_count += 7;
            }
            BeliefEvent::NodeRenamed(from, to, _) => {
                self.rename_node(from, to);
                // beliefs delete (1) + paths re-target (2), plus 2 more when the
                // network bref fixup fires.
                self.bind_count += 5;
            }
            BeliefEvent::RelationChange(..) => {
                // Don't process these, wait to get the resolved entire RelationUpdate event
            }
            BeliefEvent::RelationUpdate(source, sink, weight_set, _) => {
                self.update_relation(source, sink, weight_set);
                self.bind_count += 7; // sink, source, 3 weights, owned_by
            }
            BeliefEvent::RelationRemoved(source, sink, _) => {
                self.remove_relation(source, sink);
                self.bind_count += 2;
            }
            BeliefEvent::FileParsed(path) => {
                self.track_file_mtime(path)?;
            }
            BeliefEvent::BatchStart | BeliefEvent::BatchEnd => {
                // No-op at the backing-store level. All batch semantics (event collection,
                // node-first reordering, Transaction::execute) are owned by
                // BeliefAccumulator. add_event only sees the reordered event slice
                // after BatchEnd has been processed by the accumulator.
            }
            BeliefEvent::BuiltInTest => {
                tracing::debug!(
                    "BuiltInTest: All BeliefBase invariants *should* be true now but we're not checking."
                );
            }
        }
        Ok(())
    }

    fn update_node(&mut self, belief: &BeliefNode) {
        self.maybe_flush();
        if belief.kind.is_network() {
            self.dirty_net_bids.push(belief.bid.to_string());
        }
        self.qb
            .push("INSERT OR REPLACE INTO beliefs(bid, bref, kind, title, schema, payload, id, metadata, title_slug) ");
        self.qb.push_values(vec![belief], |mut b, belief| {
            let metadata_str = if belief.metadata.is_empty() {
                None
            } else {
                Some(belief.metadata.to_string())
            };
            // title_slug: the to_anchor() of the title, used as a fallback id key in the DB
            // for nodes that have no explicit id stored (id = NULL).  Mirrors what the
            // in-memory PathMap.id_map stores via the node.id() runtime fallback.
            let title_slug = {
                let slug = to_anchor(&belief.title);
                if slug.is_empty() {
                    None
                } else {
                    Some(slug)
                }
            };
            b.push_bind::<String>(belief.bid.into())
                .push_bind::<String>(belief.bid.bref().to_string())
                .push_bind(belief.kind.as_u32())
                .push_bind::<String>(belief.title.clone())
                .push_bind::<Option<String>>(belief.schema.clone())
                .push_bind::<String>(belief.payload.to_string())
                .push_bind::<Option<String>>(match &belief.id {
                    NodeId::Explicit(s) => Some(s.clone()),
                    NodeId::Collision(slug) => {
                        Some(format!("{}{}", crate::properties::COLLISION_PREFIX, slug))
                    }
                    NodeId::Slug => None,
                })
                .push_bind::<Option<String>>(metadata_str)
                .push_bind::<Option<String>>(title_slug);
        });
        self.qb.push("; ");
        self.staged += 1;
    }

    fn remove_nodes(&mut self, nodes: &[Bid]) {
        if nodes.is_empty() {
            return;
        }
        self.qb.push("DELETE from beliefs WHERE ");
        push_string_expr(
            &mut self.qb,
            &nodes.iter().map(|b| b.to_string()).collect::<Vec<String>>(),
            "bid",
            true,
            true,
        );
        self.qb.push("; ");
        self.staged += 1;
    }
}

/// Build an `IN(...)` / `NOT IN(...)` clause binding each element as a parameter.
/// Maximum bind variables per `IN(...)` clause to stay within SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` limit (default 999 in most builds).
const CHUNK_SIZE: usize = 500;

/// Build a `column IN (?, ?, ...)` (or `NOT IN`) expression, automatically
/// chunking into multiple `IN` clauses joined with `OR` (or `AND` for
/// `NOT IN`) when the list exceeds [`CHUNK_SIZE`] to avoid hitting SQLite's
/// bind-variable limit.
fn push_id_expr<I: ToString>(
    qb: &mut QueryBuilder<Sqlite>,
    bids: &[I],
    column: &str,
    match_pred: bool,
) {
    if bids.is_empty() {
        // Empty list: IN() is always false, NOT IN() is always true.
        if match_pred {
            qb.push("0 ");
        } else {
            qb.push("1 ");
        }
        return;
    }

    let chunks: Vec<&[I]> = bids.chunks(CHUNK_SIZE).collect();
    let needs_wrap = chunks.len() > 1;
    let joiner = if match_pred { " OR " } else { " AND " };

    if needs_wrap {
        qb.push("(");
    }

    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        qb.push(column);
        if match_pred {
            qb.push(" IN(");
        } else {
            qb.push(" NOT IN(");
        }
        for (i, bid) in chunk.iter().enumerate() {
            qb.push_bind::<String>(bid.to_string());
            if i < chunk.len() - 1 {
                qb.push(", ");
            }
        }
        qb.push(")");
        if chunk_idx < chunks.len() - 1 {
            qb.push(joiner);
        }
    }

    if needs_wrap {
        qb.push(") ");
    } else {
        qb.push(" ");
    }
}

/// Build a column match clause using `GLOB` (starts_with) or `=` per element.
fn push_string_expr(
    qb: &mut QueryBuilder<Sqlite>,
    strings: &[String],
    column: &str,
    match_pred: bool,
    starts_with: bool,
) {
    let last_sep = if !strings.is_empty() {
        strings.len() - 1
    } else {
        0
    };
    for (idx, string) in strings.iter().enumerate() {
        qb.push(format!(
            "{} {}{} ",
            column,
            if match_pred {
                ""
            } else if starts_with {
                "NOT "
            } else {
                "!"
            },
            if starts_with { "GLOB concat(" } else { "=" },
        ));
        qb.push_bind(string.clone());
        if starts_with {
            qb.push(", '*')");
        }
        if idx < last_sep {
            if match_pred {
                qb.push(" OR ");
            } else {
                qb.push(" AND ");
            }
        }
    }
}

impl Transaction<'_> {
    /// Delete `from` and re-target its `paths` entries at `to`.
    ///
    /// **Deliberately does not touch `relations`.** Edge re-pointing is expanded
    /// upstream by `expand_renames` in `beliefbase/accumulator.rs`, which turns
    /// every `NodeRenamed` into explicit `RelationRemoved` + `RelationUpdate`
    /// events before any sink sees the batch.
    ///
    /// That split exists because the two sinks could not agree otherwise. When
    /// `from` and `to` share a neighbour — the normal case for the absorption this
    /// serves, since an href stub and the content node claiming its URL both hold
    /// a Section edge to the href namespace — the in-memory `replace_bid` unions
    /// the two `WeightSet`s. This path cannot: weights live in serialized TEXT
    /// columns. Expanding upstream computes the union once, so both sinks apply
    /// the same result through `update_relation`, whose `INSERT OR REPLACE` also
    /// resolves the `UNIQUE(sink, source)` collision natively rather than needing
    /// conflicting rows deleted first.
    fn rename_node(&mut self, from: &Bid, to: &Bid) {
        self.qb.push("DELETE from beliefs WHERE bid = ");
        self.qb.push_bind::<String>(from.into());
        self.qb.push("; ");

        // `paths` is UNIQUE(net, path); re-targeting touches neither, so no
        // conflict is possible here.
        self.qb.push(" UPDATE paths SET target = ");
        self.qb.push_bind::<String>(to.into());
        self.qb.push(" WHERE target = ");
        self.qb.push_bind::<String>(from.into());
        self.qb.push("; ");
        // When a network node is renamed, its bref changes.  Child entries
        // in the paths table have `net = old_bref` which must be updated to
        // the new bref so that queries by the new bref still find them.
        let from_bref = from.bref().to_string();
        let to_bref = to.bref().to_string();
        if from_bref != to_bref {
            self.qb.push(" UPDATE paths SET net = ");
            self.qb.push_bind::<String>(to_bref);
            self.qb.push(" WHERE net = ");
            self.qb.push_bind::<String>(from_bref);
            self.qb.push(";");
        }
        self.staged += 1;
    }

    fn add_paths(&mut self, net: &Bref, paths: Vec<(&String, Bid, &Vec<u16>)>) {
        if paths.is_empty() {
            return;
        }
        let network_kind_mask = EnumSet::only(BeliefKind::Network).as_u32();
        for (path, target, order_vec) in paths {
            let order_str = serialize_order(order_vec);
            let target_str: String = target.into();
            // The "" and network-filename root entries (e.g. "index.md") point
            // at the network node itself.  They must NOT have is_net = 1 or
            // the DB submap's subnet-expansion loop will recurse into the
            // network's own PathMap infinitely.
            let is_root_entry = path.is_empty()
                || order_vec.contains(&crate::paths::pathmap::NETWORK_SECTION_SORT_KEY);
            if is_root_entry {
                self.qb.push(
                    "INSERT OR REPLACE INTO paths(net, path, target, ordering, is_net) VALUES (",
                );
                self.qb.push_bind::<String>(net.to_string());
                self.qb.push(", ");
                self.qb.push_bind::<String>(path.clone());
                self.qb.push(", ");
                self.qb.push_bind::<String>(target_str);
                self.qb.push(", ");
                self.qb.push_bind::<String>(order_str);
                self.qb.push(", 0); ");
            } else {
                self.qb.push(
                    "INSERT OR REPLACE INTO paths(net, path, target, ordering, is_net) \
                     SELECT ",
                );
                self.qb.push_bind::<String>(net.to_string());
                self.qb.push(", ");
                self.qb.push_bind::<String>(path.clone());
                self.qb.push(", ");
                self.qb.push_bind::<String>(target_str.clone());
                self.qb.push(", ");
                self.qb.push_bind::<String>(order_str);
                self.qb.push(", COALESCE((SELECT (kind & ");
                self.qb.push_bind(network_kind_mask);
                self.qb.push(" != 0) FROM beliefs WHERE bid = ");
                self.qb.push_bind::<String>(target_str);
                self.qb.push("), 0); ");
            }
            self.staged += 1;
        }
    }

    fn remove_paths(&mut self, net: &Bref, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        self.qb.push("DELETE from paths WHERE net = ");
        self.qb.push_bind::<String>(net.to_string());
        self.qb.push(" AND ");
        push_string_expr(&mut self.qb, paths, "path", true, true);
        self.qb.push("; ");
        self.staged += 1;
    }

    fn update_relation(&mut self, source: &Bid, sink: &Bid, weight_set: &WeightSet) {
        if weight_set.is_empty() {
            self.remove_relation(source, sink);
        } else {
            self.qb.push(
                "INSERT OR REPLACE INTO relations \
                 (sink, source, epistemic, section, pragmatic, owned_by) ",
            );
            self.qb.push_values(
                vec![(source, sink, weight_set)],
                |mut b, (source, sink, weight)| {
                    // Serialize each Weight to TOML string for storage
                    let serialize_weight = |w: &crate::properties::Weight| -> String {
                        toml::to_string(w).unwrap_or_default()
                    };

                    // Extract the owned_by value from whichever weight kind carries it.
                    // A given edge has a single owner across all kinds, so first-match is correct.
                    let owned_by: Option<String> = WeightKind::all().iter().find_map(|kind| {
                        weight.get(kind).and_then(|w: &crate::properties::Weight| {
                            w.get::<String>(crate::properties::WEIGHT_OWNED_BY)
                        })
                    });

                    b.push_bind::<String>(sink.to_string())
                        .push_bind::<String>(source.to_string())
                        .push_bind(weight.get(&WeightKind::Epistemic).map(serialize_weight))
                        .push_bind(weight.get(&WeightKind::Section).map(serialize_weight))
                        .push_bind(weight.get(&WeightKind::Pragmatic).map(serialize_weight))
                        .push_bind(owned_by);
                },
            );
            self.qb.push("; ");
            self.staged += 1;
        }
    }

    fn remove_relation(&mut self, source: &Bid, sink: &Bid) {
        self.qb.push("DELETE from relations where source = ");
        self.qb.push_bind::<String>(source.into());
        self.qb.push(" and sink = ");
        self.qb.push_bind::<String>(sink.into());
        self.qb.push("; ");
        self.staged += 1;
    }
}

// No 'futures' crate needed!
// This is exactly what BoxFuture<'static, u32> expands to.
type NestedNetFuture = Pin<
    Box<
        dyn Future<Output = Result<Vec<(String, Bid, Vec<u16>, bool)>, BuildonomyError>>
            + Send
            + 'static,
    >,
>;

/// Return type for the recursive `resolve_net_id_recursive` free function.
type NestedNetIdFuture =
    Pin<Box<dyn Future<Output = Result<Option<Bid>, BuildonomyError>> + Send + 'static>>;

/// Recursively search `net` and all its subnets for a node whose explicit `id`
/// (or `title_slug` fallback) matches `id`.
///
/// Mirrors the in-memory `PathMap::get_from_id` recursive subnet walk.  The
/// `processed_nets` guard prevents cycles in pathological graphs.
fn resolve_net_id_recursive(
    pool: Pool<Sqlite>,
    net: Bref,
    id: String,
    processed_nets: BTreeSet<Bref>,
) -> NestedNetIdFuture {
    Box::pin(async move {
        if processed_nets.contains(&net) {
            return Ok(None);
        }

        // Single query per level: return rows that either match the id/title_slug OR are
        // subnet entries (is_net = 1).  This lets us check for a direct hit and collect
        // subnets to recurse into with one round-trip instead of two.
        //
        // Columns: (bid, is_id_match)
        //   bid          — the target BID for the path entry
        //   is_id_match  — 1 if this row satisfies the id/title_slug predicate, 0 if it
        //                  is a subnet candidate only
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT DISTINCT p.target, \
                CASE WHEN (b.id = ? OR (b.id IS NULL AND b.title_slug = ?)) THEN 1 ELSE 0 END \
             FROM paths p \
             JOIN beliefs b ON b.bid = p.target \
             WHERE p.net = ? AND ((b.id = ? OR (b.id IS NULL AND b.title_slug = ?)) OR p.is_net = 1)",
        )
        .bind(&id)
        .bind(&id)
        .bind(net.to_string())
        .bind(&id)
        .bind(&id)
        .fetch_all(&pool)
        .await?;

        let mut new_processed = processed_nets;
        new_processed.insert(net);

        let mut subnets: Vec<Bid> = Vec::new();
        for (bid_str, is_id_match) in rows {
            let Ok(bid) = Bid::try_from(bid_str.as_str()) else {
                continue;
            };
            if is_id_match != 0 {
                // Direct match at this level — return immediately.
                return Ok(Some(bid));
            }
            // Subnet candidate — collect for recursion below.
            if !new_processed.contains(&bid.bref()) {
                subnets.push(bid);
            }
        }

        for subnet_bid in subnets {
            let subnet_bref = subnet_bid.bref();
            if let Some(found) = resolve_net_id_recursive(
                pool.clone(),
                subnet_bref,
                id.clone(),
                new_processed.clone(),
            )
            .await?
            {
                return Ok(Some(found));
            }
        }

        Ok(None)
    })
}

#[derive(Debug, Clone)]
pub struct DbConnection(pub Pool<Sqlite>);

// ---------------------------------------------------------------------------
// EpochDrain — no-op impl for DbConnection (sequential / test path)
// ---------------------------------------------------------------------------

/// `DbConnection` is used directly in the sequential parse path and in tests
/// (e.g. `bid_tests.rs`).  It has no event channel to drain, so `drain_epoch`
/// is a no-op — the same reasoning as the `BeliefBase` impl.
impl EpochDrain for DbConnection {
    fn drain_epoch(&self) -> impl std::future::Future<Output = Result<(), BuildonomyError>> + Send {
        std::future::ready(Ok(()))
    }
}

/// TODO: ensure push_values iter counts never exceed this huge value
///
/// <https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values>
/// <https://www.sqlite.org/limits.html#max_variable_number>
pub const SQLITE_LIMIT_VARIABLE_NUMBER: usize = 32766;

impl DbConnection {
    /// Resolve a `NodeKey::Id { net, id }` lookup against the DB, with a title-slug fallback.
    ///
    /// Primary lookup: matches nodes with an explicit `id` field OR a matching `title_slug`
    /// stored in the DB.  `title_slug` is the `to_anchor(title)` value written at insert time,
    /// mirroring what `PathMap.id_map` stores in memory via the `node.id()` runtime fallback.
    ///
    /// This covers nodes pushed without an explicit `{#anchor}` (e.g. network index files),
    /// which are stored with `id = NULL` but whose title slug is the canonical lookup key.
    ///
    /// Returns `None` if no match is found (including after recursive subnet search).
    async fn resolve_net_id(&self, net: Bref, id: &str) -> Result<Option<Bid>, BuildonomyError> {
        // Delegate to the recursive free function so that cross-network id lookups
        // (e.g. `Id { net: repo_bref, id: "ticket-763" }` where the node lives in a
        // sub-subnet) mirror the in-memory `PathMap::get_from_id` recursive subnet walk.
        resolve_net_id_recursive(self.0.clone(), net, id.to_string(), BTreeSet::default()).await
    }

    /// Resolve a network-relative path against `net` by walking the `paths` table.
    ///
    /// Algorithm per level:
    ///
    /// 1. Normalize: a bare anchor `"#slug"` is rewritten to `"index.md#slug"` because
    ///    the only valid referent of a naked anchor is the current network's index.md.
    /// 2. Query `WHERE net=? AND (path=? OR is_net=1)` — one round-trip returns both the
    ///    direct match (if any) and every subnet child registered under `net`.
    /// 3. If the direct match is in the result set, return it immediately.
    /// 4. Scan subnets for a prefix of `path` (followed by `'/'` or `'#'`):
    ///    - `'/'` boundary → strip the prefix and recurse with `(subnet_bref, remainder)`.
    ///    - `'#'` boundary → sections of a network's index.md are stored as
    ///      `"index.md#slug"` under that subnet's own bref; recurse with
    ///      `(subnet_bref, "index.md#anchor")`.
    /// 5. No match → `None`.
    ///
    /// `is_net` in the paths table is the authoritative network flag — no extension
    /// heuristics or cross-joins to the beliefs table are needed.
    ///
    /// Returns `None` if any segment is not found or does not resolve to a valid BID.
    async fn resolve_net_path(
        &self,
        net: Bref,
        path: &str,
    ) -> Result<Option<Bid>, BuildonomyError> {
        // Normalize bare anchors: "#slug" is always a section of the current net's
        // index.md.  Rewriting here lets the direct-match query below find it without
        // any special-casing further down.
        let normalized;
        // TODO(Issue 67): use payload["codec"] from the network node when available
        // to select the correct network filename instead of hardcoding NETWORK_NAME.
        let path = if path.starts_with('#') {
            normalized = format!("{}{}", NETWORK_NAME, path);
            normalized.as_str()
        } else {
            path
        };

        // Single query: fetch the direct match (path = ?) plus all subnet children
        // (is_net = 1) registered under `net`.  Columns: (stored_path, target, is_net).
        //
        // PathMap synthesises ("", net_bid) and ("index.md", net_bid) in its constructor
        // without emitting PathAdded events, so those two entries are absent from the DB.
        // A bare "index.md" lookup therefore falls through to Ok(None) and is handled
        // correctly by the in-memory PathMap.
        let rows = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT path, target, is_net FROM paths \
             WHERE net = ? AND (path = ? OR is_net = 1)",
        )
        .bind(net.to_string())
        .bind(path)
        .fetch_all(&self.0)
        .await?;

        // Partition: direct match vs subnet children.
        let mut subnets: Vec<(String, Bid)> = Vec::new(); // (stored_path, subnet_bid)
        for (stored_path, target_str, row_is_net) in &rows {
            let Ok(target) = Bid::try_from(target_str.as_str()) else {
                continue;
            };
            if stored_path == path {
                // Direct match — return immediately.
                return Ok(Some(target));
            }
            if *row_is_net != 0 {
                subnets.push((stored_path.clone(), target));
            }
        }

        // Scan subnets for a stored path that is a prefix of `path`, terminated by '/'
        // or '#'.  Subnet stored paths may be multi-segment when non-network intermediate
        // directories exist between two network nodes (e.g. "src/core" stored under the
        // horizon net when src/ has no index.md).  Using starts_with here correctly
        // handles both single- and multi-segment subnet paths, and unifies the
        // slash-descent and anchor-descent cases without splitting on the first delimiter
        // upfront.  At most one subnet entry will match at any given level since subnet
        // paths within a network are non-overlapping.
        for (subnet_path, subnet_bid) in &subnets {
            // Check for slash boundary: "subnet_path/" prefix.
            let slash_prefix = format!("{}/", subnet_path);
            if let Some(remainder) = path.strip_prefix(slash_prefix.as_str()) {
                // If the remainder is "index.md" or empty (with no anchor), the subnet
                // node itself is the target.  PathMap synthesises these entries without
                // emitting PathAdded events — return the BID directly.
                let remainder_is_terminal = (remainder.is_empty()
                    || WALK_CODECS.is_network_file(remainder))
                    && !remainder.contains('#');
                if remainder_is_terminal {
                    return Ok(Some(*subnet_bid));
                }
                return Box::pin(self.resolve_net_path(subnet_bid.bref(), remainder)).await;
            }

            // Check for hash boundary: "subnet_path#" prefix.
            // Sections of a network's index.md are stored as "index.md#slug" under its
            // own bref.
            let hash_prefix = format!("{}#", subnet_path);
            if let Some(anchor) = path.strip_prefix(hash_prefix.as_str()) {
                // TODO(Issue 67): use payload["codec"] from the network node when available
                // to select the correct network filename instead of hardcoding NETWORK_NAME.
                let index_anchor = format!("{}#{}", NETWORK_NAME, anchor);
                let anchor_row = sqlx::query_as::<_, (String,)>(
                    "SELECT target FROM paths WHERE net = ? AND path = ? LIMIT 1",
                )
                .bind(subnet_bid.bref().to_string())
                .bind(&index_anchor)
                .fetch_optional(&self.0)
                .await?;
                return Ok(anchor_row.and_then(|(s,)| Bid::try_from(s.as_str()).ok()));
            }
        }

        // No subnet prefix matched and no direct hit — path not found at this level.
        // (e.g. "index.md" whose PathMap entry is synthesised without a PathAdded event.)
        //
        // Note: paths containing '#' that weren't caught above are either:
        // - Verbatim-stored anchored sections (caught by direct match at step 3), or
        // - Cross-network anchored references whose subnet prefix was found in step 4.
        // No further fallback is needed.
        Ok(None)
    }

    /// Resolve a seed [`TapeFn`] to a set of BIDs by dispatching each variant
    /// to the appropriate DB lookup.
    ///
    /// This is the DB counterpart of [`BeliefBase::eval_seed`]. Each
    /// `NodeKey` variant is resolved via direct SQL queries.
    async fn resolve_seed(&self, seed: &TapeFn) -> Result<Vec<(usize, Bid)>, BuildonomyError> {
        match seed {
            TapeFn::Bids(bids) => Ok(bids.iter().enumerate().map(|(i, b)| (i, *b)).collect()),
            TapeFn::Keys(keys) => {
                let mut bids = Vec::with_capacity(keys.len());
                for (i, key) in keys.iter().enumerate() {
                    match key {
                        NodeKey::Bid { bid } => bids.push((i, *bid)),
                        NodeKey::Bref { bref } => {
                            let row = sqlx::query_as::<_, (String,)>(
                                "SELECT bid FROM beliefs WHERE bref = ? LIMIT 1",
                            )
                            .bind(bref.to_string())
                            .fetch_optional(&self.0)
                            .await?;
                            if let Some((bid_str,)) = row {
                                if let Ok(bid) = Bid::try_from(bid_str.as_str()) {
                                    bids.push((i, bid));
                                }
                            }
                        }
                        NodeKey::Path { net, path } => {
                            // Normalize Bref::default() to the API node's bref,
                            // mirroring PathMapMap::net_get_from_path.
                            let resolved_net = if net.is_default() {
                                crate::properties::buildonomy_api_bid(env!("CARGO_PKG_VERSION"))
                                    .bref()
                            } else {
                                *net
                            };
                            if let Some(bid) = self.resolve_net_path(resolved_net, path).await? {
                                bids.push((i, bid));
                            }
                        }
                        NodeKey::Id { net, id } => {
                            // Same normalization as Path above.
                            let resolved_net = if net.is_default() {
                                crate::properties::buildonomy_api_bid(env!("CARGO_PKG_VERSION"))
                                    .bref()
                            } else {
                                *net
                            };
                            if let Some(bid) = self.resolve_net_id(resolved_net, id).await? {
                                bids.push((i, bid));
                            }
                        }
                    }
                }
                Ok(bids)
            }
            TapeFn::Corpus => {
                let rows = sqlx::query_as::<_, (String,)>("SELECT bid FROM beliefs")
                    .fetch_all(&self.0)
                    .await?;
                Ok(rows
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, (s,))| Bid::try_from(s.as_str()).ok().map(|b| (i, b)))
                    .collect())
            }
            TapeFn::DocumentNodes(net, doc_path) => {
                // Fetch the document node itself + all "doc_path#section" children.
                let prefix = format!("{}#", doc_path);
                let net_str = net.to_string();
                let rows = sqlx::query_as::<_, (String,)>(
                    "SELECT target FROM paths WHERE net = ? AND (path = ? OR path LIKE ?)",
                )
                .bind(&net_str)
                .bind(doc_path)
                .bind(format!("{}%", prefix))
                .fetch_all(&self.0)
                .await?;
                Ok(rows
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, (s,))| Bid::try_from(s.as_str()).ok().map(|b| (i, b)))
                    .collect())
            }
            _ => Err(BuildonomyError::Command(
                "Non-seed TapeFn variant passed to resolve_seed".to_string(),
            )),
        }
    }

    /// Walk edges via SQL per the [`TraversalSpec`] contract.
    ///
    /// Each hop issues one SQL query against the `relations` table,
    /// filtered by the `kind_filter` weight columns. The frontier
    /// advances by collecting output-role endpoints from matched edges.
    /// Apply a traversal via SQL, incrementally building edges into the
    /// package graph and recording per-hop `TapeContent::Edges` entries.
    /// Returns the accumulated output BIDs.
    ///
    /// Each SQL hop fetches full `BeliefRelation` rows (`SELECT *`) and
    /// adds them as edges in the package graph. The resulting `EdgeIndex`
    /// values are recorded in the tape, making the DB path structurally
    /// equivalent to the in-memory path.
    async fn apply_traversal_sql(
        &self,
        current: &BTreeSet<Bid>,
        trav: &crate::query::spec::TraversalSpec,
        label: &str,
        package: &mut QueryPackage,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        let tape_start = package.tape().len();
        let mut result = BTreeSet::new();
        // Filter const namespace BIDs (href, asset, buildonomy, codec) from the
        // traversal frontier, but ONLY for halo-shaped traversals (kind_filter
        // matches every WeightKind). These are hub nodes whose 1-hop, all-kind,
        // all-role halo fans out to every document in the corpus — that explosion
        // is what this guard exists to prevent; their children are looked up
        // individually by key in that case, never enumerated via graph traversal.
        //
        // This must NOT apply to restricted single-kind traversals such as
        // `leaf_map()` / `balance_map()` (Section-only). `sync_asset_snapshot`
        // and `initialize_stack` legitimately seed a leaf-ward Section walk AT a
        // const-namespace BID specifically to enumerate its Section-source
        // children (assets/hrefs) — stripping the seed here silently empties the
        // frontier and turns the traversal into a no-op. See noet-core Issue 98.
        //
        // PERFORMANCE NOTE: allowing the leaf-ward walk through here is a
        // deliberate, *bounded* exception, not a general relaxation. In practice
        // only Section-kind edges point at a const-namespace BID (asset/href
        // nodes registered via `GraphBuilder::process_asset`), so this query WILL
        // return every asset/href node in the corpus in one shot — easily
        // thousands of rows on a large corpus. That is by design for this one
        // traversal shape (it is the whole point of `sync_asset_snapshot` /
        // `initialize_stack`'s const-namespace seeding: pull the entire
        // asset/href subgraph in a single query rather than one per file), but
        // it means this bypass must stay narrowly scoped to single-kind,
        // non-halo traversals. Do not widen this condition to cover more
        // `kind_filter` combinations without first confirming the caller isn't
        // going to fan out across the corpus the way `halo()` would have.
        // `[evaluate] large state fetch query_kind="leaf-anchored"` in the debug
        // log is the trip-wire for this cost — watch it on large corpus runs.
        let is_halo_shaped = trav.kind_filter == EnumSet::all();
        let frontier_set: BTreeSet<Bid> = if is_halo_shaped {
            let const_ns_brefs: std::collections::BTreeSet<Bref> =
                const_namespaces().iter().map(|bid| bid.bref()).collect();
            current
                .iter()
                .filter(|bid| !const_ns_brefs.contains(&bid.bref()))
                .copied()
                .collect()
        } else {
            current.clone()
        };
        let mut frontier: Vec<Bid> = frontier_set.into_iter().collect();
        let mut visited: BTreeSet<Bid> = current.clone();

        // Ensure the package has a graph to add edges into.
        if package.graph().is_none() {
            package.set_graph(BeliefGraph::default());
        }

        // BID → NodeIndex cache for the growing package graph.
        let mut bid_to_idx: BTreeMap<Bid, petgraph::stable_graph::NodeIndex> = {
            let g = package.graph().unwrap().relations.as_graph();
            g.node_indices().map(|idx| (g[idx], idx)).collect()
        };

        // Build the weight-kind column filter: "section IS NOT NULL AND/OR ..."
        // At least one matching kind column must be non-null.
        let kind_clauses: Vec<String> = trav
            .kind_filter
            .iter()
            .map(|kind| {
                let col = format!("{kind:?}").to_lowercase();
                format!("{col} IS NOT NULL")
            })
            .collect();
        if kind_clauses.is_empty() {
            return Ok(result);
        }
        let kind_filter_sql = if kind_clauses.len() == 1 {
            kind_clauses[0].clone()
        } else {
            format!("({})", kind_clauses.join(" OR "))
        };

        let has_source_input = trav.input_roles.contains(Role::Source);
        let has_sink_input = trav.input_roles.contains(Role::Sink);
        let has_owner_input = trav.input_roles.contains(Role::Owner);

        for _hop in 0..trav.depth.max_hops() {
            if frontier.is_empty() {
                break;
            }

            let frontier_csv = frontier
                .iter()
                .map(|b| format!("\"{b}\""))
                .collect::<Vec<_>>()
                .join(", ");

            let mut next_frontier = BTreeSet::new();
            let mut hop_edges: Vec<petgraph::stable_graph::EdgeIndex> = Vec::new();

            // Helper: upsert a relation into the package graph, return EdgeIndex.
            // Uses bid_to_idx cache; adds nodes if not present.
            // Multiple input-role queries (Source, Sink, Owner) within the
            // same hop can discover the same edge — use find_edge to dedup.
            let add_relation = |rel: &BeliefRelation,
                                graph: &mut BeliefGraph,
                                cache: &mut BTreeMap<Bid, petgraph::stable_graph::NodeIndex>|
             -> petgraph::stable_graph::EdgeIndex {
                let g = graph.relations.as_graph_mut();
                let src_idx = *cache
                    .entry(rel.source)
                    .or_insert_with(|| g.add_node(rel.source));
                let snk_idx = *cache
                    .entry(rel.sink)
                    .or_insert_with(|| g.add_node(rel.sink));
                if let Some(eidx) = g.find_edge(src_idx, snk_idx) {
                    *g.edge_weight_mut(eidx).unwrap() = rel.weights.clone();
                    eidx
                } else {
                    g.add_edge(src_idx, snk_idx, rel.weights.clone())
                }
            };

            // Helper: collect output BIDs from a relation based on output_roles.
            let collect_outputs = |rel: &BeliefRelation, frontier: &mut BTreeSet<Bid>| {
                if trav.output_roles.contains(Role::Source) {
                    frontier.insert(rel.source);
                }
                if trav.output_roles.contains(Role::Sink) {
                    frontier.insert(rel.sink);
                }
                // Owner output: resolve owned_by bref — handled below
                // since it requires async.
            };

            // Source-input: node is source → walk outgoing
            if has_source_input {
                let sql = format!(
                    "SELECT * FROM relations \
                     WHERE source IN ({frontier_csv}) AND {kind_filter_sql}"
                );
                let rows: Vec<BeliefRelation> = sqlx::query_as(&sql).fetch_all(&self.0).await?;
                for rel in &rows {
                    let graph = package.graph_mut().as_mut().unwrap();
                    let eidx = add_relation(rel, graph, &mut bid_to_idx);
                    hop_edges.push(eidx);
                    collect_outputs(rel, &mut next_frontier);
                }
            }

            // Sink-input: node is sink → walk incoming
            if has_sink_input {
                let sql = format!(
                    "SELECT * FROM relations \
                     WHERE sink IN ({frontier_csv}) AND {kind_filter_sql}"
                );
                let rows: Vec<BeliefRelation> = sqlx::query_as(&sql).fetch_all(&self.0).await?;
                for rel in &rows {
                    let graph = package.graph_mut().as_mut().unwrap();
                    let eidx = add_relation(rel, graph, &mut bid_to_idx);
                    hop_edges.push(eidx);
                    collect_outputs(rel, &mut next_frontier);
                }
            }

            // Owner-input: node is owner → find edges owned by frontier brefs
            if has_owner_input {
                let bref_csv = frontier
                    .iter()
                    .map(|b| format!("\"{}\"", b.bref()))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT * FROM relations \
                     WHERE owned_by IN ({bref_csv}) AND {kind_filter_sql}"
                );
                let rows: Vec<BeliefRelation> = sqlx::query_as(&sql).fetch_all(&self.0).await?;
                for rel in &rows {
                    let graph = package.graph_mut().as_mut().unwrap();
                    let eidx = add_relation(rel, graph, &mut bid_to_idx);
                    hop_edges.push(eidx);
                    collect_outputs(rel, &mut next_frontier);
                }
            }

            // Cycle prevention
            next_frontier.retain(|bid| !visited.contains(bid));

            // Record this hop in the tape.
            if !hop_edges.is_empty() {
                let hop_output_bids: Vec<Bid> = next_frontier.iter().copied().collect();
                package.tape_mut().steps.push(TapeEntry {
                    label: label.to_string(),
                    content: TapeContent::Edges {
                        edges: hop_edges,
                        output_bids: hop_output_bids,
                    },
                    payload: None,
                });
            }

            if next_frontier.is_empty() {
                break;
            }

            result.extend(next_frontier.iter());
            visited.extend(next_frontier.iter());
            frontier = next_frontier.iter().copied().collect();
        }

        // Ensure at least one tape entry per step for stage detection.
        if package.tape().len() == tape_start {
            tracing::trace!(
                label = label,
                input_count = current.len(),
                "SQL traversal produced no results — pushing empty tape entry"
            );
            package.tape_mut().steps.push(TapeEntry {
                label: label.to_string(),
                content: TapeContent::Edges {
                    edges: vec![],
                    output_bids: vec![],
                },
                payload: None,
            });
        }

        Ok(result)
    }

    /// Fetch states for a set of BIDs and apply a [`NodeFilter`] in memory.
    async fn apply_filter_sql(
        &self,
        current: &BTreeSet<Bid>,
        filter: &crate::query::spec::NodeFilter,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        if current.is_empty() {
            return Ok(BTreeSet::new());
        }
        // Fetch states for current BIDs
        let bid_vec: Vec<Bid> = current.iter().copied().collect();
        let states = self.get_states_by_bids(&bid_vec).await?;
        // Build a temporary BeliefBase for in-memory filter evaluation
        let bb = BeliefBase::from(BeliefGraph {
            states,
            relations: BidGraph::default(),
        });
        bb.apply_filter(current, filter, None).map(|(bids, _)| bids)
    }

    /// Evaluate a sub-pipeline of projection steps via SQL.
    /// Used by composition branches.
    async fn apply_steps_sql(
        &self,
        steps: &[crate::query::spec::ProjectionStep],
        seed: BTreeSet<Bid>,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        let mut current = seed.clone();
        // Scratch package for sub-pipeline traversal recording (discarded).
        let scratch_spec = crate::query::spec::QuerySpec::seed_then(
            TapeFn::Bids(seed.iter().copied().collect()),
            steps.to_vec(),
        );
        let mut scratch_pkg = QueryPackage::new(scratch_spec);
        for (step_idx, step) in steps.iter().enumerate() {
            let label = if step.label.is_empty() {
                step_idx.to_string()
            } else {
                step.label.clone()
            };
            let prev_label: Option<String> = if step_idx > 0 {
                let prev = &steps[step_idx - 1];
                Some(if prev.label.is_empty() {
                    (step_idx - 1).to_string()
                } else {
                    prev.label.clone()
                })
            } else {
                None
            };
            let input =
                scratch_pkg
                    .tape()
                    .eval_input(&step.input, &seed, &current, prev_label.as_deref());
            current = match &step.operation {
                crate::query::spec::StepOperation::Identity => input,
                crate::query::spec::StepOperation::Traverse(trav) => {
                    self.apply_traversal_sql(&input, trav, &label, &mut scratch_pkg)
                        .await?
                }
                crate::query::spec::StepOperation::Filter(filter) => {
                    self.apply_filter_sql(&input, filter).await?
                }
                crate::query::spec::StepOperation::Compose(comp) => {
                    let left = Box::pin(self.apply_steps_sql(&comp.left, seed.clone())).await?;
                    let right = Box::pin(self.apply_steps_sql(&comp.right, seed.clone())).await?;
                    match comp.op {
                        crate::query::spec::CompositionOp::And => {
                            left.intersection(&right).copied().collect()
                        }
                        crate::query::spec::CompositionOp::Or => {
                            left.union(&right).copied().collect()
                        }
                        crate::query::spec::CompositionOp::Not => {
                            left.difference(&right).copied().collect()
                        }
                    }
                }
            };
        }
        Ok(current)
    }

    #[tracing::instrument(skip(self))]
    async fn get_states_by_bids(
        &self,
        bids: &[Bid],
    ) -> Result<FxHashMap<Bid, BeliefNode>, BuildonomyError> {
        if bids.len() > 500 {
            tracing::debug!(
                target: "noet_core::db::query_size",
                bid_count = bids.len(),
                "[get_states_by_bids] large BID set",
            );
        }
        // Chunk into batches to stay within SQLite's bind-variable limit.
        let mut results = FxHashMap::default();
        for chunk in bids.chunks(CHUNK_SIZE) {
            let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM beliefs WHERE ");
            push_id_expr(&mut qb, chunk, "bid", true);
            qb.push("GROUP BY bid");
            let state_query = qb.build_query_as::<BeliefNode>();
            let state_sql = state_query.sql();

            let chunk_results = state_query
                .fetch_all(&self.0)
                .await
                .map_err(|e| {
                    tracing::error!(
                        "[DbConnection.get_states_by_bids] SQL error processing \
                        state_query '{}'\n\terror: {}",
                        state_sql,
                        e
                    );
                    e
                })?
                .into_iter()
                .map(|s| (s.bid, s));
            results.extend(chunk_results);
        }

        Ok(results)
    }

    pub async fn is_db_balanced(&self) -> Result<(), BuildonomyError> {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM beliefs;");
        let state_query = qb.build_query_as::<BeliefNode>();
        let states = state_query
            .fetch_all(&self.0)
            .await
            .map_err(|e| {
                tracing::error!(
                    "[DbConnection.export_beliefgraph] SQL error processing \
                    get all states query\n\terror: {}",
                    e
                );
                e
            })?
            .into_iter()
            .map(|s| (s.bid, s))
            .collect::<FxHashMap<Bid, BeliefNode>>();

        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM relations;");
        let rel_query = qb.build_query_as::<BeliefRelation>();
        let relation_vec: Vec<BeliefRelation> =
            rel_query.fetch_all(&self.0).await.map_err(|e| {
                tracing::error!(
                    "[DbConnection.export_beliefgraph] SQL error processing \
                    get all relations query\n\terror: {}",
                    e
                );
                e
            })?;
        let relations = BidGraph::from_edges(relation_vec);
        tracing::debug!(
            "DB has {} states and {} edges",
            states.len(),
            relations.0.edge_count()
        );
        let bs = BeliefBase::new_unbalanced(states, relations, false);

        bs.is_balanced()
    }

    pub async fn get_file_mtimes(&self) -> Result<BTreeMap<PathBuf, i64>, BuildonomyError> {
        let rows = sqlx::query_as::<_, (String, i64)>("SELECT path, mtime FROM file_mtimes")
            .fetch_all(&self.0)
            .await?;

        Ok(rows
            .into_iter()
            .map(|(path, mtime)| (string_to_os_path(&path), mtime))
            .collect())
    }
}

fn submap(
    pool: Pool<Sqlite>,
    network_bid: Bid,
    path: String,
    depth: u8,
    processed_nets: BTreeSet<Bid>,
) -> NestedNetFuture {
    Box::pin(async move {
        if processed_nets.contains(&network_bid) {
            return Ok(vec![]);
        }

        // If a non-empty path is given, resolve it to a subnet bid and recurse into it.
        if !path.is_empty() {
            // Walk path segments one at a time, same as resolve_net_path.
            let mut current_net = network_bid;
            let segments: Vec<&str> = path.splitn(2, '/').collect();
            let first = segments[0];
            let rest = if segments.len() > 1 { segments[1] } else { "" };

            let row = sqlx::query_as::<_, (String,)>(
                "SELECT target FROM paths WHERE net = ? AND path = ? LIMIT 1",
            )
            .bind(current_net.bref().to_string())
            .bind(first)
            .fetch_optional(&pool)
            .await?;

            let Some((bid_str,)) = row else {
                return Ok(vec![]);
            };
            let Ok(sub_bid) = Bid::try_from(bid_str.as_str()) else {
                return Ok(vec![]);
            };
            current_net = sub_bid;

            return submap(pool, current_net, rest.to_owned(), depth, processed_nets).await;
        }

        let rows = sqlx::query_as::<_, (String, String, String, i64)>(
            "SELECT path, target, ordering, is_net FROM paths WHERE net = ?",
        )
        .bind(network_bid.bref().to_string())
        .fetch_all(&pool)
        .await?;
        let mut row_results = rows
            .into_iter()
            .filter_map(|(path, target, ordering, is_net)| {
                if path.is_empty() {
                    return None;
                }
                let order: Vec<u16> = match parse_order(&ordering) {
                    Some(o) => o,
                    None => {
                        tracing::warn!("[submap] Failed to parse ordering {:?}", ordering);
                        return None;
                    }
                };
                Bid::try_from(target.as_str())
                    .ok()
                    .map(|bid| (path, bid, order, is_net != 0))
            })
            .collect::<Vec<_>>();

        if depth == 0 {
            return Ok(row_results);
        }

        let mut row_nets = row_results
            .iter()
            .enumerate()
            .filter_map(|(idx, elem)| {
                if elem.3 && !processed_nets.contains(&elem.1) {
                    Some((idx, elem.1))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        tracing::debug!(
            "[submap] Found {} subnets in network {}",
            row_nets.len(),
            network_bid
        );
        row_nets.sort_by_key(|a| a.0);
        let new_nets = BTreeSet::from_iter(row_nets.iter().map(|elem| elem.1));
        let mut newly_processed = processed_nets.clone();
        newly_processed.insert(network_bid);
        newly_processed.append(&mut new_nets.clone());
        for new_net in new_nets.iter() {
            // remove new_net from processed just for this call
            let mut newly_processed_for_call = newly_processed.clone();
            newly_processed_for_call.remove(new_net);
            let mut sub_results = submap(
                pool.clone(),
                *new_net,
                String::new(),
                depth,
                newly_processed_for_call,
            )
            .await?;

            if !sub_results.is_empty() {
                let Some(row_nets_index) = row_nets.iter().position(|elem| elem.1 == *new_net)
                else {
                    tracing::warn!(
                        "[submap] Subnet {} expected in row_nets but not found (len={})",
                        new_net,
                        row_nets.len()
                    );
                    continue;
                };
                let (start_idx, _net) = row_nets[row_nets_index];
                {
                    let base_ap = AnchorPath::from(&row_results[start_idx].0);
                    let base_order = row_results[start_idx].2.clone();
                    for (sub_path, _bid, sub_order, _is_net) in sub_results.iter_mut() {
                        *sub_path = base_ap.join(sub_path.as_str()).into_string();
                        let mut new_order = base_order.clone();
                        new_order.append(sub_order);
                        *sub_order = new_order;
                    }
                }
                let incr = sub_results.len() - 1; // since not empty, this is always >= 0
                row_results.splice(
                    start_idx..start_idx + 1,
                    sub_results
                        .into_iter()
                        .map(|(p, b, o, _is_net)| (p, b, o, false)),
                );
                // Increment indices to account for our splice
                for net in row_nets.iter_mut().skip(row_nets_index + 1) {
                    net.0 += incr;
                }
            }
        }
        Ok(row_results)
    })
}

impl BeliefSource for DbConnection {
    /// Get cached file modification times for cache invalidation.
    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> {
        Box::pin(async move { self.get_file_mtimes().await })
    }

    /// SQL-native QueryPackage evaluation.
    ///
    /// Drives the full query lifecycle using SQL for seed resolution
    /// and per-hop traversal, then bulk-fetches all accumulated BIDs
    /// to produce the final output.
    ///
    /// **Pipeline:**
    /// 1. Resolve seed `TapeFn` → `Vec<Bid>` via SQL
    /// 2. Walk each projection step:
    ///    - `Traverse` → per-hop SQL against `relations` table
    ///    - `Filter` → SQL state fetch + in-memory predicate
    ///    - `Compose` → recursive sub-evaluation of branches
    /// 3. Bulk-fetch all BIDs from tape, materialize `BeliefBase`,
    ///    delegate to `materialize_graph` for Trace coloring and
    ///    graph construction.
    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async move {
            // 1. Resolve seed TapeFn → BIDs, recording anchor map for Keys seeds.
            //    resolve_seed returns (key_index, bid) pairs so we can build
            //    the anchor map without re-resolving.
            let mut anchor_map: Option<Vec<(usize, Bid)>> = None;
            if package.stage() == PackageStage::Constructed {
                let first_input = package
                    .spec()
                    .steps
                    .first()
                    .map(|s| s.input.clone())
                    .unwrap_or(TapeFn::Bids(vec![]));
                if first_input.is_seed() {
                    let indexed_bids = self.resolve_seed(&first_input).await?;
                    if matches!(first_input, TapeFn::Keys(_)) && !indexed_bids.is_empty() {
                        anchor_map = Some(indexed_bids.clone());
                    }
                    let seed_bids: Vec<Bid> =
                        indexed_bids.into_iter().map(|(_, bid)| bid).collect();
                    if let Some(first_step) = package.spec_mut().steps.first_mut() {
                        first_step.input = TapeFn::Bids(seed_bids);
                    }
                } else {
                    // No top-level seed — Compose branches provide their own.
                    if let Some(first_step) = package.spec_mut().steps.first_mut() {
                        first_step.input = TapeFn::Bids(vec![]);
                    }
                }
            }

            // 2. Run projection steps via SQL
            if package.stage() < PackageStage::Projected {
                let steps = package.spec().steps.clone();
                let seed: BTreeSet<Bid> = match package.spec().steps.first().map(|s| &s.input) {
                    Some(TapeFn::Bids(bids)) => bids.iter().copied().collect(),
                    _ => {
                        return Err(BuildonomyError::Command(
                            "evaluate called with unresolved seed".to_string(),
                        ));
                    }
                };

                // Store anchor map for the first step's payload.
                if let Some(map) = anchor_map {
                    package.set_anchor_map(map);
                }

                // Count completed projection steps.
                let completed = package.tape().steps.len();
                let mut current: BTreeSet<Bid> = if let Some(last) = package.tape().steps.last() {
                    last.content.output_bids().into_iter().collect()
                } else {
                    seed.clone()
                };

                for (step_idx, step) in steps.iter().enumerate().skip(completed) {
                    let label = if step.label.is_empty() {
                        step_idx.to_string()
                    } else {
                        step.label.clone()
                    };

                    let prev_label: Option<String> = if step_idx > 0 {
                        let prev = &steps[step_idx - 1];
                        Some(if prev.label.is_empty() {
                            (step_idx - 1).to_string()
                        } else {
                            prev.label.clone()
                        })
                    } else {
                        None
                    };
                    let input = package.tape().eval_input(
                        &step.input,
                        &seed,
                        &current,
                        prev_label.as_deref(),
                    );

                    // If this is the first step, consume the pending anchor map.
                    let step_payload = if step_idx == 0 {
                        package.take_anchor_map().map(TapePayload::AnchorMap)
                    } else {
                        None
                    };

                    match &step.operation {
                        StepOperation::Identity => {
                            current = input;
                            package.tape_mut().steps.push(TapeEntry {
                                label,
                                content: TapeContent::Nodes(current.iter().copied().collect()),
                                payload: step_payload,
                            });
                        }
                        StepOperation::Traverse(trav) => {
                            current = self
                                .apply_traversal_sql(&input, trav, &label, package)
                                .await?;
                        }
                        StepOperation::Filter(filter) => {
                            current = self.apply_filter_sql(&input, filter).await?;
                            package.tape_mut().steps.push(TapeEntry {
                                label: label.clone(),
                                content: TapeContent::Nodes(current.iter().copied().collect()),
                                payload: step_payload,
                            });
                        }
                        StepOperation::Compose(comp) => {
                            // Evaluate each branch as a sub-pipeline
                            let left = self.apply_steps_sql(&comp.left, seed.clone()).await?;
                            let right = self.apply_steps_sql(&comp.right, seed.clone()).await?;

                            let left_start = package.tape().len();
                            package.tape_mut().steps.push(TapeEntry {
                                label: format!("{label}.L"),
                                content: TapeContent::Nodes(left.iter().copied().collect()),
                                payload: None,
                            });
                            let right_start = package.tape().len();
                            package.tape_mut().steps.push(TapeEntry {
                                label: format!("{label}.R"),
                                content: TapeContent::Nodes(right.iter().copied().collect()),
                                payload: None,
                            });
                            let right_end = package.tape().len();

                            let intersection: Vec<Bid> =
                                left.intersection(&right).copied().collect();
                            current = match comp.op {
                                CompositionOp::And => intersection.iter().copied().collect(),
                                CompositionOp::Or => left.union(&right).copied().collect(),
                                CompositionOp::Not => left.difference(&right).copied().collect(),
                            };
                            package.tape_mut().steps.push(TapeEntry {
                                label,
                                content: TapeContent::Compose {
                                    op: comp.op,
                                    left: left_start..right_start,
                                    right: right_start..right_end,
                                    result: current.iter().copied().collect(),
                                    intersection,
                                },
                                payload: None,
                            });
                        }
                    };
                }
            }

            // 3. Fetch states for all accumulated BIDs and apply Trace coloring.
            // The package graph already has edges from apply_traversal_sql;
            // this step populates states and applies primary/Trace distinction.
            if package.stage() == PackageStage::Projected {
                let all_bids = {
                    let seed: BTreeSet<Bid> = match package.spec().steps.first().map(|s| &s.input) {
                        Some(TapeFn::Bids(bids)) => bids.iter().copied().collect(),
                        _ => BTreeSet::new(),
                    };
                    let mut all = seed;
                    all.extend(package.tape().cumulative_bids());
                    all
                };

                if all_bids.is_empty() {
                    if package.graph().is_none() {
                        package.set_graph(BeliefGraph::default());
                    }
                    return Ok(());
                }

                // Fetch states for all BIDs.
                let all_bid_vec: Vec<Bid> = all_bids.iter().copied().collect();
                if all_bid_vec.len() > 200 {
                    let has_halo = package
                        .spec()
                        .steps
                        .iter()
                        .any(|s| s.label == crate::query::spec::GRAPH_CONTEXT_HALO_LABEL);
                    let has_balance = package
                        .spec()
                        .steps
                        .iter()
                        .any(|s| s.label == crate::query::spec::GRAPH_CONTEXT_BALANCE_LABEL);
                    let has_leaf = package
                        .spec()
                        .steps
                        .iter()
                        .any(|s| s.label == crate::query::spec::GRAPH_CONTEXT_LEAF_LABEL);
                    let query_kind = match (has_halo, has_balance, has_leaf) {
                        (true, true, _) => "balanced",
                        (false, true, false) => "anchored",
                        (false, false, true) => "leaf-anchored",
                        (true, false, false) => "halo-only",
                        (false, false, false) => "seed-only",
                        // balance + leaf together shouldn't occur from any current
                        // constructor, but classify rather than silently drop info.
                        (true, _, true) | (false, true, true) => "mixed-context",
                    };
                    tracing::debug!(
                        target: "noet_core::db::query_size",
                        total_bids = all_bid_vec.len(),
                        tape_steps = package.tape().steps.len(),
                        spec_steps = package.spec().steps.len(),
                        query_kind,
                        spec = ?package.original_spec(),
                        "[evaluate] large state fetch",
                    );
                }
                let states = self.get_states_by_bids(&all_bid_vec).await?;

                // Merge states into the existing package graph (which already
                // has edges from traversal). If no graph exists yet (e.g.,
                // filter-only query), also fetch relations.
                if package.graph().is_some() {
                    // Graph exists with edges from traversal — just add states.
                    let graph = package.graph_mut().as_mut().unwrap();
                    graph.states = states;
                } else {
                    // No graph yet (filter/compose-only) — fetch relations too.
                    let state_set = states
                        .keys()
                        .map(|bid| format!("\"{}\"", bid))
                        .collect::<Vec<String>>()
                        .join(", ");
                    let relations = if !state_set.is_empty() {
                        let mut qb = QueryBuilder::new(&format!(
                            "SELECT * FROM relations WHERE \
                         sink IN ({state_set}) AND source IN ({state_set});"
                        ));
                        let rows: Vec<BeliefRelation> = qb
                            .build_query_as::<BeliefRelation>()
                            .fetch_all(&self.0)
                            .await?;
                        BidGraph::from_edges(rows)
                    } else {
                        BidGraph::default()
                    };
                    package.set_graph(BeliefGraph { states, relations });
                }

                // Apply Trace coloring in-place.
                let bb = BeliefBase::from(package.graph().unwrap().clone());
                bb.materialize_graph(package)?;
            }

            Ok(())
        })
    }

    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            let results = submap(
                self.0.clone(),
                network_bid,
                path.to_owned(),
                depth,
                BTreeSet::default(),
            )
            .await?;
            Ok(results
                .into_iter()
                .filter(|(p, _bid, order, _is_net)| {
                    !p.is_empty() && (include_index || !order.contains(&u16::MAX))
                })
                .map(|(p, bid, order, _is_net)| (p, bid, order))
                .collect())
        })
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            // If no entry BID given, delegate to the full-network submap.
            let Some(entry_bid) = entry else {
                return self.submap(network_bid, "", depth, include_index).await;
            };

            // Look up the path and ordering of entry_bid within the given network.
            // The paths table stores (net=bref, path=..., target=bid_str, ordering=...).
            let bid_str = entry_bid.to_string();

            // Query: find (net, path, ordering) for this target BID.
            // We search by target BID — each BID has exactly one canonical path entry.
            let row = sqlx::query_as::<_, (String, String, String)>(
                "SELECT net, path, ordering FROM paths WHERE target = ? LIMIT 1",
            )
            .bind(&bid_str)
            .fetch_optional(&self.0)
            .await
            .map_err(|e| {
                BuildonomyError::Cache(format!("submap_by_bid path lookup failed: {e}"))
            })?;

            let (found_net_str, _found_path, found_ordering_str) = match row {
                Some(r) => r,
                None => {
                    tracing::debug!("[submap_by_bid] no path entry found for BID {entry_bid}");
                    return Ok(vec![]);
                }
            };

            // Parse the entry's ordering into a Vec<u16> for prefix-matching.
            let entry_order: Vec<u16> = match parse_order(&found_ordering_str) {
                Some(o) => o,
                None => {
                    tracing::warn!(
                        "[submap_by_bid] Failed to parse ordering {:?}",
                        found_ordering_str
                    );
                    return Ok(vec![]);
                }
            };

            // Resolve the found net bref to a Bid.
            // The net column stores bref strings (5 hex chars), not full BIDs,
            // so Bid::try_from on a bref string will fail — fall back to beliefs table.
            let found_net_bid = match Bid::try_from(found_net_str.as_str()) {
                Ok(b) => b,
                Err(_) => {
                    let net_row = sqlx::query_as::<_, (String,)>(
                        "SELECT bid FROM beliefs WHERE bref = ? LIMIT 1",
                    )
                    .bind(&found_net_str)
                    .fetch_optional(&self.0)
                    .await
                    .map_err(|e| {
                        BuildonomyError::Cache(format!("submap_by_bid net BID lookup failed: {e}"))
                    })?;
                    match net_row {
                        Some((net_bid_str,)) => {
                            Bid::try_from(net_bid_str.as_str()).map_err(|_| {
                                BuildonomyError::Cache(format!(
                                    "unparseable net BID: {net_bid_str}"
                                ))
                            })?
                        }
                        None => {
                            tracing::debug!(
                            "[submap_by_bid] network bref {found_net_str} not found in beliefs table"
                        );
                            return Ok(vec![]);
                        }
                    }
                }
            };

            // Fetch the full network's paths (empty path = no segment walking),
            // then filter to the entry's subtree using its ordering prefix.
            //
            // This mirrors PathMap::submap's in-memory logic (pathmap.rs L2184–2193):
            // find the entry's order_prefix, then retain rows whose ordering starts
            // with that prefix. The previous approach fed found_path through the
            // segment-walking submap(), which treated leaf document filenames as
            // subnet hops and failed for non-network documents.
            let all_results = submap(
                self.0.clone(),
                found_net_bid,
                String::new(),
                depth,
                BTreeSet::default(),
            )
            .await?;

            Ok(all_results
                .into_iter()
                .filter(|(p, _bid, order, _is_net)| {
                    if p.is_empty() {
                        return false;
                    }
                    // Order-prefix filter: keep rows whose ordering starts with
                    // the entry's own ordering. This scopes to the entry's subtree.
                    if order.len() < entry_order.len() || order[..entry_order.len()] != entry_order
                    {
                        return false;
                    }
                    include_index || !order.contains(&u16::MAX)
                })
                .map(|(p, bid, order, _is_net)| (p, bid, order))
                .collect())
        })
    }

    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async move {
            // Get all states from database
            let state_query = sqlx::query_as::<_, BeliefNode>("SELECT * FROM beliefs");
            let states: FxHashMap<Bid, BeliefNode> = state_query
                .fetch_all(&self.0)
                .await
                .map_err(|e| {
                    tracing::error!(
                        "[DbConnection.export_beliefgraph] Failed to fetch beliefs: {}",
                        e
                    );
                    e
                })?
                .into_iter()
                .map(|node| (node.bid, node))
                .collect();

            // Get all relations from database
            let relation_query = sqlx::query_as::<_, BeliefRelation>("SELECT * FROM relations");
            let relation_vec: Vec<BeliefRelation> =
                relation_query.fetch_all(&self.0).await.map_err(|e| {
                    tracing::error!(
                        "[DbConnection.export_beliefgraph] Failed to fetch relations: {}",
                        e
                    );
                    e
                })?;

            let relations = BidGraph::from_edges(relation_vec);

            tracing::debug!(
                "Exported BeliefGraph from database: {} states, {} relations",
                states.len(),
                relations.0.edge_count()
            );

            Ok(BeliefGraph { states, relations })
        })
    }
}

/// A migration definition.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i64,
    pub description: &'static str,
    pub sql: &'static str,
    pub kind: MigrationType,
}

#[derive(Debug, Clone)]
struct MigrationList(Vec<Migration>);

impl MigrationSource<'static> for MigrationList {
    fn resolve(self) -> BoxFuture<'static, Result<Vec<SqlxMigration>, BoxDynError>> {
        Box::pin(async move {
            let mut migrations = Vec::new();
            for migration in self.0 {
                if matches!(migration.kind, MigrationType::ReversibleUp) {
                    migrations.push(SqlxMigration::new(
                        migration.version,
                        migration.description.into(),
                        migration.kind,
                        migration.sql.into(),
                        false,
                    ));
                }
            }
            Ok(migrations)
        })
    }
}

/// Shared migration list used by both [`db_init`] and [`db_init_memory`].
fn belief_migrations() -> MigrationList {
    MigrationList(vec![Migration {
        version: 1,
        description: "create_initial_tables",
        sql: "\
            CREATE TABLE beliefs (bid TEXT PRIMARY KEY, bref TEXT, kind INTEGER, title TEXT, schema TEXT, payload TEXT, id TEXT, metadata TEXT, title_slug TEXT); \
            CREATE TABLE relations (sink TEXT, source TEXT, epistemic TEXT, section TEXT, pragmatic TEXT, owned_by TEXT, UNIQUE(sink, source)); \
            CREATE TABLE paths (net TEXT, path TEXT, target TEXT, ordering TEXT, is_net INTEGER NOT NULL DEFAULT 0, UNIQUE(net, path)); \
            CREATE TABLE file_mtimes (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL); \
            CREATE INDEX beliefs_id ON beliefs(id); \
            CREATE INDEX beliefs_title_slug ON beliefs(title_slug); \
            CREATE INDEX beliefs_bref ON beliefs(bref); \
            CREATE INDEX paths_target ON paths(target); \
            CREATE INDEX paths_is_net ON paths(net, is_net); \
            CREATE INDEX relations_owned_by ON relations(owned_by);",
        kind: MigrationType::ReversibleUp,
    }])
}

pub async fn db_init(db_path: PathBuf) -> Result<Pool<Sqlite>, sqlx::Error> {
    let fqdb = format!("sqlite:{}", db_path.to_str().unwrap());
    tracing::debug!("Initializing cache db from file: {:?}", fqdb);
    if !Sqlite::database_exists(&fqdb).await.unwrap_or(false) {
        Sqlite::create_database(&fqdb).await?;
    }
    let options = SqliteConnectOptions::from_str(&fqdb)?
        .read_only(false)
        .disable_statement_logging()
        .create_if_missing(true);

    // Use PoolOptions with after_connect to register regexp on each connection
    // and set a busy timeout so lock contention (e.g. a concurrent reader vs.
    // an in-flight writer transaction — see noet-core Issue 100) blocks with a
    // bounded wait instead of surfacing immediately as SQLITE_BUSY/SQLITE_LOCKED.
    let pool = PoolOptions::<Sqlite>::new()
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // Register the regexp function for this connection
                sqlx::query("SELECT sqlite_compileoption_used('ENABLE_DBSTAT_VTAB')")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA busy_timeout = 5000;")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?;

    let migrator = Migrator::new(belief_migrations()).await?;
    migrator.run(&pool).await?;

    let count_res = sqlx::query("SELECT COUNT(*) as bcount FROM beliefs;")
        .fetch_one(&pool)
        .await?;
    let rel_res = sqlx::query("SELECT COUNT(*) as rcount FROM relations;")
        .fetch_one(&pool)
        .await?;
    tracing::info!(
        "DB Connection initialized.\n \
         \tCached node count:\t{:?} \n \
         \tCached edge count:\t{:?}",
        count_res.get::<u32, usize>(0),
        rel_res.get::<u32, usize>(0)
    );

    Ok(pool)
}

/// Initialise an **ephemeral in-memory** SQLite pool with the same schema as
/// [`db_init`].
///
/// Use this for the `parse` command where the DB is a throw-away accumulator:
/// `DbConnection::apply_batch` commits a single WAL-style transaction per epoch,
/// which is cheaper than the full `BeliefBase::process_event` + PathMapMap
/// reconstruction path, but the data does not need to survive the process.
///
/// Each call creates a fresh, isolated database.  The pool is dropped (and the
/// in-memory DB freed) when the last `Pool<Sqlite>` clone goes out of scope.
pub async fn db_init_memory() -> Result<Pool<Sqlite>, sqlx::Error> {
    // Use a named shared-cache in-memory database: `file:noet_parse?mode=memory&cache=shared`.
    //
    // The plain `sqlite::memory:` URI creates a *separate* in-memory database for
    // every new connection.  Under the previous `max_connections(1)` guard this was
    // safe as long as sqlx never recycled or timed-out the single connection — but
    // under heavy parallel load (~200 concurrent tasks all issuing queries in the
    // same remainder-epoch batch) the pool's idle-connection logic can drop
    // and recreate the connection, producing a fresh empty DB that has never had
    // migrations run ("no such table: beliefs").
    //
    // The named shared-cache URI fixes this: all connections to
    // `file:noet_parse?mode=memory&cache=shared` within the same process see the
    // same in-memory schema, so the pool is free to open, close, and recycle
    // connections without losing the migrated schema.  We retain min_connections(1)
    // to keep the DB alive for the pool's lifetime (SQLite drops a named in-memory
    // DB when the last connection to it closes).
    let options = SqliteConnectOptions::from_str("file:noet_parse?mode=memory&cache=shared")?
        .disable_statement_logging();

    let pool = PoolOptions::<Sqlite>::new()
        // Keep at least one connection alive so the named in-memory database is
        // never dropped.  max_connections is unconstrained: all connections share
        // the same schema via the shared-cache URI, so concurrent access is safe.
        .min_connections(1)
        // Set a busy timeout on every pooled connection so shared-cache table-lock
        // contention (e.g. a concurrent reader vs. an in-flight writer transaction
        // — see noet-core Issue 100) blocks with a bounded wait instead of surfacing
        // immediately as SQLITE_LOCKED_SHAREDCACHE/SQLITE_BUSY.
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("PRAGMA busy_timeout = 5000;")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        // Disable idle_timeout (default: 10 min) and max_lifetime (default: 30 min).
        //
        // Both defaults are shorter than a typical large-corpus Phase 2 push_relation
        // loop (~35 min of CPU-bound Rust with zero DB activity).  When the pool's
        // background reaper fires it closes the idle connection; if that was the last
        // connection to the named shared-cache in-memory DB, SQLite silently drops the
        // entire database — migrations included.  The next acquire then opens a fresh,
        // empty connection to the same URI name ("no such table: beliefs").
        //
        // Setting both to None disables the reaper entirely for this pool.  The pool
        // is ephemeral (parse-command lifetime only) so there is no leak concern.
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await?;

    let migrator = Migrator::new(belief_migrations()).await?;
    migrator.run(&pool).await?;

    tracing::debug!("In-memory DB initialised (ephemeral, parse-command path)");
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventOrigin;

    /// Regression test for the `BEGIN`/`COMMIT` wrapping added to
    /// `Transaction::execute` (noet-core Issue 100). Before that change,
    /// atomicity of a whole `apply_batch` call was provided only by the
    /// caller holding a Rust-level lock for the call's entire duration — a
    /// mid-batch SQL failure would leave whatever ran before it permanently
    /// committed. This test forces a valid statement to run first (via
    /// `overflow`, which `execute` drains before the final `qb`), followed by
    /// a deliberately invalid statement, and asserts the valid statement's
    /// effect is rolled back along with the failure.
    #[tokio::test]
    async fn execute_rolls_back_valid_statements_on_later_failure() {
        let pool = db_init_memory().await.unwrap();

        let bid = Bid::from(uuid::Uuid::from_u128(1));
        let node = BeliefNode {
            bid,
            ..Default::default()
        };

        let mut tx = Transaction::new();
        tx.add_event(&BeliefEvent::NodeUpdate(
            vec![NodeKey::Bid { bid }],
            node,
            EventOrigin::Remote,
        ))
        .unwrap();

        // `execute` drains `overflow` (in order) before the final `qb`. Move
        // the valid NodeUpdate statement into `overflow` so it runs first,
        // then install a deliberately invalid statement as the final `qb` so
        // the failure occurs only after the valid statement has already run
        // inside the same (as-yet-uncommitted) transaction.
        let valid_qb = std::mem::replace(
            &mut tx.qb,
            QueryBuilder::<Sqlite>::new(
                "INSERT INTO nonexistent_table_for_atomicity_test (x) VALUES (1);",
            ),
        );
        tx.overflow.push(valid_qb);

        let result = tx.execute(&pool).await;
        assert!(
            result.is_err(),
            "execute should fail due to the deliberately invalid statement"
        );

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM beliefs WHERE bid = ?")
            .bind(bid.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "the valid NodeUpdate statement should have been rolled back \
             along with the later failure (BEGIN/COMMIT must wrap the whole batch)"
        );
    }
}
