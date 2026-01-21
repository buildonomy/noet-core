use crate::{
    beliefset::{BeliefSet, Beliefs, BidGraph},
    error::BuildonomyError,
    event::BeliefEvent,
    properties::{BeliefNode, BeliefRelation, Bid, WeightKind, WeightSet},
    query::{push_string_expr, AsSql, BeliefCache, Expression},
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
use std::{path::PathBuf, str::FromStr};

pub const BELIEF_CACHE_DB: &str = "sqlite:belief_cache.db";

pub struct Transaction<'a> {
    qb: QueryBuilder<'a, Sqlite>,
    pub staged: usize,
}

impl<'a> Transaction<'a> {
    pub fn new() -> Transaction<'a> {
        Transaction {
            qb: QueryBuilder::<Sqlite>::new(""),
            staged: 0,
        }
    }

    pub async fn execute(&mut self, connection: &Pool<Sqlite>) -> Result<(), BuildonomyError> {
        let query = self.qb.build();
        tracing::debug!("Executing SQL for {} events", self.staged);
        // tracing::debug!("SQL:\n{}", query.sql());
        query.execute(connection).await?;
        self.qb.reset();
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
        tracing::debug!("Transaction::add_event {:?}", event);
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
            // transactions. At least it is via Beliefsetaccumulator. Paths must be updated though.
            BeliefEvent::NodeRenamed(from, to, _) => {
                self.rename_node(from, to);
            }
            BeliefEvent::RelationInsert(source, sink, kind, payload, _) => {
                let mut weight_set = WeightSet::empty();
                weight_set.set(*kind, payload.clone());
                self.update_relation(&source, &sink, &weight_set);
            }
            BeliefEvent::RelationUpdate(source, sink, weight_set, _) => {
                self.update_relation(&source, &sink, weight_set);
            }
            BeliefEvent::RelationRemoved(source, sink, _) => {
                self.remove_relation(source, sink);
            }
            BeliefEvent::BalanceCheck => {
                tracing::debug!(
                    "BalanceCheck: DB *should* be balanced now but we're not checking."
                );
            }
            BeliefEvent::BuiltInTest => {
                tracing::debug!(
                    "BuiltInTest: All BeliefSet invariants *should* be true now but we're not checking."
                );
            }
        }
        Ok(())
    }

    fn update_node(&mut self, belief: &BeliefNode) {
        self.qb
            .push("INSERT OR REPLACE INTO beliefs(bid, bref, kind, title, schema, payload) ");
        self.qb.push_values(vec![belief], |mut b, belief| {
            b.push_bind::<String>(belief.bid.into())
                .push_bind::<String>(belief.bid.namespace().into())
                .push_bind(belief.kind.as_u32())
                .push_bind::<String>(belief.title.clone())
                .push_bind::<Option<String>>(belief.schema.clone())
                .push_bind::<String>(belief.payload.to_string());
        });
        self.qb.push("; ");
        self.staged += 1;
    }

    fn remove_nodes(&mut self, nodes: &Vec<Bid>) {
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

    fn add_paths(&mut self, net: &Bid, paths: Vec<(&String, Bid, &Vec<u16>)>) {
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
                b.push_bind::<String>(net.into())
                    .push_bind::<String>(path.clone())
                    .push_bind::<String>(target.into())
                    .push_bind::<String>(order_str);
            });
        self.qb.push(";");
        self.staged += 1;
    }

    fn remove_paths(&mut self, net: &Bid, paths: &Vec<String>) {
        if paths.is_empty() {
            return;
        }
        self.qb.push("DELETE from paths WHERE net = ");
        self.qb.push_bind::<String>(net.into());
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
                 (sink, source, epistemic, subsection, pragmatic) ",
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

#[derive(Debug, Clone)]
pub struct DbConnection(pub(crate) Pool<Sqlite>);

/// TODO: ensure push_values iter counts never exceed this huge value
///
/// <https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values>
/// <https://www.sqlite.org/limits.html#max_variable_number>
// pub const SQLITE_LIMIT_VARIABLE_NUMBER: usize = 32766;

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
        // tracing::debug!("SQL: {}", state_sql);
        Ok(state_query
            .fetch_all(&self.0)
            .await
            .map_err(|e| {
                tracing::error!(
                    "[DbConnection.eval_unbalanced] SQL error processing \
                    state_query '{}'\n\terror: {}",
                    state_sql,
                    e
                );
                e
            })?
            .into_iter()
            .map(|s| (s.bid, s))
            .collect::<BTreeMap<Bid, BeliefNode>>())
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
        let bs = BeliefSet::new_unbalanced(states, relations, false);

        bs.is_balanced()
    }
}

