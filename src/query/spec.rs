// query/spec.rs — Core types and evaluation for the unified query infrastructure.
//
// This module defines the QuerySpec unified pipeline model: a single
// Vec<ProjectionStep> where the seed (starting BID set) is expressed as a
// TapeFn variant on a step's input.
// See docs/design/query_model.md §3–§7 for the formal model.
//
// Evaluation is handled by the query evaluator (see evaluator.rs).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::hash::Hasher;
use std::ops::{Deref, Range};
use std::str::FromStr;

use petgraph::stable_graph::EdgeIndex;

use enumset::{EnumSet, EnumSetType};
use regex::{escape as re_escape, Regex, RegexBuilder};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use toml::value::Table;

use crate::beliefbase::BeliefGraph;
use crate::nodekey::NodeKey;
use crate::properties::{BeliefNode, Bid, Bref, Weight, WeightKind, WeightSet};
use crate::BuildonomyError;

// ═══════════════════════════════════════════════════════════════════════════════
// WrappedRegex — serde-friendly regex wrapper
// ═══════════════════════════════════════════════════════════════════════════════

/// A [`Regex`] wrapper that implements `Hash`, `Eq`, `Serialize`, and `Deserialize`
/// by delegating to the pattern string.
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

impl std::hash::Hash for WrappedRegex {
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

// ═══════════════════════════════════════════════════════════════════════════════
// Top-level QuerySpec
// ═══════════════════════════════════════════════════════════════════════════════

/// A complete query specification with two orthogonal components.
///
/// The steps pipeline combines seed selection (via TapeFn variants on step
/// inputs) with transformation steps (filter, traverse, compose, identity).
///
/// See query_model.md §3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuerySpec {
    pub steps: Vec<ProjectionStep>,
}

impl QuerySpec {
    /// Construct a QuerySpec from a list of steps.
    pub fn new(steps: Vec<ProjectionStep>) -> Self {
        Self { steps }
    }

    /// Construct a seed-only QuerySpec (seed TapeFn + Identity operation).
    pub fn seed(seed: TapeFn) -> Self {
        Self {
            steps: vec![ProjectionStep::with_input(seed, StepOperation::Identity)],
        }
    }

    /// Construct a QuerySpec with a seed TapeFn applied to the first step's input.
    /// If `steps` is empty, creates a seed-only query with Identity.
    pub fn seed_then(seed: TapeFn, mut steps: Vec<ProjectionStep>) -> Self {
        if steps.is_empty() {
            return Self::seed(seed);
        }
        steps[0].input = seed;
        Self { steps }
    }

    /// True if the first step's input is `TapeFn::Corpus`.
    pub fn is_corpus(&self) -> bool {
        self.steps
            .first()
            .is_some_and(|s| matches!(s.input, TapeFn::Corpus))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Projection (transformation chain)
// ═══════════════════════════════════════════════════════════════════════════════

/// A single step in the projection chain. See query_model.md §5.
///
/// Each step declares a label (for tape entry naming), how it receives its
/// input from the tape (`input`, a `TapeFn`), and what operation to perform.
///
/// In the textual grammar, `THEN` (implicit or explicit) produces
/// `TapeFn::Then(None)`; `FOLD` produces `TapeFn::Fold { .. }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionStep {
    /// Names tape entries for this step. Multi-hop traversals produce
    /// multiple entries sharing the same label; hop index is derived from
    /// position within the label group. Defaults to step index ("0", "1", …)
    /// when not user-specified. See query_model.md §5.5, §6.
    #[serde(default)]
    pub label: String,
    /// How this step receives its input set from the tape.
    #[serde(default)]
    pub input: TapeFn,
    /// The transformation to apply.
    pub operation: StepOperation,
}

impl ProjectionStep {
    /// Construct a filter step with default (Then) input and auto label.
    pub fn filter(filter: NodeFilter) -> Self {
        Self {
            label: String::new(),
            input: TapeFn::default(),
            operation: StepOperation::Filter(filter),
        }
    }

    /// Construct a traversal step with default (Then) input and auto label.
    pub fn traverse(spec: TraversalSpec) -> Self {
        Self {
            label: String::new(),
            input: TapeFn::default(),
            operation: StepOperation::Traverse(spec),
        }
    }

    /// Construct a composition step with default (Then) input and auto label.
    pub fn compose(comp: Composition) -> Self {
        Self {
            label: String::new(),
            input: TapeFn::default(),
            operation: StepOperation::Compose(comp),
        }
    }

    /// Construct an identity step (pass-through) with default input.
    pub fn identity() -> Self {
        Self {
            label: String::new(),
            input: TapeFn::default(),
            operation: StepOperation::Identity,
        }
    }

    /// Construct a step with explicit input mode and auto label.
    pub fn with_input(input: TapeFn, operation: StepOperation) -> Self {
        Self {
            label: String::new(),
            input,
            operation,
        }
    }
}

impl From<StepOperation> for ProjectionStep {
    fn from(operation: StepOperation) -> Self {
        Self {
            label: String::new(),
            input: TapeFn::default(),
            operation,
        }
    }
}

/// The transformation a projection step performs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StepOperation {
    /// Zero-hop property filter. See §5.1.
    Filter(NodeFilter),
    /// Graph walk. See §5.2.
    Traverse(TraversalSpec),
    /// Set-algebraic composition of sub-chains. See §5.3.
    Compose(Composition),
    /// Pass-through: output = input. Used for seed-only queries where
    /// the TapeFn produces the BID set and no transformation is needed.
    Identity,
}

/// Set-algebraic composition of two projection sub-chains.
///
/// Each branch is an independent projection chain evaluated against the same
/// input set. The results are combined according to `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub left: Vec<ProjectionStep>,
    pub op: CompositionOp,
    pub right: Vec<ProjectionStep>,
}

/// Set-algebraic operator for [`Composition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositionOp {
    /// Intersection — score = min(left, right).
    And,
    /// Union — score = max(left, right).
    Or,
    /// Exclusion — left minus right (A AND NOT B). The gap/complement
    /// operator: items in the left set not covered by the right set.
    Not,
}

// ═══════════════════════════════════════════════════════════════════════════════
// NodeFilter (zero-hop filter)
// ═══════════════════════════════════════════════════════════════════════════════

/// A zero-hop projection step that down-selects the node set based on
/// node-intrinsic properties. No relation traversal.
///
/// Combining multiple filters uses [`Composition`] at the projection level,
/// not filter-level conjunction. This keeps one representation for set
/// algebra regardless of whether branches contain filters, traversals, or
/// both.
///
/// **Optimization opportunity**: When a `Composition` has pure `Filter`
/// branches on both sides, the evaluator can speculatively lower the entire
/// composition to a single SQL expression (two `StatePred`s joined by
/// `INTERSECT`/`UNION`/`EXCEPT`). If one branch contains a traversal or
/// other non-lowerable step, fall back to independent branch evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeFilter {
    /// Hard boolean test against a node property.
    Predicate(PropertyPredicate),
    /// Soft-scored text search against a node property field.
    TextMatch { path: PropertyPath, query: String },
}

/// Callback trait for evaluating `TextMatch` filters.
///
/// `BeliefBase::apply_filter` delegates `TextMatch` evaluation to this trait
/// when one is provided. The WASM viewer implements it using loaded search
/// indices; the compile-time pipeline does not (yet) support it.
///
/// The method receives the query string from the `TextMatch` filter and returns
/// a set of matching BIDs with their TF-IDF scores. The caller intersects
/// this set with the current pipeline BID set.
pub trait TextSearchProvider {
    /// Run a full-text search and return matching BIDs with scores.
    fn text_search(&self, query: &str) -> Vec<(crate::properties::Bid, f64)>;
}

// ═══════════════════════════════════════════════════════════════════════════════
// PropertyPredicate
// ═══════════════════════════════════════════════════════════════════════════════

/// A single property test: resolve `path` on a [`BeliefNode`], then apply `op`
/// with `value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyPredicate {
    pub path: PropertyPath,
    pub op: CompareOp,
    pub value: PropertyValue,
}

/// TOML-style dotted key resolution path. Each segment is a table key,
/// array index, or wildcard.
///
/// # Examples
///
/// | Input string         | Parsed segments                                     |
/// |----------------------|-----------------------------------------------------|
/// | `"schema"`           | `[Key("schema")]`                                   |
/// | `"payload.status"`   | `[Key("payload"), Key("status")]`                   |
/// | `"payload.listing.0"`| `[Key("payload"), Key("listing"), Index(0)]`         |
/// | `"payload.*"`        | `[Key("payload"), Wildcard]`                         |
pub type PropertyPath = Vec<PropertySegment>;

/// A single segment in a property path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PropertySegment {
    /// Table key lookup.
    Key(String),
    /// Array element by index.
    Index(usize),
    /// Array slice (start inclusive, end exclusive).
    Slice(usize, usize),
    /// `*` — all keys/elements at this level (shallow). Predicates match if
    /// *any* resolved value satisfies the [`CompareOp`].
    Wildcard,
    /// `**` — all keys at any depth (recursive). Specified but deferred;
    /// resolution currently returns an empty set.
    GlobStar,
}

/// Comparison operator for a [`PropertyPredicate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompareOp {
    Eq,
    NotEq,
    /// Value is in a set (e.g., `kind in {Document, Symbol}`).
    In,
    /// Value matches a regex pattern.
    Matches,
    /// String contains substring, or array contains element.
    Contains,
    /// Path resolves to at least one value (no `value` operand needed).
    Exists,
    Gt,
    Lt,
    Gte,
    Lte,
}

/// The right-hand operand of a [`PropertyPredicate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    String(String),
    Number(f64),
    /// Set of values for [`CompareOp::In`] (e.g., `kind in {Document, Symbol}`).
    Set(Vec<String>),
    /// Regex pattern string for [`CompareOp::Matches`].
    Regex(String),
    /// No value — used with [`CompareOp::Exists`].
    None,
}

// PropertyValue uses f64::to_bits() for Hash/Eq. This is correct for cache-key
// identity: same predicate = same bits. We never compare computed results — only
// user-supplied literals against stored literals, so IEEE 754 representation
// equality is the right semantics. Query evaluation concerns (epsilon tolerance,
// NaN handling) are handled in PropertyPredicate::evaluate, not here.
impl Eq for PropertyValue {}

impl std::hash::Hash for PropertyValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            PropertyValue::String(s) => s.hash(state),
            PropertyValue::Number(n) => n.to_bits().hash(state),
            PropertyValue::Set(v) => v.hash(state),
            PropertyValue::Regex(r) => r.hash(state),
            PropertyValue::None => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PropertyPath parsing
// ═══════════════════════════════════════════════════════════════════════════════

/// Error from parsing a path segment.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("invalid path segment '{segment}': {message}")]
pub struct PathParseError {
    pub segment: String,
    pub message: String,
}

impl PropertySegment {
    /// Parse a single dotted-key segment string.
    ///
    /// Recognition order: `*` → `**` → slice (`N:M`) → index (`N`) → key.
    ///
    /// Returns `Err` if the segment contains `:` but isn't a valid slice
    /// (e.g., `"foo:bar"`) — this likely indicates confusion between the
    /// path separator and the `TextMatch` colon operator.
    pub fn parse(s: &str) -> Result<Self, PathParseError> {
        if s == "*" {
            Ok(PropertySegment::Wildcard)
        } else if s == "**" {
            Ok(PropertySegment::GlobStar)
        } else if let Some(colon_pos) = s.find(':') {
            // Slice syntax: "0:4"
            match (
                s[..colon_pos].parse::<usize>(),
                s[colon_pos + 1..].parse::<usize>(),
            ) {
                (Ok(start), Ok(end)) => Ok(PropertySegment::Slice(start, end)),
                _ => Err(PathParseError {
                    segment: s.to_string(),
                    message: "contains ':' but is not a valid slice (expected N:M \
                              where N and M are integers); if you meant a text search, \
                              use 'path:term' outside the dotted path"
                        .to_string(),
                }),
            }
        } else if let Ok(idx) = s.parse::<usize>() {
            Ok(PropertySegment::Index(idx))
        } else {
            Ok(PropertySegment::Key(s.to_string()))
        }
    }
}

/// Parse a dotted property path string into segments.
///
/// Returns `Err` if any segment is malformed (e.g., contains `:` but
/// isn't a valid slice).
///
/// # Examples
///
/// ```
/// use noet_core::query::parse_property_path;
/// let path = parse_property_path("payload.status").unwrap();
/// assert_eq!(path.len(), 2); // [Key("payload"), Key("status")]
/// ```
pub fn parse_property_path(input: &str) -> Result<PropertyPath, PathParseError> {
    input.split('.').map(PropertySegment::parse).collect()
}

// ═══════════════════════════════════════════════════════════════════════════════
// PropertyPath resolution against BeliefNode
// ═══════════════════════════════════════════════════════════════════════════════

/// Diagnostic produced during property path resolution. These indicate
/// likely authoring errors (not fatal — the query still runs, but results
/// may be empty for unexpected reasons).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ResolveDiagnostic {
    /// A path segment resolved, but the value's type doesn't support the
    /// next operation (e.g., `Index` on a `String`, `Slice` on a `Table`).
    #[error("segment {at_segment}: expected {expected}, found {found}")]
    TypeMismatch {
        at_segment: usize,
        expected: &'static str,
        found: &'static str,
    },
    /// A key was not found in a table.
    #[error("segment {at_segment}: key '{key}' not found")]
    NotFound { at_segment: usize, key: String },
    /// BeliefNode could not be serialized to TOML for path resolution.
    #[error("node serialization failed: {message}")]
    SerializationFailed { message: String },
    /// `GlobStar` (`**`) is specified but not yet implemented.
    #[error("segment {at_segment}: '**' glob not yet implemented")]
    GlobStarNotImplemented { at_segment: usize },
    /// Index out of bounds on an array.
    #[error("segment {at_segment}: index {index} out of bounds (length {length})")]
    IndexOutOfBounds {
        at_segment: usize,
        index: usize,
        length: usize,
    },
}

