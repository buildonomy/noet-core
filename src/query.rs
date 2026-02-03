use std::{
    cmp,
    collections::BTreeMap,
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
    path::PathBuf,
};

use enumset::EnumSet;
use regex::{escape as re_escape, Regex, RegexBuilder};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

#[cfg(feature = "service")]
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    beliefbase::{BeliefBase, BeliefGraph},
    nodekey::NodeKey,
    properties::{
        AsRun, BeliefKind, BeliefNode, BeliefRefRelation, BeliefRelation, Bid, Bref, WeightKind,
        WeightSet,
    },
    BuildonomyError,
};

pub const DEFAULT_QUERY_DISTANCE: u8 = 5;

/// Recursion Cutoff for query traversal
pub const MAX_TRAVERSAL: u8 = 10;

#[cfg(feature = "service")]
fn push_id_expr<I: ToString>(
    qb: &mut QueryBuilder<Sqlite>,
    bids: &[I],
    column: &str,
    match_pred: bool,
) {
    let last_sep = if !bids.is_empty() { bids.len() - 1 } else { 0 };
    qb.push(column);
    if match_pred {
        qb.push(" IN(");
    } else {
        qb.push(" NOT IN(");
    }
    for (idx, bid) in bids.iter().enumerate() {
        qb.push_bind::<String>(bid.to_string());
        if idx < last_sep {
            qb.push(", ");
        }
    }
    qb.push(") ");
}

#[cfg(feature = "service")]
pub fn push_string_expr(
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

#[cfg(feature = "service")]
fn push_namespace_expr(
    qb: &mut QueryBuilder<Sqlite>,
    namespaces: &[Bref],
    column: &str,
    match_pred: bool,
) {
    let last_sep = if !namespaces.is_empty() {
        namespaces.len() - 1
    } else {
        0
    };
    for (idx, bref) in namespaces.iter().enumerate() {
        qb.push(format!(
            "{} {}LIKE concat('%', ",
            column,
            if match_pred { "" } else { "NOT " }
        ));
        qb.push_bind::<String>(bref.into());
        qb.push(") ");
        if idx < last_sep {
            if match_pred {
                qb.push(" OR ");
            } else {
                qb.push(" AND ");
            }
        }
    }
}

/// Query language for interacting with BeliefGraph and their relations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Hash)]
pub enum Expression {
    StateIn(StatePred),
    StateNotIn(StatePred),
    RelationIn(RelationPred),
    RelationNotIn(RelationPred),
    Dyad(Box<Expression>, SetOp, Box<Expression>),
}

impl Eq for Expression {}

#[cfg(feature = "service")]
impl AsSql for Expression {
    fn build_query(&self, match_pred: bool, qb: &mut QueryBuilder<Sqlite>) {
        match self {
            Expression::StateIn(state_pred) => {
                state_pred.build_query(match_pred, qb);
            }
            Expression::StateNotIn(state_pred) => {
                state_pred.build_query(!match_pred, qb);
            }
            Expression::RelationIn(relation_pred) => {
                relation_pred.build_query(match_pred, qb);
            }
            Expression::RelationNotIn(relation_pred) => {
                relation_pred.build_query(!match_pred, qb);
            }
            Expression::Dyad(lhs_expr, SetOp::SymmetricDifference, rhs_expr) => {
                qb.push("SELECT * FROM ( ");
                Expression::Dyad(lhs_expr.clone(), SetOp::Difference, rhs_expr.clone())
                    .build_query(match_pred, qb);
                qb.push(
                    ") \
                         UNION \
                         SELECT * FROM ( ",
                );
                Expression::Dyad(rhs_expr.clone(), SetOp::Difference, lhs_expr.clone())
                    .build_query(match_pred, qb);
                qb.push(")");
            }
            Expression::Dyad(lhs_expr, op, rhs_expr) => {
                qb.push("SELECT * FROM ( ");
                lhs_expr.as_ref().build_query(match_pred, qb);
                qb.push(") ");
                match op {
                    SetOp::Union => qb.push("UNION "),
                    SetOp::Intersection => qb.push("INTERSECT "),
                    SetOp::Difference | SetOp::SymmetricDifference => qb.push("EXCEPT "),
                };
                qb.push("SELECT * FROM ( ");
                rhs_expr.as_ref().build_query(match_pred, qb);
                qb.push(")");
            }
        }
    }
}

#[cfg(feature = "service")]
impl AsSql for &Expression {
    fn build_query(&self, match_pred: bool, qb: &mut QueryBuilder<Sqlite>) {
        (*self).build_query(match_pred, qb)
    }
}

// TODO/FIXME See Statepred todo/fixme for changing Path and Title predicates
impl From<&NodeKey> for Expression {
    fn from(key: &NodeKey) -> Expression {
        match key {
            NodeKey::Bid { bid } => Expression::StateIn(StatePred::Bid(vec![*bid])),
            NodeKey::Bref { bref } => Expression::StateIn(StatePred::Bref(vec![bref.clone()])),
            NodeKey::Path { net, path } => {
                Expression::StateIn(StatePred::NetPath(*net, path.to_string()))
            }
            NodeKey::Title { net, title } => {
                // TODO/fixme: path can't specify Make title search use the paths table. Doc titles
                // are simply special paths tied to the network
                Expression::StateIn(StatePred::Title(*net, WrappedRegex::from(title.as_str())))
            }
            NodeKey::Id { net: _, id } => Expression::StateIn(StatePred::Id(vec![id.clone()])),
        }
    }
}

/// Filter based on BeliefState properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedRegex(
    #[serde(serialize_with = "serialize_regex")]
    #[serde(deserialize_with = "deserialize_regex")]
    Regex,
);