impl BeliefCache for DbConnection {
    /// db eval sets should return the state of all relationships of the primary query --- both
    /// incoming or outgoing. This provides user's bidirectional awareness of how each belief is
    /// using and is used by other beliefs.
    #[tracing::instrument(skip(self))]
    async fn eval_unbalanced(&self, expr: &Expression) -> Result<Beliefs, BuildonomyError> {
        let states = self.get_states(expr).await?;
        let relations = match !states.is_empty() {
            false => BidGraph::default(),
            true => {
                // start the query builder over
                let state_set = states
                    .keys()
                    .map(|bid| format!("\"{}\"", bid))
                    .collect::<Vec<String>>()
                    .join(", ");
                let mut qb = QueryBuilder::new(&format!(
                    "SELECT * FROM relations WHERE sink IN ({});",
                    state_set
                ));
                let relation_query = qb.build_query_as::<BeliefRelation>();
                let relation_sql = relation_query.sql();
                let mut relation_vec: Vec<BeliefRelation> =
                    relation_query.fetch_all(&self.0).await.map_err(|e| {
                        tracing::error!(
                            "[DbConnection.eval_unbalanced] SQL error processing \
                            relation_query '{}'\n\terror: {}",
                            relation_sql,
                            e
                        );
                        e
                    })?;

                qb = QueryBuilder::new(&format!(
                    "SELECT * FROM relations WHERE source IN ({});",
                    state_set
                ));
                let relation_query = qb.build_query_as::<BeliefRelation>();
                let relation_sql = relation_query.sql();
                let mut source_side = relation_query.fetch_all(&self.0).await.map_err(|e| {
                    tracing::error!(
                        "[DbConnection.eval_unbalanced] SQL error processing \
                        relation_query '{}'\n\terror: {}",
                        relation_sql,
                        e
                    );
                    e
                })?;
                relation_vec.append(&mut source_side);
                BidGraph::from_edges(relation_vec.into_iter())
            }
        };

        Ok(Beliefs { states, relations })
    }
    async fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> Result<Beliefs, BuildonomyError> {
        let states = self.get_states(expr).await?;
        let relations = match !states.is_empty() {
            false => BidGraph::default(),
            true => {
                // start the query builder over
                let state_set = states
                    .keys()
                    .map(|bid| format!("\"{}\"", bid))
                    .collect::<Vec<String>>()
                    .join(", ");
                let mut kind_q = Vec::<String>::new();
                for (kind, _) in weight_filter {
                    let column_name = format!("{:?}", kind).to_lowercase();
                    kind_q.push(format!("{} IS NOT NULL", column_name,));
                }
                let mut qb = QueryBuilder::new(&format!(
                    "SELECT * FROM relations WHERE sink IN ({}) AND {};",
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

        Ok(Beliefs { states, relations })
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
        .disable_statement_logging();
    let pool = Pool::<Sqlite>::connect_with(options).await?;

    let migrations = MigrationList(vec![
        // Define your migrations here
        Migration {
            version: 1,
            description: "create_initial_tables",
            sql: "\
            CREATE TABLE beliefs (bid TEXT PRIMARY KEY, bref TEXT, kind INTEGER, title TEXT, schema TEXT, payload TEXT); \
            CREATE TABLE relations (sink TEXT, source TEXT, epistemic TEXT, subsection TEXT, pragmatic TEXT, UNIQUE(sink, source)); \
            CREATE TABLE paths (net TEXT, path TEXT, target TEXT, ordering TEXT, UNIQUE(net, path));",
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