impl From<ResolveDiagnostic> for BuildonomyError {
    fn from(diag: ResolveDiagnostic) -> Self {
        BuildonomyError::Command(format!("query path resolution: {diag}"))
    }
}

impl From<PathParseError> for BuildonomyError {
    fn from(err: PathParseError) -> Self {
        BuildonomyError::Command(format!("query path parse: {err}"))
    }
}

/// Result of resolving a [`PropertyPath`] against a [`BeliefNode`].
///
/// Always contains the resolved values (possibly empty). Additionally
/// carries diagnostics for type mismatches, missing keys, etc. that
/// help authors debug unexpected empty results.
#[derive(Debug, Clone, Default)]
pub struct ResolveResult {
    /// Successfully resolved values. Empty if the path didn't match.
    pub values: Vec<toml::Value>,
    /// Diagnostics produced during resolution. Non-fatal — the query
    /// proceeds with whatever values were resolved.
    pub diagnostics: Vec<ResolveDiagnostic>,
}

impl ResolveResult {
    fn value(v: toml::Value) -> Self {
        Self {
            values: vec![v],
            diagnostics: vec![],
        }
    }

    fn values(values: Vec<toml::Value>) -> Self {
        Self {
            values,
            diagnostics: vec![],
        }
    }

    fn empty() -> Self {
        Self::default()
    }

    fn with_diagnostic(diag: ResolveDiagnostic) -> Self {
        Self {
            values: vec![],
            diagnostics: vec![diag],
        }
    }

    fn extend(&mut self, other: ResolveResult) {
        self.values.extend(other.values);
        self.diagnostics.extend(other.diagnostics);
    }
}

/// Resolve a property path against a [`BeliefNode`], returning matched
/// values and any diagnostics.
///
/// Concrete paths (no wildcards) return zero or one value.
/// Wildcards may return multiple values. Diagnostics are produced for
/// type mismatches, missing keys, and other resolution issues.
pub fn resolve_property_path(node: &BeliefNode, path: &PropertyPath) -> ResolveResult {
    if path.is_empty() {
        return ResolveResult::empty();
    }

    // Serialize the node to a TOML value and walk it uniformly.
    // This avoids hand-mapping each BeliefNode field — the serde
    // representation is the source of truth for field names and types.
    let node_value = match toml::Value::try_from(node) {
        Ok(v) => v,
        Err(e) => {
            return ResolveResult::with_diagnostic(ResolveDiagnostic::SerializationFailed {
                message: e.to_string(),
            });
        }
    };
    resolve_in_value(&node_value, path, 0)
}

/// Resolve remaining path segments within a TOML table.
///
/// `depth` is the zero-based segment offset from the original path,
/// used to position diagnostics.
fn resolve_in_table(table: &Table, path: &[PropertySegment], depth: usize) -> ResolveResult {
    if path.is_empty() {
        return ResolveResult::value(toml::Value::Table(table.clone()));
    }

    match &path[0] {
        PropertySegment::Key(key) => match table.get(key) {
            Some(value) => resolve_in_value(value, &path[1..], depth + 1),
            None => ResolveResult::with_diagnostic(ResolveDiagnostic::NotFound {
                at_segment: depth,
                key: key.clone(),
            }),
        },
        PropertySegment::Wildcard => {
            let mut result = ResolveResult::empty();
            for value in table.values() {
                result.extend(resolve_in_value(value, &path[1..], depth + 1));
            }
            result
        }
        PropertySegment::GlobStar => {
            ResolveResult::with_diagnostic(ResolveDiagnostic::GlobStarNotImplemented {
                at_segment: depth,
            })
        }
        PropertySegment::Index(_) | PropertySegment::Slice(_, _) => {
            ResolveResult::with_diagnostic(ResolveDiagnostic::TypeMismatch {
                at_segment: depth,
                expected: "Array",
                found: "Table",
            })
        }
    }
}

/// Resolve remaining path segments within a TOML value.
///
/// `depth` is the zero-based segment offset from the original path,
/// used to position diagnostics.
fn resolve_in_value(value: &toml::Value, path: &[PropertySegment], depth: usize) -> ResolveResult {
    if path.is_empty() {
        return ResolveResult::value(value.clone());
    }

    match value {
        toml::Value::Table(table) => resolve_in_table(table, path, depth),
        toml::Value::Array(arr) => match &path[0] {
            PropertySegment::Index(idx) => match arr.get(*idx) {
                Some(v) => resolve_in_value(v, &path[1..], depth + 1),
                None => ResolveResult::with_diagnostic(ResolveDiagnostic::IndexOutOfBounds {
                    at_segment: depth,
                    index: *idx,
                    length: arr.len(),
                }),
            },
            PropertySegment::Slice(start, end) => {
                let end = (*end).min(arr.len());
                let start = (*start).min(end);
                if path.len() == 1 {
                    ResolveResult::values(arr[start..end].to_vec())
                } else {
                    let mut result = ResolveResult::empty();
                    for v in &arr[start..end] {
                        result.extend(resolve_in_value(v, &path[1..], depth + 1));
                    }
                    result
                }
            }
            PropertySegment::Wildcard => {
                let mut result = ResolveResult::empty();
                for v in arr {
                    result.extend(resolve_in_value(v, &path[1..], depth + 1));
                }
                result
            }
            PropertySegment::Key(_) => {
                ResolveResult::with_diagnostic(ResolveDiagnostic::TypeMismatch {
                    at_segment: depth,
                    expected: "Table",
                    found: "Array",
                })
            }
            PropertySegment::GlobStar => {
                ResolveResult::with_diagnostic(ResolveDiagnostic::GlobStarNotImplemented {
                    at_segment: depth,
                })
            }
        },
        // Scalar values have no sub-paths
        _ => ResolveResult::with_diagnostic(ResolveDiagnostic::TypeMismatch {
            at_segment: depth,
            expected: "Table or Array",
            found: toml_type_name(value),
        }),
    }
}

/// Return a human-readable type name for a TOML value.
fn toml_type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "String",
        toml::Value::Integer(_) => "Integer",
        toml::Value::Float(_) => "Float",
        toml::Value::Boolean(_) => "Boolean",
        toml::Value::Datetime(_) => "Datetime",
        toml::Value::Array(_) => "Array",
        toml::Value::Table(_) => "Table",
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PropertyPredicate evaluation
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of evaluating a [`PropertyPredicate`] against a [`BeliefNode`].
/// Carries both the boolean outcome and any diagnostics from path resolution
/// or type mismatches during comparison.
#[derive(Debug, Clone, Default)]
pub struct PredicateResult {
    /// Whether the predicate matched.
    pub matched: bool,
    /// Diagnostics from path resolution and value comparison.
    pub diagnostics: Vec<ResolveDiagnostic>,
}

impl PropertyPredicate {
    /// Evaluate this predicate against a [`BeliefNode`], returning the
    /// match result along with any diagnostics.
    pub fn evaluate(&self, node: &BeliefNode) -> PredicateResult {
        let resolved = resolve_property_path(node, &self.path);
        let mut diagnostics = resolved.diagnostics;

        let matched = match self.op {
            CompareOp::Exists => !resolved.values.is_empty(),
            _ => {
                let mut any_matched = false;
                for v in &resolved.values {
                    let outcome = self.compare_value(v);
                    diagnostics.extend(outcome.diagnostics);
                    if outcome.matched {
                        any_matched = true;
                    }
                }
                any_matched
            }
        };

        PredicateResult {
            matched,
            diagnostics,
        }
    }

    /// Compare a single resolved TOML value against this predicate's value.
    pub(crate) fn compare_value(&self, resolved: &toml::Value) -> PredicateResult {
        let (matched, diag) = match (&self.op, &self.value) {
            // ── Equality ─────────────────────────────────────────────────────
            // For arrays, Eq checks if any element matches.
            (CompareOp::Eq, PropertyValue::String(s)) => match resolved {
                toml::Value::Array(arr) => (
                    arr.iter().any(|item| item.as_str().is_some_and(|v| v == s)),
                    None,
                ),
                toml::Value::String(v) => (v == s, None),
                _ => (false, Some(compare_type_mismatch("String", resolved))),
            },
            (CompareOp::Eq, PropertyValue::Number(n)) => match value_as_f64(resolved) {
                Some(v) => (v == *n, None),
                None => (false, Some(compare_type_mismatch("Number", resolved))),
            },
            (CompareOp::NotEq, PropertyValue::String(s)) => match resolved {
                toml::Value::Array(arr) => (
                    !arr.iter().any(|item| item.as_str().is_some_and(|v| v == s)),
                    None,
                ),
                toml::Value::String(v) => (v != s, None),
                _ => (false, Some(compare_type_mismatch("String", resolved))),
            },
            (CompareOp::NotEq, PropertyValue::Number(n)) => match value_as_f64(resolved) {
                Some(v) => (v != *n, None),
                None => (false, Some(compare_type_mismatch("Number", resolved))),
            },

            // ── Set membership ─────────────────────────────────────────────
            (CompareOp::In, PropertyValue::Set(set)) => {
                let matched = match resolved {
                    toml::Value::Array(arr) => arr
                        .iter()
                        .any(|item| set.contains(&value_to_comparable_string(item))),
                    _ => set.contains(&value_to_comparable_string(resolved)),
                };
                (matched, None)
            }

            // ── Regex matching ────────────────────────────────────────────
            (CompareOp::Matches, PropertyValue::Regex(pattern)) => {
                match (regex::Regex::new(pattern), resolved.as_str()) {
                    (Ok(re), Some(s)) => (re.is_match(s), None),
                    (Err(_), _) => (
                        false,
                        Some(ResolveDiagnostic::TypeMismatch {
                            at_segment: 0,
                            expected: "valid regex",
                            found: "invalid pattern",
                        }),
                    ),
                    (_, None) => (false, Some(compare_type_mismatch("String", resolved))),
                }
            }

            // ── Containment ──────────────────────────────────────────────
            (CompareOp::Contains, PropertyValue::String(s)) => match resolved {
                toml::Value::String(v) => (v.contains(s.as_str()), None),
                toml::Value::Array(arr) => (
                    arr.iter().any(|item| item.as_str().is_some_and(|v| v == s)),
                    None,
                ),
                _ => (
                    false,
                    Some(compare_type_mismatch("String or Array", resolved)),
                ),
            },

            // ── Numeric ordering ──────────────────────────────────────────
            (CompareOp::Gt, PropertyValue::Number(n)) => match value_as_f64(resolved) {
                Some(v) => (v > *n, None),
                None => (false, Some(compare_type_mismatch("Number", resolved))),
            },
            (CompareOp::Lt, PropertyValue::Number(n)) => match value_as_f64(resolved) {
                Some(v) => (v < *n, None),
                None => (false, Some(compare_type_mismatch("Number", resolved))),
            },
            (CompareOp::Gte, PropertyValue::Number(n)) => match value_as_f64(resolved) {
                Some(v) => (v >= *n, None),
                None => (false, Some(compare_type_mismatch("Number", resolved))),
            },
            (CompareOp::Lte, PropertyValue::Number(n)) => match value_as_f64(resolved) {
                Some(v) => (v <= *n, None),
                None => (false, Some(compare_type_mismatch("Number", resolved))),
            },

            // ── Exists handled at call site ────────────────────────────
            (CompareOp::Exists, _) => (true, None),

            // ── Op/value type mismatch ──────────────────────────────────
            _ => (
                false,
                Some(ResolveDiagnostic::TypeMismatch {
                    at_segment: 0,
                    expected: "compatible op/value combination",
                    found: toml_type_name(resolved),
                }),
            ),
        };

        PredicateResult {
            matched,
            diagnostics: diag.into_iter().collect(),
        }
    }
}

/// Build a type-mismatch diagnostic for a comparison operation.
fn compare_type_mismatch(expected: &'static str, resolved: &toml::Value) -> ResolveDiagnostic {
    ResolveDiagnostic::TypeMismatch {
        at_segment: 0,
        expected,
        found: toml_type_name(resolved),
    }
}