fn serialize_regex<S>(re: &Regex, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(re.as_str())
}

struct ReVisitor;

impl<'de> de::Visitor<'de> for ReVisitor {
    type Value = Regex;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "A regex string, as validated by the Rust regex crate (https://docs.rs/regex/latest/regex/index.html)", )
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Regex::new(s).map_err(|_e| E::invalid_value(de::Unexpected::Str(s), &self))
    }
}

fn deserialize_regex<'de, D>(deserializer: D) -> Result<Regex, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_str(ReVisitor)
}

impl Hash for WrappedRegex {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state);
    }
}

impl PartialEq for WrappedRegex {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl From<&str> for WrappedRegex {
    fn from(other: &str) -> WrappedRegex {
        WrappedRegex(
            RegexBuilder::new(other)
                .unicode(true)
                .case_insensitive(true)
                .build()
                .unwrap_or(
                    RegexBuilder::new(&re_escape(other))
                        .unicode(true)
                        .case_insensitive(true)
                        .build()
                        .expect("An escaped string to always suceed as a regex"),
                ),
        )
    }
}

impl Deref for WrappedRegex {
    type Target = Regex;
    fn deref(&self) -> &Regex {
        &self.0
    }
}

impl From<Regex> for WrappedRegex {
    fn from(other: Regex) -> WrappedRegex {
        WrappedRegex(other)
    }
}

impl Eq for WrappedRegex {}

/// Filter based on BeliefState properties
///
/// TODO/FIXME Path and Title predicates should mirror nodekey and take a preceeding home_net and
/// home_path argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatePred {
    // Return all nodes
    Any,
    // Return nodes who's parent namespace is contained in the predicate values
    InNamespace(Vec<Bref>),
    // Return nodes who's Bid is contained in the predicate values
    Bid(Vec<Bid>),
    // Return nodes who's bref is contained in the predicate values
    Bref(Vec<Bref>),
    // Return nodes who's Id is contained in the predicate values
    Id(Vec<String>),
    // Return nodes who's schema matches
    Schema(String),
    // Return nodes who's node kind is a superset of the predicate value
    Kind(EnumSet<BeliefKind>),
    // Return node of the specified network path
    NetPath(Bid, String),
    // Return all paths within a network
    NetPathIn(Bid),
    // Return nodes containing a path that equals one of the predicate values
    Path(Vec<String>),
    // Return nodes who's payload or title matches the regex. Bid is the containing network.
    Title(Bid, WrappedRegex),
    // Return nodes who's payload matches the key and regex value
    Payload(String, WrappedRegex),
}

