use crate::{
    beliefbase::{BeliefBase, BeliefGraph, BidGraph},
    error::BuildonomyError,
    event::BeliefEvent,
    paths::{
        path::{os_path_to_string, string_to_os_path},
        AnchorPath,
    },
    properties::{BeliefKind, BeliefNode, BeliefRelation, Bid, Bref, WeightKind, WeightSet},
    query::{push_string_expr, AsSql, BeliefSource, Expression, StatePred},
};
use futures_core::future::BoxFuture;
use sqlx::Execute;
use sqlx::{
    error::BoxDynError,
    migrate::{MigrateDatabase, Migration as SqlxMigration, MigrationSource, Migrator},
    sqlite::{Sqlite, SqliteConnectOptions},
    ConnectOptions, Row,
};
use sqlx::{migrate::MigrationType, Pool, QueryBuilder};
use std::{collections::BTreeMap, fmt::Debug, result::Result};
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
    mtime_updates: BTreeMap<String, i64>,
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
            mtime_updates: BTreeMap::new(),
        }
    }

    pub async fn execute(&mut self, connection: &Pool<Sqlite>) -> Result<(), BuildonomyError> {
        let query = self.qb.build();
        // tracing::debug!("Executing SQL for {} events", self.staged);
        // tracing::debug!("SQL:\n{}", query.sql());
        query.execute(connection).await?;
        self.qb.reset();

        // Batch insert mtime updates
        if !self.mtime_updates.is_empty() {
            let mut mtime_qb =
                QueryBuilder::<Sqlite>::new("INSERT OR REPLACE INTO file_mtimes (path, mtime) ");
            mtime_qb.push_values(self.mtime_updates.iter(), |mut b, (path, mtime)| {
                b.push_bind(path.clone()).push_bind(*mtime);
            });
            mtime_qb.build().execute(connection).await?;
            self.mtime_updates.clear();
        }

        Ok(())
    }

    pub fn track_file_mtime(&mut self, path: &Path) -> Result<(), BuildonomyError> {
        tracing::debug!("[Transaction] track_file_mtime called for path: {:?}", path);
        tracing::debug!(
            "[Transaction] path exists: {}, path is_absolute: {}",
            path.exists(),
            path.is_absolute()
        );

        match fs::metadata(path) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified) => match modified.duration_since(SystemTime::UNIX_EPOCH) {
                    Ok(duration) => {
                        let mtime = duration.as_secs() as i64;
                        let path_str = os_path_to_string(path);
                        self.mtime_updates.insert(path_str.clone(), mtime);
                        tracing::info!(
                            "[Transaction]   ✓ Successfully tracked mtime {} for {:?}",
                            mtime,
                            path
                        );
                        tracing::info!(
                            "[Transaction]   mtime_updates.len() = {}",
                            self.mtime_updates.len()
                        );
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
                    "[Transaction]   ✗ Failed to get metadata for {:?}: {} (path may not exist or be inaccessible)",
                    path,
                    e
                );
                tracing::warn!("[Transaction]   errno/kind: {:?}", e.kind());
            }
        }
        Ok(())
    }

    // Bool return value lets the caller know whether rewrite paths should be called.
    pub fn add_event(&mut self, event: &BeliefEvent) -> Result<(), BuildonomyError> {
        // TODO: rewrite paths
        //
        // Also:
        // - is it because this isn't in place that UI get query wasn't returning any nodes? or
        //   is it a different reason?
        // - why didn't I see subpaths in the raw DB for net-> doc links
        match event {
            BeliefEvent::NodeUpdate(_bid, yaml_str, _) => {
                let node = BeliefNode::try_from(&yaml_str[..])?;
                self.update_node(&node);
            }
            BeliefEvent::NodesRemoved(bids, _) => {
                self.remove_nodes(bids);
            }
            BeliefEvent::PathsRemoved(net, paths, _) => {
                self.remove_paths(net, paths);
            }
            BeliefEvent::PathAdded(net, path, bid, order, _) => {
                self.add_paths(net, vec![(path, *bid, order)]);
            }
            BeliefEvent::PathUpdate(net, path, bid, order, _) => {
                // FIXME: we use INSERT OR REPLACE, which makes pathAdded and PathUpdate the same,
                // but really we would like to have pathUpdate update on either the path or the
                // order as the indexing element, and have PathAdded be an INSERT and pathUpdate a
                // REPLACE.
                self.add_paths(net, vec![(path, *bid, order)]);
            }
            // For relations and nodes, these cases should handled by other, more atomic
            // transactions. At least it is via GraphBuilder. Paths must be updated though.
            BeliefEvent::NodeRenamed(from, to, _) => {
                self.rename_node(from, to);
            }
            BeliefEvent::RelationChange(..) => {
                // Don't process these, wait to get the resolved entire RelationUpdate event
            }
            BeliefEvent::RelationUpdate(source, sink, weight_set, _) => {
                self.update_relation(source, sink, weight_set);
            }
            BeliefEvent::RelationRemoved(source, sink, _) => {
                self.remove_relation(source, sink);
            }
            BeliefEvent::FileParsed(path) => {
                self.track_file_mtime(path)?;
            }
            BeliefEvent::BalanceCheck => {
                tracing::debug!(
                    "BalanceCheck: DB *should* be balanced now but we're not checking."
                );
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
        self.qb
            .push("INSERT OR REPLACE INTO beliefs(bid, bref, kind, title, schema, payload, id) ");
        self.qb.push_values(vec![belief], |mut b, belief| {
            b.push_bind::<String>(belief.bid.into())
                .push_bind::<String>(belief.bid.bref().to_string())
                .push_bind(belief.kind.as_u32())
                .push_bind::<String>(belief.title.clone())
                .push_bind::<Option<String>>(belief.schema.clone())
                .push_bind::<String>(belief.payload.to_string())
                .push_bind::<Option<String>>(belief.id.clone());
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

    fn rename_node(&mut self, from: &Bid, to: &Bid) {
        self.qb.push("DELETE from beliefs WHERE bid = ");
        self.qb.push_bind::<String>(from.into());
        self.qb.push("; ");
        self.qb
            .push(" UPDATE relations SET source = replace(source, ");
        self.qb.push_bind::<String>(from.into());
        self.qb.push(", ");
        self.qb.push_bind::<String>(to.into());
        self.qb.push(");");
        self.qb.push(" UPDATE relations SET sink = replace(sink, ");
        self.qb.push_bind::<String>(from.into());
        self.qb.push(", ");
        self.qb.push_bind::<String>(to.into());
        self.qb.push(");");
        self.qb.push(" UPDATE paths SET target = replace(target, ");
        self.qb.push_bind::<String>(from.into());
        self.qb.push(", ");
        self.qb.push_bind::<String>(to.into());
        self.qb.push(");");
        self.staged += 1;
    }

    fn add_paths(&mut self, net: &Bref, paths: Vec<(&String, Bid, &Vec<u16>)>) {
        if paths.is_empty() {
            return;
        }
        self.qb
            .push("INSERT OR REPLACE INTO paths(net, path, target, ordering)");
        self.qb
            .push_values(paths, |mut b, (path, target, order_vec)| {
                let order_str = order_vec
                    .iter()
                    .map(|idx| idx.to_string())
                    .collect::<Vec<String>>()
                    .join(".");
                b.push_bind::<String>(net.to_string())
                    .push_bind::<String>(path.clone())
                    .push_bind::<String>(target.into())
                    .push_bind::<String>(order_str);
            });
        self.qb.push(";");
        self.staged += 1;
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
                 (sink, source, epistemic, section, pragmatic) ",
            );
            self.qb.push_values(
                vec![(source, sink, weight_set)],
                |mut b, (source, sink, weight)| {
                    // Serialize each Weight to TOML string for storage
                    let serialize_weight = |w: &crate::properties::Weight| -> String {
                        toml::to_string(w).unwrap_or_default()
                    };

                    b.push_bind::<String>(sink.to_string())
                        .push_bind::<String>(source.to_string())
                        .push_bind(weight.get(&WeightKind::Epistemic).map(serialize_weight))
                        .push_bind(weight.get(&WeightKind::Section).map(serialize_weight))
                        .push_bind(weight.get(&WeightKind::Pragmatic).map(serialize_weight));
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
type NestedNetFuture =
    Pin<Box<dyn Future<Output = Result<Vec<(String, Bid)>, BuildonomyError>> + Send + 'static>>;

#[derive(Debug, Clone)]
pub struct DbConnection(pub Pool<Sqlite>);

/// TODO: ensure push_values iter counts never exceed this huge value
///
/// <https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values>
/// <https://www.sqlite.org/limits.html#max_variable_number>
pub const SQLITE_LIMIT_VARIABLE_NUMBER: usize = 32766;

impl DbConnection {
    #[tracing::instrument(skip(self))]
    async fn get_states<Q>(&self, expr: Q) -> Result<BTreeMap<Bid, BeliefNode>, BuildonomyError>
    where
        Q: AsSql + Debug,
    {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM beliefs WHERE bid IN (");
        expr.build_query(true, &mut qb);
        qb.push(") GROUP BY bid");
        let state_query = qb.build_query_as::<BeliefNode>();
        let state_sql = state_query.sql();

        let results = state_query
            .fetch_all(&self.0)
            .await
            .map_err(|e| {
                tracing::error!(
                    "[DbConnection.get_states] SQL error processing \
                    state_query '{}'\n\terror: {}",
                    state_sql,
                    e
                );
                e
            })?
            .into_iter()
            .map(|s| (s.bid, s))
            .collect::<BTreeMap<Bid, BeliefNode>>();

        // tracing::debug!(
        //     "[get_states] Query for {:?} returned {} results",
        //     expr,
        //     results.len()
        // );

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
                    "[DbConnection.eval_unbalanced] SQL error processing \
                    get all states query\n\terror: {}",
                    e
                );
                e
            })?
            .into_iter()
            .map(|s| (s.bid, s))
            .collect::<BTreeMap<Bid, BeliefNode>>();

        let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM relations;");
        let rel_query = qb.build_query_as::<BeliefRelation>();
        let relation_vec: Vec<BeliefRelation> =
            rel_query.fetch_all(&self.0).await.map_err(|e| {
                tracing::error!(
                    "[DbConnection.eval_unbalanced] SQL error processing \
                    get all relations query\n\terror: {}",
                    e
                );
                e
            })?;
        let relations = BidGraph::from_edges(relation_vec.into_iter());
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

fn get_all_document_paths(
    pool: Pool<Sqlite>,
    network_bid: Bid,
    processed_nets: BTreeSet<Bid>,
) -> NestedNetFuture {
    Box::pin(async move {
        if processed_nets.contains(&network_bid) {
            tracing::debug!(
                "[get_all_document_paths] Skipping already processed network: {}",
                network_bid
            );
            return Ok(vec![]);
        }

        tracing::debug!(
            "[get_all_document_paths] Querying paths for network: {}",
            network_bid
        );

        let rows =
            sqlx::query_as::<_, (String, String)>("SELECT path, target FROM paths WHERE net = ?")
                .bind(network_bid.to_string())
                .fetch_all(&pool)
                .await?;
        let mut row_results = rows
            .into_iter()
            .filter_map(|(path, target)| {
                // Filter out empty paths (network root itself) but keep all other paths
                if path.is_empty() {
                    None
                } else {
                    Bid::try_from(target.as_str()).ok().map(|bid| (path, bid))
                }
            })
            .collect::<Vec<_>>();

        // Identify subnet entries for recursive processing
        let mut row_nets = row_results
            .iter()
            .enumerate()
            .filter_map(|(idx, elem)| {
                // Designates a network directory (no file extension)
                if PathBuf::from(&elem.0).extension().is_none() && !processed_nets.contains(&elem.1)
                {
                    Some((idx, elem.1))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        tracing::debug!(
            "[get_all_document_paths] Found {} subnets in network {}",
            row_nets.len(),
            network_bid
        );
        row_nets.sort_by(|a, b| a.0.cmp(&b.0));
        let new_nets = BTreeSet::from_iter(row_nets.iter().map(|elem| elem.1));
        let mut newly_processed = processed_nets.clone();
        newly_processed.insert(network_bid);
        newly_processed.append(&mut new_nets.clone());
        for new_net in new_nets.iter() {
            // remove new_net from processed just for this call
            let mut newly_processed_for_call = newly_processed.clone();
            newly_processed_for_call.remove(new_net);
            let mut sub_results =
                get_all_document_paths(pool.clone(), *new_net, newly_processed_for_call).await?;

            tracing::debug!(
                "[get_all_document_paths] Subnet {} returned {} documents",
                new_net,
                sub_results.len()
            );

            if !sub_results.is_empty() {
                let Some(row_nets_index) = row_nets.iter().position(|elem| elem.1 == *new_net)
                else {
                    tracing::warn!(
                        "[get_all_document_paths] Subnet {} expected in row_nets but not found (len={})",
                        new_net,
                        row_nets.len()
                    );
                    continue;
                };
                let (start_idx, _net) = row_nets[row_nets_index];
                {
                    let base_ap = AnchorPath::from(&row_results[start_idx].0);
                    for (sub_path, _bid) in sub_results.iter_mut() {
                        *sub_path = base_ap.join(&sub_path);
                    }
                }
                let incr = sub_results.len() - 1; // since not empty, this is always >= 0
                row_results.splice(start_idx..start_idx + 1, sub_results.into_iter());
                // Increment indices to account for our splice
                for net in row_nets.iter_mut().skip(row_nets_index + 1) {
                    net.0 += incr;
                }
            }
        }
        Ok(row_results)
    })
}

fn get_network_paths(
    pool: Pool<Sqlite>,
    network_bid: Bid,
    processed_nets: BTreeSet<Bid>,
) -> NestedNetFuture {
    Box::pin(async move {
        if processed_nets.contains(&network_bid) {
            tracing::debug!(
                "[get_network_paths] Skipping already processed network: {}",
                network_bid
            );
            return Ok(vec![]);
        }

        tracing::debug!(
            "[get_network_paths] Querying paths for network: {}",
            network_bid
        );

        let rows =
            sqlx::query_as::<_, (String, String)>("SELECT path, target FROM paths WHERE net = ?")
                .bind(network_bid.to_string())
                .fetch_all(&pool)
                .await?;
        let mut row_results = rows
            .into_iter()
            .filter_map(|(path, target)| Bid::try_from(target.as_str()).ok().map(|bid| (path, bid)))
            .collect::<Vec<_>>();

        let mut row_nets = row_results
            .iter()
            .enumerate()
            .filter_map(|(idx, elem)| {
                // Designates a network directory (no file extension)
                if PathBuf::from(&elem.0).extension().is_none() && !processed_nets.contains(&elem.1)
                {
                    Some((idx, elem.1))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        tracing::debug!(
            "[get_network_paths] Found {} subnets in network {}",
            row_nets.len(),
            network_bid
        );
        row_nets.sort_by(|a, b| a.0.cmp(&b.0));
        let new_nets = BTreeSet::from_iter(row_nets.iter().map(|elem| elem.1));
        let mut newly_processed = processed_nets.clone();
        newly_processed.insert(network_bid);
        newly_processed.append(&mut new_nets.clone());
        for new_net in new_nets.iter() {
            // remove new_net from processed just for this call
            let mut newly_processed_for_call = newly_processed.clone();
            newly_processed_for_call.remove(new_net);
            let mut sub_results =
                get_network_paths(pool.clone(), *new_net, newly_processed_for_call).await?;

            tracing::debug!(
                "[get_network_paths] Subnet {} returned {} documents",
                new_net,
                sub_results.len()
            );

            if !sub_results.is_empty() {
                let Some(row_nets_index) = row_nets.iter().position(|elem| elem.1 == *new_net)
                else {
                    tracing::warn!(
                        "[get_network_paths] Subnet {} expected in row_nets but not found (len={})",
                        new_net,
                        row_nets.len()
                    );
                    continue;
                };
                let (start_idx, _net) = row_nets[row_nets_index];
                {
                    let base_ap = AnchorPath::from(&row_results[start_idx].0);
                    for (sub_path, _bid) in sub_results.iter_mut() {
                        *sub_path = base_ap.join(&sub_path);
                    }
                }
                let incr = sub_results.len() - 1; // since not empty, this is always >= 0
                row_results.splice(start_idx..start_idx + 1, sub_results.into_iter());
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
    async fn get_file_mtimes(&self) -> Result<BTreeMap<PathBuf, i64>, BuildonomyError> {
        self.get_file_mtimes().await
    }

    /// db eval sets should return the state of all relationships of the primary query --- both
    /// incoming or outgoing. This provides user's bidirectional awareness of how each belief is
    /// using and is used by other beliefs.
    #[tracing::instrument(skip(self))]
    async fn eval_unbalanced(&self, expr: &Expression) -> Result<BeliefGraph, BuildonomyError> {
        let mut states = self.get_states(expr).await?;
        // tracing::debug!(
        //     "[DbConnection.eval_unbalanced] Query returned {} states for expr: {:?}",
        //     states.len(),
        //     expr
        // );

        // For RelationIn queries, mark all nodes as Trace (matches BeliefBase behavior)
        // because we don't guarantee complete relation sets for returned nodes
        let is_relation_query = matches!(expr, Expression::RelationIn(_));
        if is_relation_query {
            for node in states.values_mut() {
                node.kind.insert(BeliefKind::Trace);
            }
        }

        let relations = match !states.is_empty() {
            false => BidGraph::default(),
            true => {
                // ISSUE 34 FIX: Use single query with OR to avoid duplicates
                // Previously used two separate queries (sink IN + source IN) and appended,
                // which caused duplicates when both source and sink were in result set
                let state_set = states
                    .keys()
                    .map(|bid| format!("\"{bid}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                let mut qb = QueryBuilder::new(&format!(
                    "SELECT * FROM relations WHERE sink IN ({state_set}) OR source IN ({state_set});"
                ));
                let relation_query = qb.build_query_as::<BeliefRelation>();
                let relation_sql = relation_query.sql();
                let relation_vec: Vec<BeliefRelation> =
                    relation_query.fetch_all(&self.0).await.map_err(|e| {
                        tracing::error!(
                            "[DbConnection.eval_unbalanced] SQL error processing \
                            relation_query '{}'\n\terror: {}",
                            relation_sql,
                            e
                        );
                        e
                    })?;
                BidGraph::from_edges(relation_vec.into_iter())
            }
        };

        // ISSUE 34 FIX: Check for orphaned edges and load missing nodes
        let temp_graph = BeliefGraph {
            states: states.clone(),
            relations: relations.clone(),
        };
        let missing = temp_graph.find_orphaned_edges();

        if !missing.is_empty() {
            // tracing::debug!(
            //     "[DbConnection.eval_unbalanced] Loading {} missing nodes to complete graph",
            //     missing.len()
            // );
            let missing_expr = Expression::StateIn(StatePred::Bid(missing));
            let mut missing_states = self.get_states(&missing_expr).await?;

            // Mark missing nodes as Trace (incomplete relation set)
            for node in missing_states.values_mut() {
                node.kind.insert(BeliefKind::Trace);
            }

            states.extend(missing_states);
        }

        // tracing::debug!(
        //     "[DbConnection.eval_unbalanced] Returning BeliefGraph with {} states, {} edges",
        //     states.len(),
        //     relations.0.edge_count()
        // );
        Ok(BeliefGraph { states, relations })
    }
    async fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> Result<BeliefGraph, BuildonomyError> {
        let mut states = self.get_states(expr).await?;

        // Mark all queried states as Trace (matches BeliefBase::evaluate_expression_as_trace)
        for node in states.values_mut() {
            node.kind.insert(BeliefKind::Trace);
        }

        let relations = match !states.is_empty() {
            false => BidGraph::default(),
            true => {
                // start the query builder over
                let state_set = states
                    .keys()
                    .map(|bid| format!("\"{bid}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                let mut kind_q = Vec::<String>::new();
                for (kind, _) in weight_filter {
                    let column_name = format!("{kind:?}").to_lowercase();
                    kind_q.push(format!("{column_name} IS NOT NULL",));
                }
                let mut qb = QueryBuilder::new(&format!(
                    "SELECT * FROM relations WHERE source IN ({}) AND {};",
                    state_set,
                    kind_q.join(" AND ")
                ));
                let relation_query = qb.build_query_as::<BeliefRelation>();
                let relation_sql = relation_query.sql();
                let relation_vec: Vec<BeliefRelation> =
                    relation_query.fetch_all(&self.0).await.map_err(|e| {
                        tracing::error!(
                            "[DbConnection.eval_unbalanced] SQL error processing \
                            relation_query '{}'\n\terror: {}",
                            relation_sql,
                            e
                        );
                        e
                    })?;
                BidGraph::from_edges(relation_vec.into_iter())
            }
        };

        // ISSUE 34 FIX: Load missing sink nodes (matches BeliefBase::evaluate_expression_as_trace)
        // eval_trace only loads downstream relations (WHERE source IN), so we need to add missing sinks
        let missing_sinks: Vec<Bid> = relations
            .as_graph()
            .raw_edges()
            .iter()
            .map(|edge| relations.as_graph()[edge.target()])
            .filter(|bid| !states.contains_key(bid))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        if !missing_sinks.is_empty() {
            // tracing::debug!(
            //     "[DbConnection.eval_trace] Loading {} missing sink nodes to complete graph",
            //     missing_sinks.len()
            // );
            let missing_expr = Expression::StateIn(StatePred::Bid(missing_sinks));
            let mut missing_states = self.get_states(&missing_expr).await?;

            // Mark missing nodes as Trace
            for node in missing_states.values_mut() {
                node.kind.insert(BeliefKind::Trace);
            }

            states.extend(missing_states);
        }

        Ok(BeliefGraph { states, relations })
    }

    async fn get_network_paths(
        &self,
        network_bid: Bid,
    ) -> Result<Vec<(String, Bid)>, BuildonomyError> {
        get_network_paths(self.0.clone(), network_bid, BTreeSet::default()).await
    }

    async fn get_all_document_paths(
        &self,
        network_bid: Bid,
    ) -> Result<Vec<(String, Bid)>, BuildonomyError> {
        get_all_document_paths(self.0.clone(), network_bid, BTreeSet::default()).await
    }

    async fn export_beliefgraph(&self) -> Result<BeliefGraph, BuildonomyError> {
        // Get all states from database
        let state_query = sqlx::query_as::<_, BeliefNode>("SELECT * FROM beliefs");
        let states: BTreeMap<Bid, BeliefNode> = state_query
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

        let relations = BidGraph::from_edges(relation_vec.into_iter());

        tracing::info!(
            "Exported BeliefGraph from database: {} states, {} relations",
            states.len(),
            relations.0.edge_count()
        );

        Ok(BeliefGraph { states, relations })
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
    use sqlx::pool::PoolOptions;
    let pool = PoolOptions::<Sqlite>::new()
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // Register the regexp function for this connection
                sqlx::query("SELECT sqlite_compileoption_used('ENABLE_DBSTAT_VTAB')")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?;

    let migrations = MigrationList(vec![
        // Define your migrations here
        Migration {
            version: 1,
            description: "create_initial_tables",
            sql: "\
            CREATE TABLE beliefs (bid TEXT PRIMARY KEY, bref TEXT, kind INTEGER, title TEXT, schema TEXT, payload TEXT, id TEXT); \
            CREATE TABLE relations (sink TEXT, source TEXT, epistemic TEXT, section TEXT, pragmatic TEXT, UNIQUE(sink, source)); \
            CREATE TABLE paths (net TEXT, path TEXT, target TEXT, ordering TEXT, UNIQUE(net, path)); \
            CREATE TABLE file_mtimes (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL);",
            kind: MigrationType::ReversibleUp,
        }
    ]);
    let migrator = Migrator::new(migrations.clone()).await?;
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