/// Extract a numeric value from a TOML value.
fn value_as_f64(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Integer(i) => Some(*i as f64),
        toml::Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// Convert a TOML value to a comparable string representation.
fn value_to_comparable_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        _ => v.to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TraversalSpec
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// TapeFn — tape accessor function (replaces StepInput)
// ═══════════════════════════════════════════════════════════════════════════════

/// Input source for a projection step. Either selects from the tape (Then,
/// Fold, Terminal, Orphan) or produces a fresh BID set from a seed (Bids,
/// Keys, Corpus, DocumentNodes). Seed variants ignore any upstream tape
/// state. See query_model.md §4–5.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TapeFn {
    // ── Tape accessors ───────────────────────────────────────────────
    /// Output BIDs of a single entry.
    /// `None` (default) = previous entry's output (sequential pipeline).
    /// `Some(ref)` = output of the referenced entry.
    Then(Option<StepRef>),

    /// Fold a set operation across a range of entries.
    /// Applies `op` to the output BID sets: e\[a\] op e\[a+1\] op ... op e\[b-1\].
    /// `None` range = all entries (seed ∪ all prior tape BIDs).
    Fold {
        op: SetOp,
        range: Option<(StepRef, StepRef)>,
    },

    /// Boundary/terminal nodes: output BIDs that never appear as input
    /// BIDs within the range. Roots in a Section traversal, leaf nodes
    /// in a downward walk. Internally: union(outputs) \ union(inputs).
    /// `None` range = previous projection step's entries.
    Terminal(Option<(StepRef, StepRef)>),

    /// Orphan nodes: input BIDs that produced no output BIDs within
    /// the range. Nodes with no edges of the traversed kind.
    /// Internally: union(inputs) \ union(outputs).
    /// `None` range = previous projection step's entries.
    Orphan(Option<(StepRef, StepRef)>),

    // ── Seed variants ─────────────────
    /// Explicit resolved BIDs. Ignores upstream tape.
    Bids(Vec<Bid>),

    /// Unresolved node keys. Resolved at evaluation time via the
    /// evaluator's state/path/bref indices.
    Keys(Vec<NodeKey>),

    /// All loaded nodes — the widest-angle starting position.
    Corpus,

    /// All nodes belonging to a document: the document root node plus
    /// every section/heading whose path starts with `doc_path#`.
    DocumentNodes(Bref, String),
}

impl TapeFn {
    /// True if this is a seed variant (Bids, Keys, Corpus, DocumentNodes)
    /// that produces a BID set independently of the tape.
    pub fn is_seed(&self) -> bool {
        matches!(
            self,
            TapeFn::Bids(_) | TapeFn::Keys(_) | TapeFn::Corpus | TapeFn::DocumentNodes(..)
        )
    }
}

impl Default for TapeFn {
    fn default() -> Self {
        Self::Then(None)
    }
}

/// Set operation for `TapeFn::Fold`. See query_model.md §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SetOp {
    Union,
    Intersection,
    LeftDiff,
    RightDiff,
    SymmetricDiff,
}

/// Reference to a tape position. See query_model.md §5.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepRef {
    /// By step label name.
    Label(String),
    /// By absolute tape index.
    Index(usize),
}

// ═══════════════════════════════════════════════════════════════════════════════
// TapeContent + SortPayload
// ═══════════════════════════════════════════════════════════════════════════════

/// The content recorded in a single tape entry. See query_model.md §6.1.
#[derive(Debug, Clone)]
pub enum TapeContent {
    /// Traversal hop: edge indices into the package graph plus the output
    /// BIDs discovered at this hop. `edges` are ordered by `WEIGHT_SORT_KEY`.
    /// `output_bids` stores the resolved output endpoints so the tape is
    /// self-contained for BID extraction (no graph needed).
    Edges {
        edges: Vec<EdgeIndex>,
        output_bids: Vec<Bid>,
    },

    /// Filter: BIDs that survived the predicate.
    /// Order inherited from input.
    Nodes(Vec<Bid>),

    /// Compose result: set operation applied to branch outputs.
    /// `left`/`right` are entry ranges for each branch.
    Compose {
        op: CompositionOp,
        left: Range<usize>,
        right: Range<usize>,
        result: Vec<Bid>,
        /// BIDs present in both left and right branches. Combined with
        /// the branch ranges, this enables diff rendering:
        /// `left_unique = fold_bids(left) - intersection`,
        /// `right_unique = fold_bids(right) - intersection`.
        intersection: Vec<Bid>,
    },

    /// Corpus-wide seed: all loaded nodes are implicitly in scope.
    /// Zero allocation — the set is not enumerable from the tape.
    /// Consumers that need actual BIDs read from subsequent entries.
    Corpus,
}

impl TapeContent {
    /// Extract output BIDs from this content entry.
    /// All variants carry their output BIDs directly, so no graph access
    /// is needed.
    pub fn output_bids(&self) -> Vec<Bid> {
        match self {
            TapeContent::Nodes(bids) => bids.clone(),
            TapeContent::Compose { result, .. } => result.clone(),
            TapeContent::Edges { output_bids, .. } => output_bids.clone(),
            TapeContent::Corpus => vec![],
        }
    }

    /// Check if a BID is present in this content's output.
    pub fn contains_bid(&self, bid: &Bid) -> bool {
        match self {
            TapeContent::Nodes(bids) => bids.contains(bid),
            TapeContent::Compose { result, .. } => result.contains(bid),
            TapeContent::Edges { output_bids, .. } => output_bids.contains(bid),
            TapeContent::Corpus => false,
        }
    }

    /// Number of output BIDs. Returns 0 for `Corpus` (not enumerable).
    pub fn len(&self) -> usize {
        match self {
            TapeContent::Nodes(bids) => bids.len(),
            TapeContent::Compose { result, .. } => result.len(),
            TapeContent::Edges { output_bids, .. } => output_bids.len(),
            TapeContent::Corpus => 0,
        }
    }

    /// Borrow the edge indices, if this is an `Edges` entry.
    pub fn edges(&self) -> Option<&[EdgeIndex]> {
        match self {
            TapeContent::Edges { edges, .. } => Some(edges),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Per-element score payload for tape entries that carry ranking data
/// (e.g. TF-IDF search results). See query_model.md §6.1.
#[derive(Debug, Clone)]
pub struct SortPayload {
    /// Scalar score (TF-IDF, decay, boosting). None = hard inclusion.
    pub score: Option<f32>,
}

/// Typed payload carried by a [`TapeEntry`]. Most entries have no
/// payload (`None` on the field). The enum lets different entry kinds
/// carry their natural data shape without forcing everything into a
/// single struct.
#[derive(Debug, Clone)]
pub enum TapePayload {
    /// Per-BID ranking scores. Positionally aligned with the entry's
    /// `output_bids`.
    Scores(Vec<SortPayload>),
    /// Anchor resolution map for seed entries whose TapeFn was
    /// `TapeFn::Keys`. Each element is `(key_index, bid)` recording
    /// which original key resolved to which BID. Keys that didn't
    /// resolve are absent.
    AnchorMap(Vec<(usize, Bid)>),
}

/// The traversal finds relations where the input node occupies one of the
/// `input_roles`, the relation's weight kind is in `kind_filter`, and then
/// resolves the `output_roles` to produce the next node set.
///
/// ## Graph direction convention
///
/// Section edges flow **source → sink** where source is the
/// more-discrete node (leaf, section, dependency) and sink is
/// the more-aggregate node (trunk, network root, consumer).
///
/// - **Toward roots**: input=Source, output=Sink
///   (`s-section-k`) — "I am a dependency, give me my consumers"
/// - **Toward leaves**: input=Sink, output=Source
///   (`k-section-s`) — "I am a consumer, give me my dependencies"
///
/// Every named helper (`halo`, `balance_map`, `roots`, `leaves`)
/// must have a unit test against a small fixture graph that
/// validates the output BID set matches the intended semantic.
/// Do not trust abstract role reasoning alone — the source/sink
/// direction is easy to get backwards.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraversalSpec {
    /// Which roles the input node must occupy on matched relations.
    pub input_roles: EnumSet<Role>,
    /// Which edge kinds to traverse (OR filter).
    pub kind_filter: EnumSet<WeightKind>,
    /// Which roles to resolve output nodes from.
    pub output_roles: EnumSet<Role>,
    /// How to iterate: count-based or guided with per-hop edge filters.
    pub depth: TraversalDepth,
    /// When true, invert the traversal into an existence filter: return
    /// the subset of *input* nodes that produced NO output, instead of
    /// returning the output nodes. Written as `!uses(1)`, `!composed_of(1)`,
    /// or `!k-pragmatic-s(1)` in the surface syntax.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inverted: bool,
}

impl TraversalSpec {
    /// The halo traversal: full neighbor glob `sko-[*]-sko {1}`.
    ///
    /// 1-hop, all edge kinds, all roles (Source, Sink, Owner).
    /// Collects immediate neighbors of the input set — including nodes
    /// connected via `WEIGHT_OWNED_BY` ownership. Used by balanced queries
    /// to provide edge-endpoint and ownership context for result nodes.
    pub fn halo() -> Self {
        Self {
            input_roles: Role::Source | Role::Sink | Role::Owner,
            kind_filter: EnumSet::all(),
            output_roles: Role::Source | Role::Sink | Role::Owner,
            depth: TraversalDepth::count(1),
            inverted: false,
        }
    }

    /// Balance map: chase Section edges toward roots, collecting all
    /// ancestors up to and including network roots.
    ///
    /// The root-ward complement of `leaves()` (which walks toward
    /// leaves). Both collect all intermediates.
    ///
    /// Walks `Source → Sink` along Section edges until exhaustion.
    /// Returns **all** intermediate ancestors, not just roots —
    /// this satisfies the `is_balanced` invariant (Section ancestor
    /// chain complete to root) for balanced graph materialization.
    ///
    /// To collect **only** root nodes (no intermediates), follow
    /// with a `TapeFn::Terminal` input step — i.e.
    /// `s-section-k(*)` then `TERMINAL`.
    pub fn balance_map() -> Self {
        Self {
            input_roles: Role::Source.into(),
            kind_filter: WeightKind::Section.into(),
            output_roles: Role::Sink.into(),
            depth: TraversalDepth {
                count: DepthCount::Max,
                edge_filter: None,
            },
            inverted: false,
        }
    }

    /// Leaf map: chase Section edges toward leaves, collecting all
    /// section-sources reachable from the seed (the seed's full leaf-ward
    /// subtree — e.g. every asset/href node registered under a namespace
    /// hub, or every section under a document).
    ///
    /// The leaf-ward complement of `balance_map()` (which walks toward
    /// roots). Both collect all intermediates.
    ///
    /// Walks `Sink → Source` along Section edges until exhaustion. Returns
    /// **all** intermediate sources, not just terminal leaves — this is the
    /// leaf-ward analogue of `balance_map()`'s "all intermediates" behavior.
    ///
    /// To collect **only** leaf nodes (no intermediates), follow with a
    /// `TapeFn::Terminal` input step — see [`leaves()`].
    pub fn leaf_map() -> Self {
        Self {
            input_roles: Role::Sink.into(),
            kind_filter: WeightKind::Section.into(),
            output_roles: Role::Source.into(),
            depth: TraversalDepth {
                count: DepthCount::Max,
                edge_filter: None,
            },
            inverted: false,
        }
    }
}

/// Projection steps to find root nodes: walk Section edges toward
/// roots via `balance_map()`, then select terminal nodes (those
/// never used as traversal input).
pub fn roots() -> Vec<ProjectionStep> {
    vec![
        ProjectionStep::traverse(TraversalSpec::balance_map()),
        ProjectionStep::with_input(
            TapeFn::Terminal(None),
            StepOperation::Filter(pass_all_filter()),
        ),
    ]
}

/// Projection steps to find leaf nodes: walk Section edges toward
/// leaves (from consumers to their dependencies) via `leaf_map()`,
/// then select terminal nodes.
pub fn leaves() -> Vec<ProjectionStep> {
    vec![
        ProjectionStep::traverse(TraversalSpec::leaf_map()),
        ProjectionStep::with_input(
            TapeFn::Terminal(None),
            StepOperation::Filter(pass_all_filter()),
        ),
    ]
}

/// A trivially-true filter for use as a pass-through operation
/// when the `TapeFn` input selector does the real work.
fn pass_all_filter() -> NodeFilter {
    NodeFilter::Predicate(PropertyPredicate {
        path: parse_property_path("kind").unwrap(),
        op: CompareOp::Exists,
        value: PropertyValue::None,
    })
}

/// Controls iteration strategy for a traversal. See query_model.md §5.2.
///
/// `count` specifies how many hops to take. `edge_filter` optionally constrains
/// which edges are followed at each hop by matching against the edge `Weight`
/// payload.
///
/// The two fields are orthogonal and compose freely:
/// - `Count(3)` alone follows any matching edge for 3 hops.
/// - `edge_filter` alone implies `Count(1)`.
/// - Together, the filter applies at every hop for the specified count.
/// - `Count(Max) + edge_filter` = "chase edges matching the filter until
///   exhaustion or `MAX_TRAVERSAL`."
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraversalDepth {
    /// How many hops to iterate.
    pub count: DepthCount,
    /// Optional per-hop edge property filter.
    pub edge_filter: Option<EdgePredicate>,
}

impl TraversalDepth {
    /// Convenience constructor for a simple counted traversal (no edge filter).
    pub fn count(n: u8) -> Self {
        Self {
            count: DepthCount::N(n),
            edge_filter: None,
        }
    }

    /// Return the effective maximum hop count, clamped to `MAX_TRAVERSAL`.
    pub fn max_hops(&self) -> u8 {
        match self.count {
            DepthCount::N(n) => n.min(crate::query::MAX_TRAVERSAL),
            DepthCount::Max => crate::query::MAX_TRAVERSAL,
        }
    }
}

impl From<u8> for TraversalDepth {
    fn from(n: u8) -> Self {
        Self::count(n)
    }
}

/// How many hops a traversal should take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DepthCount {
    /// Fixed iteration count.
    N(u8),
    /// Unbounded — iterate until exhaustion or `MAX_TRAVERSAL`.
    Max,
}

/// A predicate over edge `Weight` payload properties, evaluated at each
/// traversal hop. Reuses the same `PropertyPath` / `CompareOp` /
/// `PropertyValue` types as `PropertyPredicate` but resolves against the
/// edge weight payload table rather than the `BeliefNode` TOML representation.
///
/// See query_model.md §5.2 "Path sugar" for how filesystem-style paths
/// desugar into sequences of `EdgePredicate`s.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgePredicate {
    /// Dotted key into the edge `Weight.payload` table.
    pub path: PropertyPath,
    /// Comparison operator.
    pub op: CompareOp,
    /// Comparison operand.
    pub value: PropertyValue,
}