impl StatePred {
    pub fn match_state(&self, node: &BeliefNode) -> bool {
        match self {
            StatePred::Any => true,
            StatePred::InNamespace(ns_vec) => ns_vec.contains(&node.bid.parent_namespace()),
            StatePred::Bid(bid_vec) => bid_vec.contains(&node.bid),
            StatePred::Bref(bref_vec) => bref_vec.contains(&node.bid.namespace()),
            StatePred::Schema(schema) => node.schema.as_ref().filter(|s| *s == schema).is_some(),
            StatePred::Id(id_vec) => node
                .id
                .as_ref()
                .filter(|id_str| id_vec.contains(id_str))
                .is_some(),
            StatePred::Kind(kind_set) => kind_set.is_superset(node.kind.0),
            // Path search needs to be handled separately
            StatePred::Path(..) => false,
            // Path search needs to be handled separately
            StatePred::NetPath(..) => false,
            // Path search needs to be handled separately
            StatePred::NetPathIn(..) => false,
            // Title search needs to be handled separately
            StatePred::Title(..) => false,
            StatePred::Payload(key, re) => {
                if let Some(value) = node.payload.get(key) {
                    re.0.is_match(&value.to_string())
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(feature = "service")]
impl AsSql for StatePred {
    fn build_query(&self, match_pred: bool, qb: &mut QueryBuilder<Sqlite>) {
        match self {
            StatePred::Path(..) | StatePred::NetPath(..) | StatePred::NetPathIn(..) => {
                qb.push(
                    "SELECT DISTINCT target as bid \
                     FROM paths \
                     WHERE ",
                );
            }
            _ => {
                qb.push(
                    "SELECT DISTINCT bid \
                             FROM beliefs \
                             WHERE ",
                );
            }
        }
        match self {
            StatePred::Any => {
                qb.push(format!(
                    "bid {}LIKE '%'",
                    if match_pred { "" } else { "NOT " }
                ));
            }
            StatePred::InNamespace(ns_vec) => {
                push_namespace_expr(qb, ns_vec, "bid", match_pred);
            }
            StatePred::Bid(bid_vec) => {
                push_id_expr(qb, bid_vec, "bid", match_pred);
            }
            StatePred::Bref(bref_vec) => {
                push_id_expr(qb, bref_vec, "bref", match_pred);
            }
            StatePred::Id(id_vec) => {
                push_id_expr(qb, id_vec, "id", match_pred);
            }
            StatePred::Schema(schema) => {
                push_id_expr(qb, &[schema], "schema", match_pred);
            }
            StatePred::Kind(kind_set) => {
                let kind_mask = kind_set.as_u32();
                qb.push("kind & ");
                qb.push_bind(kind_mask);
                if match_pred {
                    qb.push(" = ");
                } else {
                    qb.push(" != ");
                }
                qb.push_bind(kind_mask);
            }
            StatePred::NetPath(net, path) => {
                qb.push("path = ");
                qb.push_bind(path.clone());
                qb.push(" AND net = ");
                qb.push_bind(net.to_string());
            }
            StatePred::NetPathIn(net) => {
                qb.push("net = ");
                qb.push_bind(net.to_string());
            }
            StatePred::Path(path_vec) => {
                push_string_expr(qb, path_vec, "path", match_pred, false);
            }
            // TODO/FIXME this doesn't do any filtering based on containing network
            StatePred::Title(_net, re) => {
                // TODO: Switch back to REGEXP once regexp function registration is fixed
                // For now, using LIKE with wildcards as a workaround
                let pattern = format!("%{}%", re.0.as_str());
                if match_pred {
                    qb.push("title LIKE ");
                } else {
                    qb.push("title NOT LIKE ");
                }
                qb.push_bind(pattern);
            }
            StatePred::Payload(_key, _re_val) => {
                tracing::warn!(
                    "Cannot construct a payload query using the database! Instead, perform a \
                    general query to the database then query the resulting BeliefBase in order \
                    to filter by payload value."
                );
            }
        };
    }
}

/// Filter based on Relation properties
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationPred {
    Any,
    SinkIn(Vec<Bid>),
    SourceIn(Vec<Bid>),
    NodeIn(Vec<Bid>),
    Kind(WeightSet),
}

impl RelationPred {
    pub fn match_relation(&self, rel: BeliefRelation) -> bool {
        self.match_ref(&BeliefRefRelation::from(&rel))
    }

    pub fn match_ref(&self, rel: &BeliefRefRelation) -> bool {
        match self {
            RelationPred::Any => true,
            RelationPred::SinkIn(bid_vec) => bid_vec.contains(rel.sink),
            RelationPred::SourceIn(bid_vec) => bid_vec.contains(rel.source),
            RelationPred::NodeIn(bid_vec) => {
                bid_vec.contains(rel.source) || bid_vec.contains(rel.sink)
            }
            RelationPred::Kind(kind) => !rel.weights.intersection(kind).is_empty(),
        }
    }
}

#[cfg(feature = "service")]
impl AsSql for RelationPred {
    fn build_query(&self, match_pred: bool, qb: &mut QueryBuilder<Sqlite>) {
        fn build_where_clause(
            rpred: &RelationPred,
            match_pred: bool,
            qb: &mut QueryBuilder<Sqlite>,
        ) {
            match rpred {
                RelationPred::Any => {
                    qb.push(format!(
                        "source {}LIKE '%'",
                        if match_pred { "" } else { "NOT " }
                    ));
                }
                RelationPred::SinkIn(bid_vec) => {
                    push_id_expr(qb, bid_vec, "sink", match_pred);
                }
                RelationPred::SourceIn(bid_vec) => {
                    push_id_expr(qb, bid_vec, "source", match_pred);
                }
                RelationPred::NodeIn(bid_vec) => {
                    RelationPred::SourceIn(bid_vec.to_vec()).build_query(match_pred, qb);
                    qb.push(" OR ");
                    RelationPred::SinkIn(bid_vec.to_vec()).build_query(match_pred, qb);
                }
                RelationPred::Kind(kinds) => {
                    let mut kind_q = Vec::<String>::new();
                    for kind in kinds {
                        let column_name = format!("{kind:?}").to_lowercase();
                        kind_q.push(format!(
                            "{} IS{} NULL",
                            column_name,
                            if !match_pred { "" } else { " NOT" }
                        ));
                    }
                    qb.push(kind_q.join(" AND "));
                }
            }
        }

        qb.push(
            "SELECT DISTINCT source as bid \
             FROM relations \
             WHERE ",
        );
        build_where_clause(self, match_pred, qb);
        qb.push(
            "UNION SELECT DISTINCT sink as bid \
             FROM relations \
             WHERE ",
        );
        build_where_clause(self, match_pred, qb);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SetOp {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

#[cfg(feature = "service")]
pub trait AsSql {
    fn build_query(&self, match_pred: bool, qb: &mut QueryBuilder<Sqlite>);
}

/// Cutoff limit for build balance expression recursion
pub const BALANCE_CUTOFF: usize = 10;

pub trait BeliefSource: Sync {
    fn eval_unbalanced(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send;

    /// Get all paths for a network as (path, target_bid) pairs.
    /// Useful for querying asset manifests or all documents in a network.
    /// Default implementation returns empty (in-memory BeliefBase doesn't cache paths).
    fn get_network_paths(
        &self,
        _network_bid: Bid,
    ) -> impl std::future::Future<Output = Result<Vec<(String, Bid)>, BuildonomyError>> + Send;

    /// Evaluate an expression as a trace, marking nodes as Trace and only returning
    /// relations matching the provided weight filter. This prevents pulling in the
    /// entire graph during balance operations.
    fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send;

    /// Get cached file modification times for cache invalidation.
    /// Default implementation returns empty map (no cache invalidation support).
    fn get_file_mtimes(
        &self,
    ) -> impl std::future::Future<Output = Result<BTreeMap<PathBuf, i64>, BuildonomyError>> + Send
    {
        tracing::warn!("This BeliefSource impl does not have a get_file_mtime implementation!");
        async { Ok(BTreeMap::new()) }
    }

    /// Export entire BeliefGraph for serialization (e.g., to JSON for client-side use).
    ///
    /// For BeliefBase: Returns consumed clone of the entire belief set.
    /// For DbConnection: Queries all beliefs and relations from database.
    ///
    /// Default implementation uses eval_unbalanced with StatePred::Any, which may not
    /// be comprehensive for all implementations.
    fn export_beliefgraph(
        &self,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        async {
            // Default: query all states - implementors should override for better performance
            self.eval_unbalanced(&Expression::StateIn(StatePred::Any))
                .await
        }
    }

    fn eval_balanced(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefBase, BuildonomyError>> + Send {
        async {
            let beliefs = self.eval_unbalanced(expr).await?;
            let bset = BeliefBase::from(beliefs);
            bset.is_balanced()?;
            Ok(bset)
        }
    }

    fn eval(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> {
        async {
            let mut set = self.eval_unbalanced(expr).await?;
            self.balance(&mut set).await?;
            Ok(set)
        }
    }

    #[tracing::instrument(skip(self))]
    fn get_async(
        &self,
        key: &NodeKey,
    ) -> impl std::future::Future<Output = Result<Option<BeliefNode>, BuildonomyError>> + Send {
        async {
            let result_set = BeliefBase::from(self.eval_unbalanced(&Expression::from(key)).await?);
            Ok(result_set.get(key))
        }
    }

    /// This will keep querying the [BeliefSource] until the provided set returns an empty option for
    /// [BeliefGraph::build_balance_expr], or the BALANCE_CUTOFF max query depth is reached.
    ///
    /// If balanced, the resulting set may still have relation sources who's Node's are not
    /// known. In order to access those nodes, an explicit query must be exectuted.
    ///
    /// Nodes retrieved during balance operations are marked as Trace, indicating that only
    /// a subset of their relations (specifically Subsection relations) have been loaded.
    fn balance<'a>(
        &'a self,
        set: &'a mut BeliefGraph,
    ) -> impl std::future::Future<Output = Result<(), BuildonomyError>> + Send + 'a {
        async move {
            // Go upstream once in order to jump start our balance expression
            let mut loop_iter = 0;
            // The initial loop needs to find ALL external sinks for our set, not just subsection
            // sinks. This way we can balance any epistemic/pragmatic links we acquired during the
            // primary 'seed' query.
            let mut balance_expr = set.build_downstream_expr(None);

            while let Some(expr) = balance_expr {
                if loop_iter > BALANCE_CUTOFF {
                    tracing::warn!(
                        "Cutting off building of a balanced BeliefBase - \
                         the expression is taking too long to complete."
                    );
                    break;
                }
                // tracing::debug!("loop {}: Processing balance_expr: {:?}", loop_iter, expr);
                // Use eval_trace to only get Subsection relations and mark nodes as Trace
                let balance_set = self
                    .eval_trace(&expr, WeightSet::from(WeightKind::Section))
                    .await?;
                set.union_mut(&balance_set);
                balance_expr = set.build_balance_expr();

                if let Some(ref new_expr) = balance_expr {
                    if *new_expr == expr {
                        if let Expression::StateIn(StatePred::Bid(bids)) = expr {
                            if bids.iter().any(|b| !set.states.contains_key(b)) {
                                tracing::warn!(
                                    "Cache exhausted before all requested nodes could be found."
                                );
                            }
                        }
                        break;
                    }
                }
                loop_iter += 1;
            }
            Ok(())
        }
    }

    /// Parse the NeighborsExpression such that traverse: 1 means go (up/down)stream once.
    ///
    /// all_or_none: Return an empty set if the cache exhausted before the traversal is complete.
    #[tracing::instrument(skip(self))]
    fn eval_query(
        &self,
        query: &Query,
        all_or_none: bool,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        async move {
            let mut bs = self.eval_unbalanced(&query.seed).await?;

            if let Some(ref neighbor_walk) = query.traverse {
                // walk upstream, accrueing relation sources
                let mut upstream_set = None;
                let mut upstream_loop = 0;
                let mut upstream_expr = bs.build_upstream_expr(neighbor_walk.filter.clone());
                let upstream_cutoff = cmp::min(MAX_TRAVERSAL, neighbor_walk.upstream);
                while let Some(up_expr) = upstream_expr {
                    // Traverse 1 should mean loop once, not twice
                    if upstream_loop >= upstream_cutoff {
                        break;
                    }
                    // tracing::debug!("upstream loop {}", upstream_loop);
                    upstream_loop += 1;
                    let up_set = upstream_set.get_or_insert(BeliefGraph {
                        states: BTreeMap::from_iter(bs.states.iter().map(|(k, v)| (*k, v.clone()))),
                        relations: bs.relations.clone(),
                    });
                    let upwalk_eval_set = self.eval_unbalanced(&up_expr).await?;
                    up_set.union_mut(&upwalk_eval_set);
                    upstream_expr = up_set.build_upstream_expr(neighbor_walk.filter.clone());
                    if let Some(ref new_up_expr) = upstream_expr {
                        if *new_up_expr == up_expr {
                            if all_or_none {
                                // tracing::debug!("Returning empty set");
                                return Ok(BeliefGraph::default());
                            } else {
                                // tracing::debug!("breaking traversal");
                                break;
                            }
                        }
                    }
                }

                // walk downstream, accrueing relation sinks
                let mut downstream_set = None;
                let mut downstream_loop = 0;
                let mut downstream_expr = bs.build_downstream_expr(neighbor_walk.filter.clone());
                let downstream_cutoff = cmp::min(MAX_TRAVERSAL, neighbor_walk.downstream);
                while let Some(down_expr) = downstream_expr {
                    // Traverse 1 should mean loop once, not twice
                    if downstream_loop >= downstream_cutoff {
                        break;
                    }
                    // tracing::debug!("downstream loop {}", downstream_loop);
                    downstream_loop += 1;
                    let down_set = downstream_set.get_or_insert(BeliefGraph {
                        states: BTreeMap::from_iter(bs.states.iter().map(|(k, v)| (*k, v.clone()))),
                        relations: bs.relations.clone(),
                    });
                    let downwalk_eval_set = self.eval_unbalanced(&down_expr).await?;
                    down_set.union_mut(&downwalk_eval_set);
                    downstream_expr = down_set.build_downstream_expr(neighbor_walk.filter.clone());
                    if let Some(ref new_down_expr) = downstream_expr {
                        if *new_down_expr == down_expr {
                            if all_or_none {
                                // tracing::debug!("Returning empty set");
                                return Ok(BeliefGraph::default());
                            } else {
                                // tracing::debug!("breaking traversal");
                                break;
                            }
                        }
                    }
                }

                bs = match (upstream_set, downstream_set) {
                    (Some(mut up_set), Some(down_set)) => {
                        // tracing::debug!("joining upstream and downstream sets");
                        up_set.union_mut(&down_set);
                        up_set
                    }
                    (Some(up_set), None) => {
                        // tracing::debug!("returning upstream set");
                        up_set
                    }
                    (None, Some(down_set)) => {
                        // tracing::debug!("returning downstream set");
                        down_set
                    }
                    (None, None) => {
                        // tracing::debug!("returning original eval set");
                        bs
                    }
                }
            }
            if !bs.states.is_empty() {
                self.balance(&mut bs).await?;
                // debug_assert!(BeliefBase::from(bs.clone()).check(true).is_err_and(|e| {
                //     if let BuildonomyError::Custom(msg) = e {
                //         tracing::warn!(
                //             "Query results for {:?} aren't balanced! errors are:\n\t{}",
                //             query,
                //             msg.replace("\n", "\n\t")
                //         );
                //         true
                //     } else {
                //         false
                //     }
                // }));
            }
            Ok(bs)
        }
    }
}

pub const DEFAULT_LIMIT: usize = 100;
pub const DEFAULT_OFFSET: usize = 0;

/// A page of results from the sqlx cache of BeliefState or BeliefRelation objects.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultsPage<B> {
    pub count: usize,
    pub start: usize,
    pub results: B,
}

/// Target for a query expression, can either be the full belief cache, or a depth-limited graph traversal
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NeighborsExpression {
    pub filter: Option<WeightSet>,
    pub upstream: u8,
    pub downstream: u8,
}

/// A page of results from the sqlx cache of BeliefState or BeliefRelation objects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Query {
    pub seed: Expression,
    pub traverse: Option<NeighborsExpression>,
}

/// A page of results from the sqlx cache of BeliefState or BeliefRelation objects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaginatedQuery {
    pub query: Query,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

impl Eq for PaginatedQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Focus {
    pub awareness: Vec<Bid>,
    pub radius: u8,
    pub attention: Vec<AsRun>,
}

impl Default for Focus {
    fn default() -> Self {
        Focus {
            awareness: Vec::default(),
            radius: DEFAULT_QUERY_DISTANCE,
            attention: Vec::default(),
        }
    }
}

impl Focus {
    pub fn as_query(&self) -> Option<Query> {
        if !self.awareness.is_empty() {
            Some(Query {
                seed: Expression::StateIn(StatePred::Bid(self.awareness.clone())),
                traverse: Some(NeighborsExpression {
                    filter: None,
                    upstream: self.radius,
                    downstream: self.radius,
                }),
            })
        } else {
            None
        }
    }

    pub fn get_run_mut(&mut self, path: &str) -> Option<&mut AsRun> {
        self.attention.iter_mut().find(|ar| ar.doc_path == path)
    }

    pub fn get_run(&self, path: &str) -> Option<&AsRun> {
        self.attention.iter().find(|ar| ar.doc_path == path)
    }
}