impl EdgePredicate {
    /// Evaluate this predicate against a [`Weight`] payload.
    ///
    /// Resolves `self.path` within `weight.payload` (a TOML table) and
    /// applies the comparison. Returns `true` if the predicate matches.
    /// Resolution diagnostics are logged but do not cause errors — a
    /// missing key simply means "no match."
    pub fn matches_weight(&self, weight: &Weight) -> bool {
        let resolved = resolve_in_table(&weight.payload, &self.path, 0);
        for diag in &resolved.diagnostics {
            tracing::trace!("EdgePredicate resolution: {diag}");
        }
        match self.op {
            CompareOp::Exists => !resolved.values.is_empty(),
            _ => {
                // Reuse PropertyPredicate's comparison logic by constructing
                // a temporary predicate with the same op and value.
                let pred = PropertyPredicate {
                    path: self.path.clone(),
                    op: self.op,
                    value: self.value.clone(),
                };
                resolved
                    .values
                    .iter()
                    .any(|v| pred.compare_value(v).matched)
            }
        }
    }
}

/// Roles a node can occupy in a relation.
#[derive(Debug, Serialize, Deserialize, PartialOrd, Ord, Hash, EnumSetType)]
#[enumset(repr = "u8")]
pub enum Role {
    Source,
    Sink,
    Owner,
}

// ═══════════════════════════════════════════════════════════════════════════════
// SortSpec
// ═══════════════════════════════════════════════════════════════════════════════

/// Display ordering specification for query results. See query_model.md §7.2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum SortSpec {
    /// Sort by section sort key sequence (position-based).
    #[default]
    SectionOrder,
    /// Sort descending by TF-IDF score.
    TfIdfScore,
    /// Sort ascending by traversal step count from anchor.
    PathLength,
    /// Sort descending by number of contributing path_info records.
    IntersectionCardinality,
    /// Weighted linear combination of sort functions.
    Composite(Vec<(SortSpec, f32)>),
}

impl fmt::Display for SortSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SortSpec::SectionOrder => write!(f, "section_order"),
            SortSpec::TfIdfScore => write!(f, "tfidf"),
            SortSpec::PathLength => write!(f, "path_length"),
            SortSpec::IntersectionCardinality => write!(f, "intersection_cardinality"),
            SortSpec::Composite(parts) => {
                let parts_str: Vec<String> = parts
                    .iter()
                    .map(|(spec, weight)| format!("{spec}:{weight}"))
                    .collect();
                write!(f, "{}", parts_str.join(","))
            }
        }
    }
}

/// Parse error for [`SortSpec`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid sort spec '{input}': {message}")]
pub struct SortSpecParseError {
    pub input: String,
    pub message: String,
}

impl FromStr for SortSpec {
    type Err = SortSpecParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().to_lowercase();
        match s.as_str() {
            "section_order" => Ok(SortSpec::SectionOrder),
            "tfidf" => Ok(SortSpec::TfIdfScore),
            "path_length" => Ok(SortSpec::PathLength),
            "intersection_cardinality" => Ok(SortSpec::IntersectionCardinality),
            composite if composite.contains(',') || composite.contains(':') => {
                parse_composite_sort(composite)
            }
            other => Err(SortSpecParseError {
                input: other.to_string(),
                message: "unknown sort function; expected one of: section_order, tfidf, \
                          path_length, intersection_cardinality, or a composite like \
                          'tfidf:0.7,path_length:0.3'"
                    .to_string(),
            }),
        }
    }
}

/// Parse a composite sort spec like `"tfidf:0.7,path_length:0.3"`.
fn parse_composite_sort(input: &str) -> Result<SortSpec, SortSpecParseError> {
    let mut parts = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if let Some(colon_pos) = part.rfind(':') {
            let name = &part[..colon_pos];
            let weight_str = &part[colon_pos + 1..];
            let weight: f32 = weight_str.parse().map_err(|_| SortSpecParseError {
                input: input.to_string(),
                message: format!("invalid weight '{weight_str}' in composite sort"),
            })?;
            let sort = SortSpec::from_str(name)?;
            // Prevent recursive Composite inside Composite
            if matches!(sort, SortSpec::Composite(_)) {
                return Err(SortSpecParseError {
                    input: input.to_string(),
                    message: "nested composite sorts are not supported".to_string(),
                });
            }
            parts.push((sort, weight));
        } else {
            return Err(SortSpecParseError {
                input: input.to_string(),
                message: format!(
                    "composite sort part '{part}' missing weight (expected 'name:weight')"
                ),
            });
        }
    }
    if parts.is_empty() {
        return Err(SortSpecParseError {
            input: input.to_string(),
            message: "empty composite sort".to_string(),
        });
    }
    Ok(SortSpec::Composite(parts))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Score algebra
// ═══════════════════════════════════════════════════════════════════════════════

/// Query result score. `None` means excluded; `Some(1.0)` is hard inclusion;
/// `Some(s)` where s ∈ (0, 1) is soft inclusion. Forms a semiring under
/// (min, max).
pub type Score = Option<f32>;

/// Combine scores with And semantics (intersection): `min` of present scores.
/// If either is `None`, the result is `None`.
pub fn score_and(a: Score, b: Score) -> Score {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        _ => None,
    }
}

/// Combine scores with Or semantics (union): `max` of present scores.
/// `None` is the identity element.
pub fn score_or(a: Score, b: Score) -> Score {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Combine scores with Difference semantics: left score if right is `None`,
/// else `None`.
pub fn score_difference(left: Score, right: Score) -> Score {
    match (left, right) {
        (Some(l), None) => Some(l),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tape — per-step intermediate results (query_model.md §6)
// ═══════════════════════════════════════════════════════════════════════════════

/// Well-known label for the halo step appended by `QueryPackage::append_graph_context`.
pub const GRAPH_CONTEXT_HALO_LABEL: &str = "__halo";
/// Well-known label for the balance-map step appended by `QueryPackage::append_graph_context`.
pub const GRAPH_CONTEXT_BALANCE_LABEL: &str = "__balance";
/// Well-known label for the leaf-map step appended by `QueryPackage::leaf_anchored`.
pub const GRAPH_CONTEXT_LEAF_LABEL: &str = "__leaf";

/// The tape: ordered record of intermediate results at each projection step.
/// This is the sole interface between projection and downstream consumers.
/// See query_model.md §6.
#[derive(Debug, Clone, Default)]
pub struct Tape {
    pub steps: Vec<TapeEntry>,
}

/// A single entry in the tape. Multi-hop traversals produce multiple entries
/// sharing the same label; compose steps record branch ranges.
#[derive(Debug, Clone)]
pub struct TapeEntry {
    /// Step label. Multi-hop traversals share the same label.
    pub label: String,
    /// The content recorded by this entry.
    pub content: TapeContent,
    /// Optional typed payload. Most entries: `None`.
    pub payload: Option<TapePayload>,
}

impl Tape {
    /// For each node in the final result, return which tape entry indices
    /// contained that node. For `Edges` entries without a graph, returns
    /// false (acceptable — provenance tracking operates on Compose/Nodes).
    pub fn node_steps(&self, bid: &Bid) -> Vec<usize> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.content.contains_bid(bid))
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Map each BID to the first tape entry index where it appeared as output.
    pub fn bid_tape_indices(&self) -> BTreeMap<Bid, usize> {
        let mut map = BTreeMap::new();
        for (idx, entry) in self.steps.iter().enumerate() {
            for bid in entry.content.output_bids() {
                map.entry(bid).or_insert(idx);
            }
        }
        map
    }

    /// Check if the tape has any composition entries.
    pub fn has_composition(&self) -> bool {
        self.steps
            .iter()
            .any(|e| matches!(e.content, TapeContent::Compose { .. }))
    }

    /// Get the number of entries in this tape.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Returns `true` if the tape has no entries.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Derive per-node provenance from composition tape entries.
    ///
    /// Uses the branch ranges on the compose entry to find the left
    /// and right branch results. Returns `"Left"`, `"Right"`, `"Both"`,
    /// or `""` for each node.
    pub fn provenance_label(&self, bid: &Bid, op: CompositionOp) -> String {
        let Some(compose) = self.find_compose() else {
            return String::new();
        };
        let (left_range, right_range) = match compose {
            TapeContent::Compose { left, right, .. } => (left.clone(), right.clone()),
            _ => return String::new(),
        };
        // Check last entry in each branch range
        let in_left = left_range.end > left_range.start
            && self.steps[left_range.end - 1].content.contains_bid(bid);
        let in_right = right_range.end > right_range.start
            && self.steps[right_range.end - 1].content.contains_bid(bid);
        match (in_left, in_right) {
            (true, true) => "Both".to_string(),
            (true, false) => match op {
                CompositionOp::Not => "\u{2713}".to_string(),
                _ => "Left".to_string(),
            },
            (false, true) => match op {
                CompositionOp::Not => "\u{2014}".to_string(),
                _ => "Right".to_string(),
            },
            (false, false) => String::new(),
        }
    }

    /// Return the composition operator from the last Compose entry, if any.
    pub fn composition_op(&self) -> Option<CompositionOp> {
        self.find_compose().and_then(|c| {
            if let TapeContent::Compose { op, .. } = c {
                Some(*op)
            } else {
                None
            }
        })
    }

    /// Private helper: find the last Compose content entry.
    fn find_compose(&self) -> Option<&TapeContent> {
        self.steps.iter().rev().find_map(|e| {
            if matches!(e.content, TapeContent::Compose { .. }) {
                Some(&e.content)
            } else {
                None
            }
        })
    }

    /// Return the left-branch entry count (for gap-summary captions).
    /// Returns `None` if there is no composition.
    pub fn left_count(&self) -> Option<usize> {
        let compose = self.find_compose()?;
        if let TapeContent::Compose { left, .. } = compose {
            if left.end > left.start {
                Some(self.steps[left.end - 1].content.len())
            } else {
                Some(0)
            }
        } else {
            None
        }
    }

    /// Return the right-branch entry count (for gap-summary captions).
    /// Returns `None` if there is no composition.
    pub fn right_count(&self) -> Option<usize> {
        let compose = self.find_compose()?;
        if let TapeContent::Compose { right, .. } = compose {
            if right.end > right.start {
                Some(self.steps[right.end - 1].content.len())
            } else {
                Some(0)
            }
        } else {
            None
        }
    }

    /// Return the intersection BIDs from the last Compose entry, if any.
    pub fn compose_intersection(&self) -> Option<&[Bid]> {
        self.find_compose().and_then(|c| {
            if let TapeContent::Compose { intersection, .. } = c {
                Some(intersection.as_slice())
            } else {
                None
            }
        })
    }

    /// Fold BIDs across a range of tape entries. Returns the union of all
    /// BID sets in `self.steps[range]`.
    ///
    /// Use `..` for all entries, `..n` for the first n, `n..` for entries
    /// from n onward, or `n..m` for a specific slice.
    pub fn fold_bids(&self, range: Range<usize>) -> BTreeSet<Bid> {
        let end = range.end.min(self.steps.len());
        let start = range.start.min(end);
        self.steps[start..end]
            .iter()
            .flat_map(|entry| entry.content.output_bids())
            .collect()
    }

    /// Union of all BIDs that appeared at any point in the projection chain.
    /// This is the "primary" set for graph Trace coloring.
    pub fn cumulative_bids(&self) -> BTreeSet<Bid> {
        self.fold_bids(0..self.steps.len())
    }

    /// Find the tape index where graph context steps (halo, ancestry) begin.
    ///
    /// Searches backwards from the end for an entry with the halo label.
    /// Returns the index of the halo entry if found, or `self.len()` if no
    /// graph context was appended (meaning all tape entries are user
    /// projection steps).
    pub fn graph_context_boundary(&self) -> usize {
        for (i, entry) in self.steps.iter().enumerate().rev() {
            if entry.label == GRAPH_CONTEXT_HALO_LABEL {
                return i;
            }
        }
        self.steps.len()
    }

    /// Extract the result BID set using a result lens.
    ///
    /// The lens is a `TapeFn` that determines which BIDs from the tape
    /// constitute "the result" for display:
    ///
    /// - `Then(None)` — final frontier: last user-step entry's output.
    ///   For compositions, this is the compose step's merged result.
    /// - `Fold(Union, None)` — full tree: union of all user-step entries.
    ///   This is the default, showing all hops of multi-hop traversals.
    /// - Label/Index reference — a specific step's output.
    ///
    /// The `seed` parameter provides fallback BIDs when the tape is empty.
    pub fn result_bids(&self, lens: &TapeFn, seed: &BTreeSet<Bid>) -> BTreeSet<Bid> {
        let boundary = self.graph_context_boundary();
        if boundary == 0 {
            return seed.clone();
        }
        match lens {
            TapeFn::Then(None) => {
                // Last user-step entry's output (compose result or final hop).
                self.steps[boundary - 1]
                    .content
                    .output_bids()
                    .into_iter()
                    .collect()
            }
            TapeFn::Fold {
                op: SetOp::Union,
                range: None,
            } => {
                // Union of all user-step entries.
                self.fold_bids(0..boundary)
            }
            _ => {
                // For other TapeFn variants, delegate to eval.
                self.eval(lens, seed, None)
            }
        }
    }

    /// Get a tape entry by index.
    pub fn get(&self, idx: usize) -> Option<&TapeEntry> {
        self.steps.get(idx)
    }

    /// All entries sharing a label name, in tape order.
    pub fn entries_for<'a>(
        &'a self,
        label: &'a str,
    ) -> impl Iterator<Item = (usize, &'a TapeEntry)> + 'a {
        self.steps
            .iter()
            .enumerate()
            .filter(move |(_, e)| e.label == label)
    }

    /// Last non-empty entry for a label. Returns the index and entry.
    pub fn last_entry_for(&self, label: &str) -> Option<(usize, &TapeEntry)> {
        self.steps
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| e.label == label && !e.content.is_empty())
    }

    /// Output BIDs for a single entry.
    pub fn output_bids(&self, idx: usize) -> Vec<Bid> {
        self.steps
            .get(idx)
            .map(|e| e.content.output_bids())
            .unwrap_or_default()
    }

    /// Input BIDs for a single entry. Derived from context:
    /// - First entry in its label group: previous entry's output, or seed if idx==0
    /// - Subsequent entry: previous entry's output
    ///
    /// `seed` is the original seed BIDs (needed if idx==0).
    pub fn input_bids(&self, idx: usize, seed: &BTreeSet<Bid>) -> BTreeSet<Bid> {
        if idx == 0 {
            return seed.clone();
        }
        self.steps
            .get(idx - 1)
            .map(|e| e.content.output_bids().into_iter().collect())
            .unwrap_or_default()
    }

    /// Evaluate a `TapeFn` against this tape. General-purpose method for
    /// post-evaluation tape queries by consumers. Uses the seed BIDs
    /// from the package spec.
    ///
    /// For `Terminal` and `Orphan`, `prev_label` identifies which step's
    /// entries to operate over. Pass `None` to use the last label in the tape.
    pub fn eval(
        &self,
        f: &TapeFn,
        seed: &BTreeSet<Bid>,
        prev_label: Option<&str>,
    ) -> BTreeSet<Bid> {
        // For Then/Fold, chain = last entry's output or seed
        let chain: BTreeSet<Bid> = self
            .steps
            .last()
            .map(|e| e.content.output_bids().into_iter().collect())
            .unwrap_or_else(|| seed.clone());
        // If no prev_label, derive from last entry
        let derived_label: Option<String> = if prev_label.is_none() {
            self.steps.last().map(|e| e.label.clone())
        } else {
            None
        };
        let effective_label = prev_label.or(derived_label.as_deref());
        self.eval_input(f, seed, &chain, effective_label)
    }

    /// Evaluate a `TapeFn` to produce an input BID set.
    ///
    /// - `seed`: the original seed BIDs
    /// - `chain`: the output of the previous step (or seed if first step)
    /// - `prev_label`: the label of the immediately preceding projection step
    ///   (used by `Terminal(None)` and `Orphan(None)` to find the range)
    pub fn eval_input(
        &self,
        tap_fn: &TapeFn,
        seed: &BTreeSet<Bid>,
        chain: &BTreeSet<Bid>,
        prev_label: Option<&str>,
    ) -> BTreeSet<Bid> {
        match tap_fn {
            TapeFn::Then(None) => chain.clone(),
            TapeFn::Then(Some(_step_ref)) => {
                // TODO: resolve StepRef to tape index/label
                chain.clone()
            }
            TapeFn::Fold {
                op: SetOp::Union,
                range: None,
            } => {
                let mut cumulative = seed.clone();
                cumulative.extend(self.cumulative_bids());
                cumulative
            }
            TapeFn::Fold { op, range } => {
                match range {
                    None => {
                        // No range: union of seed + all prior entries (already handled above for Union).
                        // For other ops, this doesn't make sense on a flat fold; return chain.
                        chain.clone()
                    }
                    Some((start_ref, end_ref)) => {
                        let start_idx = self.resolve_step_ref(start_ref);
                        let end_idx = self.resolve_step_ref(end_ref);
                        match (start_idx, end_idx) {
                            (Some(s), Some(e)) => {
                                let left = self.fold_bids(s..s + 1);
                                let right = self.fold_bids(e..e + 1);
                                // Optimization: if there's a Compose entry whose branch
                                // ranges cover s and e, reuse its stored intersection.
                                if *op == SetOp::Intersection {
                                    if let Some(intersection) =
                                        self.find_compose_intersection_for(s, e)
                                    {
                                        return intersection;
                                    }
                                }
                                match op {
                                    SetOp::Union => left.union(&right).copied().collect(),
                                    SetOp::Intersection => {
                                        left.intersection(&right).copied().collect()
                                    }
                                    SetOp::LeftDiff => left.difference(&right).copied().collect(),
                                    SetOp::RightDiff => right.difference(&left).copied().collect(),
                                    SetOp::SymmetricDiff => {
                                        left.symmetric_difference(&right).copied().collect()
                                    }
                                }
                            }
                            _ => chain.clone(),
                        }
                    }
                }
            }
            TapeFn::Terminal(range) => self.eval_terminal(range, seed, prev_label),
            TapeFn::Orphan(range) => self.eval_orphan(range, seed, prev_label),
            // Seed variants: these are resolved by the evaluator before
            // reaching this method. If we encounter them here, return the
            // seed as a fallback.
            TapeFn::Bids(bids) => bids.iter().copied().collect(),
            TapeFn::Keys(_) | TapeFn::Corpus | TapeFn::DocumentNodes(..) => seed.clone(),
        }
    }

    /// Compute terminal nodes: `union(outputs) \ union(inputs)` across
    /// a range of tape entries. Nodes that appear as output but never as
    /// input are "terminal" — roots in an upstream walk, leaves in a
    /// downstream walk.
    ///
    /// `None` range = all entries sharing the previous step's label.
    fn eval_terminal(
        &self,
        range: &Option<(StepRef, StepRef)>,
        seed: &BTreeSet<Bid>,
        prev_label: Option<&str>,
    ) -> BTreeSet<Bid> {
        let entries = self.resolve_range(range, prev_label);
        if entries.is_empty() {
            return BTreeSet::new();
        }

        let mut all_outputs: BTreeSet<Bid> = BTreeSet::new();
        let mut all_inputs: BTreeSet<Bid> = BTreeSet::new();

        // The input BIDs of the first entry in the range are derived
        // from the entry before it (or the seed if it's the first tape
        // entry). For subsequent entries, inputs = previous entry's outputs.
        let first_idx = entries[0];
        if first_idx > 0 {
            all_inputs.extend(self.steps[first_idx - 1].content.output_bids());
        } else {
            all_inputs.extend(seed.iter());
        }

        for (i, &idx) in entries.iter().enumerate() {
            let entry = &self.steps[idx];
            let out_bids = entry.content.output_bids();
            all_outputs.extend(out_bids.iter());
            // For entries after the first, the input is the previous
            // entry's output.
            if i > 0 {
                let prev_entry = &self.steps[entries[i - 1]];
                all_inputs.extend(prev_entry.content.output_bids());
            }
        }

        // Terminal = outputs \ inputs
        all_outputs.difference(&all_inputs).copied().collect()
    }

    /// Compute orphan nodes: `union(inputs) \ union(outputs)` across
    /// a range of tape entries. Nodes that appear as input but never
    /// produce output — disconnected from the traversed edge kind.
    fn eval_orphan(
        &self,
        range: &Option<(StepRef, StepRef)>,
        seed: &BTreeSet<Bid>,
        prev_label: Option<&str>,
    ) -> BTreeSet<Bid> {
        let entries = self.resolve_range(range, prev_label);
        if entries.is_empty() {
            return BTreeSet::new();
        }

        let mut all_outputs: BTreeSet<Bid> = BTreeSet::new();
        let mut all_inputs: BTreeSet<Bid> = BTreeSet::new();

        let first_idx = entries[0];
        if first_idx > 0 {
            all_inputs.extend(self.steps[first_idx - 1].content.output_bids());
        } else {
            all_inputs.extend(seed.iter());
        }

        for (i, &idx) in entries.iter().enumerate() {
            let entry = &self.steps[idx];
            all_outputs.extend(entry.content.output_bids());
            if i > 0 {
                let prev_entry = &self.steps[entries[i - 1]];
                all_inputs.extend(prev_entry.content.output_bids());
            }
        }

        // Orphan = inputs \ outputs
        all_inputs.difference(&all_outputs).copied().collect()
    }

    /// Check if indices `a` and `b` correspond to the left and right
    /// branch entries of a Compose tape entry. If so, return the stored
    /// intersection (avoiding recomputation).
    fn find_compose_intersection_for(&self, a: usize, b: usize) -> Option<BTreeSet<Bid>> {
        for entry in self.steps.iter().rev() {
            if let TapeContent::Compose {
                left,
                right,
                intersection,
                ..
            } = &entry.content
            {
                // Check if a is in the left range and b is in the right range
                if left.contains(&a) && right.contains(&b) {
                    return Some(intersection.iter().copied().collect());
                }
            }
        }
        None
    }

    /// Resolve a range specification to a vector of tape entry indices.
    /// `None` = all entries sharing `prev_label`.
    fn resolve_range(
        &self,
        range: &Option<(StepRef, StepRef)>,
        prev_label: Option<&str>,
    ) -> Vec<usize> {
        match range {
            None => {
                // All entries from the previous step's label.
                let Some(label) = prev_label else {
                    return vec![];
                };
                self.entries_for(label).map(|(idx, _)| idx).collect()
            }
            Some((start_ref, end_ref)) => {
                let start = self.resolve_step_ref(start_ref);
                let end = self.resolve_step_ref(end_ref);
                match (start, end) {
                    (Some(s), Some(e)) => (s..e).collect(),
                    _ => vec![],
                }
            }
        }
    }

    /// Resolve a `StepRef` to a tape index.
    fn resolve_step_ref(&self, step_ref: &StepRef) -> Option<usize> {
        match step_ref {
            StepRef::Index(idx) => {
                if *idx < self.steps.len() {
                    Some(*idx)
                } else {
                    None
                }
            }
            StepRef::Label(label) => {
                // First entry with this label.
                self.entries_for(label).next().map(|(idx, _)| idx)
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// QueryPackage — lifecycle object for query evaluation
// ═════════════════════════════════════════════════════════════════════════════

/// The lifecycle stage of a [`QueryPackage`].
///
/// Stages advance monotonically — an evaluator can only move forward or stay
/// at the current stage, never regress. This lets downstream code inspect
/// where evaluation left off and decide whether to resume or act on the
/// current state.
///
/// ```text
/// Constructed → Anchored → Projecting → Projected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackageStage {
    /// Initial state. The spec is set but nothing has been evaluated.
    /// The first step's TapeFn may be a seed (Keys, Corpus, etc.) or
    /// a default Then(None) awaiting context injection.
    Constructed,
    /// Seed has been resolved to `TapeFn::Bids` on the first step.
    /// Seed BIDs are known.
    Anchored,
    /// Projection steps are being applied. The tape is partially populated
    /// (at least one entry, but not all steps complete).
    Projecting,
    /// All projection steps are complete. The tape is fully populated.
    /// This is the terminal stage.
    Projected,
}

impl fmt::Display for PackageStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageStage::Constructed => write!(f, "Constructed"),
            PackageStage::Anchored => write!(f, "Anchored"),
            PackageStage::Projecting => write!(f, "Projecting"),
            PackageStage::Projected => write!(f, "Projected"),
        }
    }
}

/// A lifecycle object that carries a query from construction through evaluation
/// to graph production. The evaluator receives a `&mut QueryPackage` and
/// populates it progressively: resolving the seed, running pipeline steps,
/// and finally materializing a `BeliefGraph`.
///
/// The lifecycle stage is **derived** from the package's internal state, not
/// maintained as a separate field. See [`QueryPackage::stage`] and
/// [`PackageStage`] for the state machine.
///
/// See Issue 83 §Architecture for the design rationale.
#[derive(Debug, Clone)]
pub struct QueryPackage {
    /// The user's original intent — never mutated after construction.
    original_spec: QuerySpec,
    /// The effective spec — evaluators may append halo/ancestry steps,
    /// rewrite subjects, apply PathMap acceleration, etc. Starts as a
    /// clone of `original_spec`.
    spec: QuerySpec,
    /// The tape — populated progressively by evaluators. Each entry
    /// records a projection step and its output BID set.
    tape: Tape,
    /// The materialized graph — populated by evaluators after projection
    /// steps are complete.
    graph: Option<BeliefGraph>,
    /// Pending anchor map from seed resolution. Consumed by the evaluator
    /// and attached to the first step's tape entry as `TapePayload::AnchorMap`.
    pending_anchor_map: Option<Vec<(usize, Bid)>>,
}

impl QueryPackage {
    /// Construct a new package from a `QuerySpec`. The effective spec starts
    /// as a clone of the original; the tape is empty; no output is produced.
    pub fn new(spec: QuerySpec) -> Self {
        Self {
            original_spec: spec.clone(),
            spec,
            tape: Tape::default(),
            graph: None,
            pending_anchor_map: None,
        }
    }

    /// Construct a balanced package — appends halo + section-root traversal
    /// steps so that after evaluation the package graph contains a
    /// self-contained graph with ancestor chains and edge-endpoint context.
    ///
    /// Use this when the consumer needs edge data (graph rendering,
    /// `to_event_stream`, node context lookups). For BID-set-only queries
    /// (table views, search results), use `QueryPackage::new(spec)` instead.
    pub fn balanced(spec: QuerySpec) -> Self {
        let mut package = Self::new(spec);
        Self::append_graph_context(&mut package);
        package
    }

    /// Construct an anchored package — appends section-root traversal
    /// (balance_map) WITHOUT the halo step.
    ///
    /// The result graph contains the seed nodes plus their full Section
    /// ancestor chain to the root, providing PathMap context for path
    /// resolution. Unlike `balanced()`, it does NOT include 1-hop
    /// neighbors (the halo), avoiding the O(N) explosion when the seed
    /// is a hub node (e.g., const-namespace roots with thousands of
    /// Section children).
    ///
    /// Use this when the consumer needs Section ancestry for PathMap
    /// anchoring but does not need edge-endpoint context.
    pub fn anchored(spec: QuerySpec) -> Self {
        let mut package = Self::new(spec);
        let cumulative_input = TapeFn::Fold {
            op: SetOp::Union,
            range: None,
        };
        let mut balance_step = ProjectionStep::with_input(
            cumulative_input,
            StepOperation::Traverse(TraversalSpec::balance_map()),
        );
        balance_step.label = GRAPH_CONTEXT_BALANCE_LABEL.to_string();
        package.spec_mut().steps.push(balance_step);
        package
    }

    /// Construct a leaf-anchored package — appends a leaf-ward section
    /// traversal (`leaf_map`) WITHOUT the halo step.
    ///
    /// The leaf-ward complement of `anchored()`. The result graph contains
    /// the seed node plus every node reachable by walking Section edges
    /// toward leaves (i.e. everything the seed is a Section-sink for,
    /// directly or transitively) — e.g. every asset/href node registered
    /// under a namespace hub, or every section under a document. Does NOT
    /// include 1-hop neighbors (the halo).
    ///
    /// Use this when the consumer needs "everything under this node" rather
    /// than "everything above this node" (which is what `anchored()` gives).
    pub fn leaf_anchored(spec: QuerySpec) -> Self {
        let mut package = Self::new(spec);
        let cumulative_input = TapeFn::Fold {
            op: SetOp::Union,
            range: None,
        };
        let mut leaf_step = ProjectionStep::with_input(
            cumulative_input,
            StepOperation::Traverse(TraversalSpec::leaf_map()),
        );
        leaf_step.label = GRAPH_CONTEXT_LEAF_LABEL.to_string();
        package.spec_mut().steps.push(leaf_step);
        package
    }

    // ── Stage ──────────────────────────────────────────────────────────

    /// Derive the current lifecycle stage from internal state.
    ///
    /// The stage is determined by inspecting the package's fields:
    /// - Last tape entry label matches last projection step → `Projected`
    /// - Tape has some entries but not all steps → `Projecting`
    /// - Seed resolved to `TapeFn::Bids` → `Anchored`
    /// - Otherwise → `Constructed`
    pub fn stage(&self) -> PackageStage {
        // Check if seed is resolved: first step must have TapeFn::Bids.
        let seed_resolved = self
            .spec
            .steps
            .first()
            .is_none_or(|s| matches!(s.input, TapeFn::Bids(_)));
        if !seed_resolved {
            return PackageStage::Constructed;
        }
        let num_steps = self.spec.steps.len();
        if num_steps == 0 {
            // Empty pipeline with resolved seed is vacuously projected.
            return PackageStage::Projected;
        }
        // A single Identity step with resolved seed is also vacuously projected
        // once the seed entry is in the tape.
        if num_steps == 1 && matches!(self.spec.steps[0].operation, StepOperation::Identity) {
            if !self.tape.is_empty() {
                return PackageStage::Projected;
            }
            return PackageStage::Anchored;
        }
        if self.tape.is_empty() {
            return PackageStage::Anchored;
        }
        // The evaluator processes steps sequentially. If the last tape entry's
        // label matches the last step's effective label, all steps are complete.
        let last_step = &self.spec.steps[num_steps - 1];
        let last_label = if last_step.label.is_empty() {
            // Auto-label: stringified step index
            (num_steps - 1).to_string()
        } else {
            last_step.label.clone()
        };
        if self.tape.steps.last().map(|e| e.label.as_str()) == Some(last_label.as_str()) {
            PackageStage::Projected
        } else {
            PackageStage::Projecting
        }
    }

    // ── Spec access ──────────────────────────────────────────────────────

    /// The original spec as provided by the caller. Never mutated.
    pub fn original_spec(&self) -> &QuerySpec {
        &self.original_spec
    }

    /// The effective spec. Evaluators may have rewritten this (e.g. appended
    /// halo/ancestry steps for balanced queries).
    pub fn spec(&self) -> &QuerySpec {
        &self.spec
    }

    /// Mutable access to the effective spec. Evaluators use this to append
    /// steps or rewrite seed TapeFn values.
    pub fn spec_mut(&mut self) -> &mut QuerySpec {
        &mut self.spec
    }

    // ── Tape access ──────────────────────────────────────────────────────

    /// The tape of intermediate results.
    pub fn tape(&self) -> &Tape {
        &self.tape
    }

    /// Mutable access to the tape. Evaluators push entries during evaluation.
    pub fn tape_mut(&mut self) -> &mut Tape {
        &mut self.tape
    }

    // ── Anchor resolution ───────────────────────────────────────────────────────

    /// Look up the BID that a specific anchor key index resolved to.
    ///
    /// Returns `Some(bid)` if the seed entry carries an `AnchorMap` payload
    /// and `key_index` is present in it. Returns `None` if the seed was
    /// not `Keys`, or the key didn't resolve, or evaluation hasn't run yet.
    pub fn resolved_bid(&self, key_index: usize) -> Option<Bid> {
        let seed = self.tape.steps.first()?;
        match &seed.payload {
            Some(TapePayload::AnchorMap(map)) => map
                .iter()
                .find(|(idx, _)| *idx == key_index)
                .map(|(_, bid)| *bid),
            _ => None,
        }
    }

    /// Return the full anchor resolution map from the seed entry, if present.
    ///
    /// Each element is `(key_index, bid)` from the original `TapeFn::Keys`.
    /// Returns `None` if the seed wasn't `Keys` or no anchors resolved.
    pub fn anchor_map(&self) -> Option<&[(usize, Bid)]> {
        let seed = self.tape.steps.first()?;
        match &seed.payload {
            Some(TapePayload::AnchorMap(map)) => Some(map),
            _ => None,
        }
    }

    // ── Graph access ──────────────────────────────────────────────────────────────

    /// Borrow the graph, if populated.
    pub fn graph(&self) -> Option<&BeliefGraph> {
        self.graph.as_ref()
    }

    /// Mutable access to the graph slot. Evaluators use this to populate
    /// or replace the graph.
    pub fn graph_mut(&mut self) -> &mut Option<BeliefGraph> {
        &mut self.graph
    }

    /// Set the graph. Used by evaluators after projection is complete.
    /// After this call, `stage()` returns `Evaluated`.
    pub fn set_graph(&mut self, graph: BeliefGraph) {
        self.graph = Some(graph);
    }

    /// Store an anchor map from seed resolution. The evaluator calls this
    /// after resolving Keys→Bids; the projection loop consumes it via
    /// `take_anchor_map` and attaches it to the first step's tape entry.
    pub fn set_anchor_map(&mut self, map: Vec<(usize, Bid)>) {
        self.pending_anchor_map = Some(map);
    }

    /// Consume the pending anchor map (if any). Returns `None` after the
    /// first call.
    pub fn take_anchor_map(&mut self) -> Option<Vec<(usize, Bid)>> {
        self.pending_anchor_map.take()
    }

    /// Consume the package and return the graph. Panics if no graph
    /// has been set.
    pub fn into_graph(self) -> BeliefGraph {
        self.graph
            .expect("QueryPackage::into_graph called but no graph has been set")
    }

    /// Append halo and section ancestry steps to the effective spec.
    ///
    /// Idempotent — checks for the halo step before appending.
    /// Both steps use `TapeFn::Fold { op: Union, range: None }` so the
    /// projection loop feeds them `seed ∪ all prior tape BIDs`.
    fn append_graph_context(package: &mut QueryPackage) {
        let halo = TraversalSpec::halo();
        let already_present = package
            .spec()
            .steps
            .iter()
            .any(|step| matches!(step.operation, StepOperation::Traverse(ref t) if *t == halo));
        if already_present {
            return;
        }
        let cumulative_input = TapeFn::Fold {
            op: SetOp::Union,
            range: None,
        };
        let mut halo_step = ProjectionStep::with_input(
            cumulative_input.clone(),
            StepOperation::Traverse(TraversalSpec::halo()),
        );
        halo_step.label = GRAPH_CONTEXT_HALO_LABEL.to_string();
        package.spec_mut().steps.push(halo_step);

        let mut balance_step = ProjectionStep::with_input(
            cumulative_input,
            StepOperation::Traverse(TraversalSpec::balance_map()),
        );
        balance_step.label = GRAPH_CONTEXT_BALANCE_LABEL.to_string();
        package.spec_mut().steps.push(balance_step);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Evaluation helpers
// ═════════════════════════════════════════════════════════════════════════════

/// Build a [`WeightSet`] filter from a set of [`WeightKind`]s.
///
/// Each kind gets a minimal [`Weight`] with an empty payload table. An empty
/// input set returns `None` (meaning "all kinds").
pub fn weight_set_from_kinds(kinds: &EnumSet<WeightKind>) -> Option<WeightSet> {
    if kinds.is_empty() {
        return None;
    }
    let weights: BTreeMap<WeightKind, Weight> = kinds
        .iter()
        .map(|k| {
            (
                k,
                Weight {
                    payload: Table::new(),
                },
            )
        })
        .collect();
    Some(WeightSet { weights })
}

impl From<&NodeKey> for TapeFn {
    fn from(key: &NodeKey) -> Self {
        TapeFn::Keys(vec![key.clone()])
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use crate::beliefbase::{BeliefBase, BeliefGraph, BidGraph};
    use crate::properties::{
        BeliefKind, Bid, NodeId, Weight, WeightKind, WeightSet, WEIGHT_DOC_PATHS, WEIGHT_SORT_KEY,
    };

    // ── PropertySegment::parse ──────────────────────────────────────────────

    #[test]
    fn parse_segment_key() {
        assert_eq!(
            PropertySegment::parse("title").unwrap(),
            PropertySegment::Key("title".into())
        );
    }

    #[test]
    fn parse_segment_index() {
        assert_eq!(
            PropertySegment::parse("3").unwrap(),
            PropertySegment::Index(3)
        );
    }

    #[test]
    fn parse_segment_wildcard() {
        assert_eq!(
            PropertySegment::parse("*").unwrap(),
            PropertySegment::Wildcard
        );
    }

    #[test]
    fn parse_segment_globstar() {
        assert_eq!(
            PropertySegment::parse("**").unwrap(),
            PropertySegment::GlobStar
        );
    }

    #[test]
    fn parse_segment_slice() {
        assert_eq!(
            PropertySegment::parse("1:4").unwrap(),
            PropertySegment::Slice(1, 4)
        );
    }

    #[test]
    fn parse_segment_invalid_slice_is_error() {
        let err = PropertySegment::parse("foo:bar").unwrap_err();
        assert_eq!(err.segment, "foo:bar");
        assert!(
            err.message.contains("not a valid slice"),
            "error should mention invalid slice: {}",
            err.message
        );
    }

    // ── parse_property_path ─────────────────────────────────────────────

    #[test]
    fn parse_dotted_path() {
        let path = parse_property_path("payload.status").unwrap();
        assert_eq!(
            path,
            vec![
                PropertySegment::Key("payload".into()),
                PropertySegment::Key("status".into()),
            ]
        );
    }

    #[test]
    fn parse_path_with_index() {
        let path = parse_property_path("payload.listing.0").unwrap();
        assert_eq!(
            path,
            vec![
                PropertySegment::Key("payload".into()),
                PropertySegment::Key("listing".into()),
                PropertySegment::Index(0),
            ]
        );
    }

    #[test]
    fn parse_path_with_wildcard() {
        let path = parse_property_path("payload.*").unwrap();
        assert_eq!(
            path,
            vec![
                PropertySegment::Key("payload".into()),
                PropertySegment::Wildcard,
            ]
        );
    }

    // ── SortSpec parsing ────────────────────────────────────────────────

    #[test]
    fn sort_spec_simple() {
        assert_eq!(
            "section_order".parse::<SortSpec>().unwrap(),
            SortSpec::SectionOrder
        );
        assert_eq!("tfidf".parse::<SortSpec>().unwrap(), SortSpec::TfIdfScore);
    }

    #[test]
    fn sort_spec_composite() {
        let spec = "tfidf:0.7,path_length:0.3".parse::<SortSpec>().unwrap();
        match spec {
            SortSpec::Composite(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].0, SortSpec::TfIdfScore);
                assert!((parts[0].1 - 0.7).abs() < f32::EPSILON);
                assert_eq!(parts[1].0, SortSpec::PathLength);
                assert!((parts[1].1 - 0.3).abs() < f32::EPSILON);
            }
            other => panic!("expected Composite, got {other:?}"),
        }
    }

    #[test]
    fn sort_spec_unknown_errors() {
        assert!("unknown_sort".parse::<SortSpec>().is_err());
    }

    #[test]
    fn sort_spec_display_roundtrip() {
        let spec = SortSpec::TfIdfScore;
        assert_eq!(spec.to_string().parse::<SortSpec>().unwrap(), spec);
    }

    // ── Score algebra ───────────────────────────────────────────────────

    #[test]
    fn score_and_both_some() {
        assert_eq!(score_and(Some(0.8), Some(0.5)), Some(0.5));
    }

    #[test]
    fn score_and_one_none() {
        assert_eq!(score_and(Some(0.8), None), None);
        assert_eq!(score_and(None, Some(0.5)), None);
    }

    #[test]
    fn score_or_both_some() {
        assert_eq!(score_or(Some(0.3), Some(0.7)), Some(0.7));
    }

    #[test]
    fn score_or_one_none() {
        assert_eq!(score_or(Some(0.3), None), Some(0.3));
        assert_eq!(score_or(None, Some(0.7)), Some(0.7));
    }

    #[test]
    fn score_difference_left_only() {
        assert_eq!(score_difference(Some(0.5), None), Some(0.5));
        assert_eq!(score_difference(Some(0.5), Some(0.3)), None);
        assert_eq!(score_difference(None, None), None);
    }

    // ── Helper: build a test BeliefNode ────────────────────────────────────────

    fn test_node() -> BeliefNode {
        let mut payload = Table::new();
        payload.insert("status".into(), toml::Value::String("open".into()));
        payload.insert("priority".into(), toml::Value::Integer(3));
        payload.insert(
            "listing".into(),
            toml::Value::Array(vec![
                toml::Value::String("alpha".into()),
                toml::Value::String("beta".into()),
                toml::Value::String("gamma".into()),
            ]),
        );

        let mut git = Table::new();
        git.insert("branch".into(), toml::Value::String("main".into()));
        let mut metadata = Table::new();
        metadata.insert("git".into(), toml::Value::Table(git));

        BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: BeliefKind::Document.into(),
            title: "Test Document".to_string(),
            schema: Some("procedure".to_string()),
            payload,
            id: NodeId::Explicit("test-doc".to_string()),
            metadata,
        }
    }

    // ── resolve_property_path tests ─────────────────────────────────────

    #[test]
    fn resolve_fixed_field_title() {
        let node = test_node();
        let path = parse_property_path("title").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(
            result.values,
            vec![toml::Value::String("Test Document".into())]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_fixed_field_schema() {
        let node = test_node();
        let path = parse_property_path("schema").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(result.values, vec![toml::Value::String("procedure".into())]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_fixed_field_schema_none() {
        let mut node = test_node();
        node.schema = None;
        let path = parse_property_path("schema").unwrap();
        let result = resolve_property_path(&node, &path);
        assert!(result.values.is_empty());
        // schema=None means the key is absent from the serialized TOML,
        // so we get a NotFound diagnostic (informational, not an error).
        assert_eq!(result.diagnostics.len(), 1);
        assert!(
            matches!(&result.diagnostics[0], ResolveDiagnostic::NotFound { key, .. } if key == "schema"),
            "expected NotFound for 'schema', got {:?}",
            result.diagnostics[0]
        );
    }

    #[test]
    fn resolve_fixed_field_id() {
        let node = test_node();
        let path = parse_property_path("id").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(result.values, vec![toml::Value::String("test-doc".into())]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_payload_flat_key() {
        let node = test_node();
        let path = parse_property_path("payload.status").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(result.values, vec![toml::Value::String("open".into())]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_payload_numeric() {
        let node = test_node();
        let path = parse_property_path("payload.priority").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(result.values, vec![toml::Value::Integer(3)]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_payload_array_index() {
        let node = test_node();
        let path = parse_property_path("payload.listing.1").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(result.values, vec![toml::Value::String("beta".into())]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_payload_array_slice() {
        let node = test_node();
        let path = parse_property_path("payload.listing.0:2").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(
            result.values,
            vec![
                toml::Value::String("alpha".into()),
                toml::Value::String("beta".into()),
            ]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_metadata_nested() {
        let node = test_node();
        let path = parse_property_path("metadata.git.branch").unwrap();
        let result = resolve_property_path(&node, &path);
        assert_eq!(result.values, vec![toml::Value::String("main".into())]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_payload_wildcard() {
        let node = test_node();
        let path = parse_property_path("payload.*").unwrap();
        let result = resolve_property_path(&node, &path);
        // Wildcard returns all values in the payload table.
        // BTreeMap ordering: "listing", "priority", "status" (alphabetical).
        assert_eq!(result.values.len(), 3);
        // Verify one of them is the status string
        assert!(result.values.contains(&toml::Value::String("open".into())));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn resolve_nonexistent_path() {
        let node = test_node();
        let path = parse_property_path("payload.nonexistent").unwrap();
        let result = resolve_property_path(&node, &path);
        assert!(result.values.is_empty());
        // Now produces a NotFound diagnostic
        assert_eq!(result.diagnostics.len(), 1);
    }

    // ── PropertyPredicate::evaluate tests ──────────────────────────────

    #[test]
    fn predicate_eq_fixed_field() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("schema").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("procedure".into()),
        };
        assert!(pred.evaluate(&node).matched);

        let pred_miss = PropertyPredicate {
            path: parse_property_path("schema").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("requirement".into()),
        };
        assert!(!pred_miss.evaluate(&node).matched);
    }

    #[test]
    fn predicate_eq_payload_dotted() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("payload.status").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("open".into()),
        };
        assert!(pred.evaluate(&node).matched);
    }

    #[test]
    fn predicate_gt_payload_numeric() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("payload.priority").unwrap(),
            op: CompareOp::Gt,
            value: PropertyValue::Number(2.0),
        };
        assert!(pred.evaluate(&node).matched);

        let pred_fail = PropertyPredicate {
            path: parse_property_path("payload.priority").unwrap(),
            op: CompareOp::Gt,
            value: PropertyValue::Number(5.0),
        };
        assert!(!pred_fail.evaluate(&node).matched);
    }

    #[test]
    fn predicate_exists_present() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("payload.status").unwrap(),
            op: CompareOp::Exists,
            value: PropertyValue::None,
        };
        assert!(pred.evaluate(&node).matched);
    }

    #[test]
    fn predicate_exists_absent() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("payload.nonexistent").unwrap(),
            op: CompareOp::Exists,
            value: PropertyValue::None,
        };
        assert!(!pred.evaluate(&node).matched);
    }

    #[test]
    fn predicate_contains_string() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("title").unwrap(),
            op: CompareOp::Contains,
            value: PropertyValue::String("Document".into()),
        };
        assert!(pred.evaluate(&node).matched);
    }

    #[test]
    fn predicate_contains_array() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("payload.listing").unwrap(),
            op: CompareOp::Contains,
            value: PropertyValue::String("beta".into()),
        };
        assert!(pred.evaluate(&node).matched);

        let pred_miss = PropertyPredicate {
            path: parse_property_path("payload.listing").unwrap(),
            op: CompareOp::Contains,
            value: PropertyValue::String("delta".into()),
        };
        assert!(!pred_miss.evaluate(&node).matched);
    }

    #[test]
    fn predicate_matches_regex() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("title").unwrap(),
            op: CompareOp::Matches,
            value: PropertyValue::Regex("Test.*".into()),
        };
        assert!(pred.evaluate(&node).matched);
    }

    #[test]
    fn predicate_in_set() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("payload.status").unwrap(),
            op: CompareOp::In,
            value: PropertyValue::Set(vec!["open".into(), "closed".into()]),
        };
        assert!(pred.evaluate(&node).matched);

        let pred_miss = PropertyPredicate {
            path: parse_property_path("payload.status").unwrap(),
            op: CompareOp::In,
            value: PropertyValue::Set(vec!["closed".into(), "pending".into()]),
        };
        assert!(!pred_miss.evaluate(&node).matched);
    }

    #[test]
    fn predicate_wildcard_any_match() {
        let node = test_node();
        // payload.* == "open" should match because payload.status == "open"
        let pred = PropertyPredicate {
            path: parse_property_path("payload.*").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("open".into()),
        };
        assert!(pred.evaluate(&node).matched);

        // payload.* == "nonexistent" should not match any payload value
        let pred_miss = PropertyPredicate {
            path: parse_property_path("payload.*").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("nonexistent".into()),
        };
        assert!(!pred_miss.evaluate(&node).matched);
    }

    #[test]
    fn predicate_kind_in_set() {
        let node = test_node(); // kind is {Document}
        let pred = PropertyPredicate {
            path: parse_property_path("kind").unwrap(),
            op: CompareOp::In,
            value: PropertyValue::Set(vec!["Document".into(), "Network".into()]),
        };
        assert!(pred.evaluate(&node).matched);

        let pred_miss = PropertyPredicate {
            path: parse_property_path("kind").unwrap(),
            op: CompareOp::In,
            value: PropertyValue::Set(vec!["Network".into(), "API".into()]),
        };
        assert!(!pred_miss.evaluate(&node).matched);
    }

    #[test]
    fn predicate_kind_eq() {
        let node = test_node(); // kind is {Document}
        let pred = PropertyPredicate {
            path: parse_property_path("kind").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("Document".into()),
        };
        assert!(pred.evaluate(&node).matched);
    }

    #[test]
    fn predicate_metadata_nested_eq() {
        let node = test_node();
        let pred = PropertyPredicate {
            path: parse_property_path("metadata.git.branch").unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("main".into()),
        };
        assert!(pred.evaluate(&node).matched);
    }

    // ── Tape ─────────────────────────────────────────────────────────────

    /// Build a tape simulating a Compose(Difference) evaluation:
    ///   [0] left branch result
    ///   [1] right branch result
    ///   [2] composed (merged) result
    fn make_composition_tape(
        left_entries: Vec<(Bid, Score)>,
        right_entries: Vec<(Bid, Score)>,
        merged_entries: Vec<(Bid, Score)>,
        op: CompositionOp,
    ) -> Tape {
        let left_bids: BTreeSet<Bid> = left_entries.iter().map(|(bid, _)| *bid).collect();
        let right_bids: BTreeSet<Bid> = right_entries.iter().map(|(bid, _)| *bid).collect();
        let intersection: Vec<Bid> = left_bids.intersection(&right_bids).copied().collect();
        Tape {
            steps: vec![
                TapeEntry {
                    label: "0".to_string(),
                    content: TapeContent::Nodes(
                        left_entries.into_iter().map(|(bid, _)| bid).collect(),
                    ),
                    payload: None,
                },
                TapeEntry {
                    label: "1".to_string(),
                    content: TapeContent::Nodes(
                        right_entries.into_iter().map(|(bid, _)| bid).collect(),
                    ),
                    payload: None,
                },
                TapeEntry {
                    label: "2".to_string(),
                    content: TapeContent::Compose {
                        op,
                        left: 0..1,
                        right: 1..2,
                        result: merged_entries.into_iter().map(|(bid, _)| bid).collect(),
                        intersection,
                    },
                    payload: None,
                },
            ],
        }
    }

    #[test]
    fn tape_provenance_and() {
        // A={1,2,3}, B={2,3,4}. And → merged = {2,3}.
        let bid1 = Bid::new(Bid::nil());
        let bid2 = Bid::new(bid1);
        let bid3 = Bid::new(bid2);
        let bid4 = Bid::new(bid3);

        let tape = make_composition_tape(
            vec![(bid1, Some(1.0)), (bid2, Some(1.0)), (bid3, Some(1.0))],
            vec![(bid2, Some(1.0)), (bid3, Some(1.0)), (bid4, Some(1.0))],
            vec![(bid2, Some(1.0)), (bid3, Some(1.0))],
            CompositionOp::And,
        );

        assert!(tape.has_composition());
        assert_eq!(tape.provenance_label(&bid2, CompositionOp::And), "Both");
        assert_eq!(tape.provenance_label(&bid3, CompositionOp::And), "Both");
    }

    #[test]
    fn tape_provenance_difference() {
        // A={1,2,3}, B={2,3,4}. Difference → merged = {1} (left only).
        let bid1 = Bid::new(Bid::nil());
        let bid2 = Bid::new(bid1);
        let bid3 = Bid::new(bid2);
        let bid4 = Bid::new(bid3);
        let _ = bid4;

        let tape = make_composition_tape(
            vec![(bid1, Some(1.0)), (bid2, Some(1.0)), (bid3, Some(1.0))],
            vec![(bid2, Some(1.0)), (bid3, Some(1.0)), (bid4, Some(1.0))],
            vec![(bid1, Some(1.0))],
            CompositionOp::Not,
        );

        // Difference: left-only items get checkmark.
        assert_eq!(tape.provenance_label(&bid1, CompositionOp::Not), "\u{2713}");
    }

    #[test]
    fn tape_provenance_or() {
        // A={1,2,3}, B={2,3,4}. Or → merged = {1,2,3,4}.
        let bid1 = Bid::new(Bid::nil());
        let bid2 = Bid::new(bid1);
        let bid3 = Bid::new(bid2);
        let bid4 = Bid::new(bid3);

        let tape = make_composition_tape(
            vec![(bid1, Some(1.0)), (bid2, Some(1.0)), (bid3, Some(1.0))],
            vec![(bid2, Some(1.0)), (bid3, Some(1.0)), (bid4, Some(1.0))],
            vec![
                (bid1, Some(1.0)),
                (bid2, Some(1.0)),
                (bid3, Some(1.0)),
                (bid4, Some(1.0)),
            ],
            CompositionOp::Or,
        );

        assert_eq!(tape.provenance_label(&bid1, CompositionOp::Or), "Left");
        assert_eq!(tape.provenance_label(&bid2, CompositionOp::Or), "Both");
        assert_eq!(tape.provenance_label(&bid3, CompositionOp::Or), "Both");
        assert_eq!(tape.provenance_label(&bid4, CompositionOp::Or), "Right");
    }

    #[test]
    fn tape_node_steps() {
        let bid1 = Bid::new(Bid::nil());
        let bid2 = Bid::new(bid1);

        let tape = make_composition_tape(
            vec![(bid1, Some(1.0)), (bid2, Some(1.0))],
            vec![(bid2, Some(1.0))],
            vec![(bid2, Some(1.0))],
            CompositionOp::And,
        );

        // bid1 appears only in step 0 (left branch).
        assert_eq!(tape.node_steps(&bid1), vec![0]);
        // bid2 appears in all three steps.
        assert_eq!(tape.node_steps(&bid2), vec![0, 1, 2]);
    }

    #[test]
    fn tape_empty_has_no_composition() {
        let tape = Tape::default();
        assert!(!tape.has_composition());
        assert!(tape.is_empty());
        assert_eq!(tape.len(), 0);
        assert_eq!(tape.composition_op(), None);
    }

    // ── ResolveDiagnostic tests ─────────────────────────────────────────

    #[test]
    fn resolve_unknown_field_produces_diagnostic() {
        let node = test_node();
        let path = parse_property_path("foobar").unwrap();
        let result = resolve_property_path(&node, &path);
        assert!(result.values.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        // With TOML serialization, unknown fields produce NotFound
        assert!(
            matches!(
                &result.diagnostics[0],
                ResolveDiagnostic::NotFound { key, .. } if key == "foobar"
            ),
            "expected NotFound for 'foobar', got {:?}",
            result.diagnostics[0]
        );
    }

    #[test]
    fn resolve_scalar_subpath_produces_diagnostic() {
        let node = test_node();
        let result = resolve_property_path(&node, &parse_property_path("title.foo").unwrap());
        assert!(result.values.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        // title is a String — trying to traverse into it produces TypeMismatch
        assert!(
            matches!(
                &result.diagnostics[0],
                ResolveDiagnostic::TypeMismatch {
                    expected: "Table or Array",
                    found: "String",
                    ..
                }
            ),
            "expected TypeMismatch, got {:?}",
            result.diagnostics[0]
        );
    }

    #[test]
    fn resolve_index_on_table_produces_diagnostic() {
        let node = test_node();
        let path = parse_property_path("payload.0").unwrap();
        let result = resolve_property_path(&node, &path);
        assert!(result.values.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0],
            ResolveDiagnostic::TypeMismatch {
                at_segment: 1,
                expected: "Array",
                found: "Table",
            }
        );
    }

    #[test]
    fn resolve_key_not_found_produces_diagnostic() {
        let node = test_node();
        let path = parse_property_path("payload.nonexistent").unwrap();
        let result = resolve_property_path(&node, &path);
        assert!(result.values.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0],
            ResolveDiagnostic::NotFound {
                at_segment: 1,
                key: "nonexistent".to_string(),
            }
        );
    }

    // ═════════════════════════════════════════════════════════════════════
    // EdgePredicate tests
    // ═════════════════════════════════════════════════════════════════════

    fn test_weight() -> Weight {
        let mut payload = toml::Table::new();
        payload.insert(
            WEIGHT_DOC_PATHS.to_string(),
            toml::Value::String("section-alpha".into()),
        );
        payload.insert(
            WEIGHT_SORT_KEY.to_string(),
            toml::Value::Array(vec![toml::Value::Integer(0), toml::Value::Integer(2)]),
        );
        payload.insert("score".into(), toml::Value::Float(0.85));
        Weight { payload }
    }

    #[test]
    fn edge_predicate_eq_string() {
        let pred = EdgePredicate {
            path: parse_property_path(WEIGHT_DOC_PATHS).unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("section-alpha".into()),
        };
        assert!(pred.matches_weight(&test_weight()));
    }

    #[test]
    fn edge_predicate_eq_string_no_match() {
        let pred = EdgePredicate {
            path: parse_property_path(WEIGHT_DOC_PATHS).unwrap(),
            op: CompareOp::Eq,
            value: PropertyValue::String("section-beta".into()),
        };
        assert!(!pred.matches_weight(&test_weight()));
    }

    #[test]
    fn edge_predicate_exists() {
        let pred = EdgePredicate {
            path: parse_property_path("score").unwrap(),
            op: CompareOp::Exists,
            value: PropertyValue::None,
        };
        assert!(pred.matches_weight(&test_weight()));
    }

    #[test]
    fn edge_predicate_exists_missing_key() {
        let pred = EdgePredicate {
            path: parse_property_path("nonexistent").unwrap(),
            op: CompareOp::Exists,
            value: PropertyValue::None,
        };
        assert!(!pred.matches_weight(&test_weight()));
    }

    #[test]
    fn edge_predicate_gt_numeric() {
        let pred = EdgePredicate {
            path: parse_property_path("score").unwrap(),
            op: CompareOp::Gt,
            value: PropertyValue::Number(0.5),
        };
        assert!(pred.matches_weight(&test_weight()));

        let pred_high = EdgePredicate {
            path: parse_property_path("score").unwrap(),
            op: CompareOp::Gt,
            value: PropertyValue::Number(0.9),
        };
        assert!(!pred_high.matches_weight(&test_weight()));
    }

    // ═════════════════════════════════════════════════════════════════════
    // TraversalSpec fixture tests
    // ═════════════════════════════════════════════════════════════════════

    fn make_section_weights(sort_key: u16) -> WeightSet {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, sort_key).ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Section, w);
        ws
    }

    fn make_node(bid: Bid, kind: BeliefKind, title: &str) -> BeliefNode {
        BeliefNode {
            bid,
            kind: kind.into(),
            title: title.to_string(),
            schema: None,
            payload: Table::new(),
            id: NodeId::Slug,
            metadata: Table::new(),
        }
    }

    /// Build the fixture graph:
    ///
    /// ```text
    ///     api ← net ← doc_a ← sec1
    ///                        ← sec2
    ///               ← doc_b ← sec3
    /// ```
    ///
    /// Arrows are Section edges (source=child, sink=parent).
    /// Returns `(bb, api, net, doc_a, doc_b, sec1, sec2, sec3)`.
    fn traversal_fixture() -> (BeliefBase, Bid, Bid, Bid, Bid, Bid, Bid, Bid) {
        let api = Bid::new(Bid::nil());
        let net = Bid::new(api);
        let doc_a = Bid::new(net);
        let doc_b = Bid::new(net);
        let sec1 = Bid::new(doc_a);
        let sec2 = Bid::new(doc_a);
        let sec3 = Bid::new(doc_b);

        let mut states = rustc_hash::FxHashMap::default();
        states.insert(api, make_node(api, BeliefKind::API, "api"));
        states.insert(net, make_node(net, BeliefKind::Network, "net"));
        states.insert(doc_a, make_node(doc_a, BeliefKind::Document, "doc_a"));
        states.insert(doc_b, make_node(doc_b, BeliefKind::Document, "doc_b"));
        states.insert(sec1, make_node(sec1, BeliefKind::Symbol, "sec1"));
        states.insert(sec2, make_node(sec2, BeliefKind::Symbol, "sec2"));
        states.insert(sec3, make_node(sec3, BeliefKind::Symbol, "sec3"));

        let relations = BidGraph::from_edges(vec![
            (net, api, make_section_weights(0)),    // net → api
            (doc_a, net, make_section_weights(0)),  // doc_a → net
            (doc_b, net, make_section_weights(1)),  // doc_b → net
            (sec1, doc_a, make_section_weights(0)), // sec1 → doc_a
            (sec2, doc_a, make_section_weights(1)), // sec2 → doc_a
            (sec3, doc_b, make_section_weights(0)), // sec3 → doc_b
        ]);

        let graph = BeliefGraph { states, relations };
        let bb = BeliefBase::from(graph);
        (bb, api, net, doc_a, doc_b, sec1, sec2, sec3)
    }

    fn eval_traversal(bb: &BeliefBase, start: Bid, traversal: TraversalSpec) -> BTreeSet<Bid> {
        let spec = QuerySpec::seed_then(
            TapeFn::Bids(vec![start]),
            vec![ProjectionStep::traverse(traversal)],
        );
        let mut package = QueryPackage::new(spec);
        bb.evaluate_query(&mut package).unwrap();
        // Fold all tape entries. seed_then puts the seed TapeFn directly on
        // step 0's input, so there is no seed entry — traversal output_bids
        // contain only discovered BIDs, not the seed itself.
        package.tape().fold_bids(0..package.tape().len())
    }

    #[test]
    fn test_traversal_halo() {
        let (bb, _api, net, doc_a, _doc_b, sec1, sec2, _sec3) = traversal_fixture();
        let result = eval_traversal(&bb, doc_a, TraversalSpec::halo());
        let expected: BTreeSet<Bid> = [net, sec1, sec2].into_iter().collect();
        assert_eq!(
            result, expected,
            "halo(doc_a) should return immediate neighbors: net, sec1, sec2"
        );
    }

    #[test]
    fn test_traversal_balance_map() {
        let (bb, api, net, doc_a, _doc_b, sec1, _sec2, _sec3) = traversal_fixture();
        let result = eval_traversal(&bb, sec1, TraversalSpec::balance_map());
        let expected: BTreeSet<Bid> = [doc_a, net, api].into_iter().collect();
        assert_eq!(
            result, expected,
            "balance_map(sec1) should return all ancestors: doc_a, net, api"
        );
    }

    #[test]
    fn test_traversal_roots() {
        let (bb, api, _net, _doc_a, _doc_b, sec1, _sec2, _sec3) = traversal_fixture();
        let spec = QuerySpec::seed_then(TapeFn::Bids(vec![sec1]), roots());
        let mut package = QueryPackage::new(spec);
        bb.evaluate_query(&mut package).unwrap();
        let result: BTreeSet<Bid> = package
            .tape()
            .steps
            .last()
            .map(|e| e.content.output_bids().into_iter().collect())
            .unwrap_or_default();
        let expected: BTreeSet<Bid> = [api].into_iter().collect();
        assert_eq!(
            result, expected,
            "roots(sec1) should return only api (the node with no parent)"
        );
    }

    #[test]
    fn test_traversal_leaves() {
        let (bb, api, _net, _doc_a, _doc_b, sec1, sec2, sec3) = traversal_fixture();
        let spec = QuerySpec::seed_then(TapeFn::Bids(vec![api]), leaves());
        let mut package = QueryPackage::new(spec);
        bb.evaluate_query(&mut package).unwrap();
        let result: BTreeSet<Bid> = package
            .tape()
            .steps
            .last()
            .map(|e| e.content.output_bids().into_iter().collect())
            .unwrap_or_default();
        let expected: BTreeSet<Bid> = [sec1, sec2, sec3].into_iter().collect();
        assert_eq!(
            result, expected,
            "leaves(api) should return sec1, sec2, sec3 (nodes with no children)"
        );
    }

    #[test]
    fn test_traversal_leaf_map() {
        let (bb, api, net, doc_a, doc_b, sec1, sec2, sec3) = traversal_fixture();
        let result = eval_traversal(&bb, api, TraversalSpec::leaf_map());
        let expected: BTreeSet<Bid> = [net, doc_a, doc_b, sec1, sec2, sec3].into_iter().collect();
        assert_eq!(
            result, expected,
            "leaf_map(api) should return all descendants: net, doc_a, doc_b, sec1, sec2, sec3"
        );
    }

    /// Regression test for the namespace-hub asset-sync bug (noet-core Issue 98):
    /// asset/href nodes are the SOURCE of their Section edge to a namespace hub
    /// (the namespace is the sink), matching the `GraphBuilder::process_asset`
    /// edge shape (`asset_bid --Section--> asset_namespace()`). A query seeded
    /// at the namespace must walk leaf-ward (`leaf_anchored`) to find its
    /// children; walking root-ward (`anchored`) only continues upward and never
    /// reaches them.
    #[test]
    fn test_leaf_anchored_finds_namespace_children() {
        let ns = Bid::new(Bid::nil());
        let asset_a = Bid::new(ns);
        let asset_b = Bid::new(ns);

        let mut states = rustc_hash::FxHashMap::default();
        states.insert(ns, make_node(ns, BeliefKind::Network, "asset_namespace"));
        states.insert(asset_a, make_node(asset_a, BeliefKind::External, "asset_a"));
        states.insert(asset_b, make_node(asset_b, BeliefKind::External, "asset_b"));

        let relations = BidGraph::from_edges(vec![
            (asset_a, ns, make_section_weights(0)), // asset_a -> ns (asset is source)
            (asset_b, ns, make_section_weights(1)), // asset_b -> ns (asset is source)
        ]);

        let graph = BeliefGraph { states, relations };
        let bb = BeliefBase::from(graph);

        // leaf_anchored: must find both asset children.
        let spec = QuerySpec::seed(TapeFn::Bids(vec![ns]));
        let mut leaf_package = QueryPackage::leaf_anchored(spec);
        bb.evaluate_query(&mut leaf_package).unwrap();
        let leaf_graph = leaf_package.into_graph();
        let leaf_bids: BTreeSet<Bid> = leaf_graph.states.keys().copied().collect();
        assert!(
            leaf_bids.contains(&asset_a) && leaf_bids.contains(&asset_b),
            "leaf_anchored(ns) must include asset_a and asset_b; got {:?}",
            leaf_bids
        );

        // anchored (root-ward): must NOT find the children -- this is the bug's
        // prior (broken) behavior preserved as a contrast case, not a desired
        // outcome to design toward.
        let spec2 = QuerySpec::seed(TapeFn::Bids(vec![ns]));
        let mut root_package = QueryPackage::anchored(spec2);
        bb.evaluate_query(&mut root_package).unwrap();
        let root_graph = root_package.into_graph();
        let root_bids: BTreeSet<Bid> = root_graph.states.keys().copied().collect();
        assert!(
            !root_bids.contains(&asset_a) && !root_bids.contains(&asset_b),
            "anchored(ns) (root-ward) should NOT include asset children -- if this \
             assertion fails, the traversal direction semantics have changed and \
             sync_asset_snapshot's use of leaf_anchored should be re-verified"
        );
    }
}
