//! MyST directive support for noet.
//!
//! noet extends Markdown with two directive **syntax forms**, each detected by a different
//! pulldown-cmark event type. These are orthogonal to whether a directive has a deferred
//! render pipeline — that is determined solely by whether `builder` is set in its
//! [`DirectiveDef`] entry.
//!
//! **Fenced-block syntax** uses the backtick-fence form from the
//! [MyST spec](https://mystmd.org/guide/syntax-overview), detected as a
//! `Start(CodeBlock(Fenced("{name}")))` event. Example: a zero-body `{network_children}`
//! directive is written as four backticks, `{network_children}`, four backticks on the
//! next line. The colon-fence form (`:::`) is **not supported** — it is fatally broken
//! under pulldown-cmark with `ENABLE_DEFINITION_LIST`. See
//! `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` for the full empirical analysis.
//!
//! **Codespan syntax** uses inline code spans detected as `Code("{name}")` or
//! `Code("{relation}args")` events. Example: `` `{uses}` `` opens a Pragmatic-upstream
//! relation context; links that follow are recorded with that context until `` `{end}` ``
//! closes it.
//!
//! Both syntax forms are detected via [`parse_directive_info`] and dispatched through the
//! same [`DIRECTIVES`] lookup — directives are syntax-form-agnostic. A directive with a
//! non-empty sentinel (i.e. `builder.is_some()`) emits its sentinel from whichever form
//! triggered it; the sentinel/splice machinery downstream is form-agnostic.
//!
//! ## Sentinel derivation
//!
//! Sentinels are derived from the directive name as `<!--@@noet-{name}@@-->` (with
//! underscores replaced by hyphens). Only directives with `builder: Some(...)` produce a
//! sentinel; parse-only directives produce no HTML output.
//!
//! ## Subject/verb/referent model (codespan relation directives)
//!
//! The authoring document is always the **owner** (subject). The **verb** (directive name)
//! determines which graph slot the **referent** (referenced node) occupies; the subject
//! takes the other slot automatically.
//!
//! ## Codespan directive forms
//!
//! - **Named verb**: `` `{uses}` ``, `` `{implements}` ``, etc. Looked up in the global
//!   [`DIRECTIVES`] registry (pre-populated with all built-in verbs), then the per-document
//!   session registry.
//! - **Precise form**: `` `{relation}kind=pragmatic, ref=source` `` — parsed by
//!   [`parse_directive_info`] as `name="relation"`, `args="kind=pragmatic, ref=source"`.
//! - **Custom verb registration**: `` `{relation}name=mitigates, kind=pragmatic, ref=source` ``
//!   — registers `mitigates` in the per-document session registry (last-one-wins).
//! - **Closer**: `` `{end}` `` — pops the relation context stack.
//!
//! The relation context is a **stack**: nested verbs push; `{end}` pops and restores the
//! outer context. A heading boundary implicitly drains the stack with a warning per
//! unclosed entry. Unrecognized `{...}` code spans pass through silently.
//!
//! ## Authoring convention (fenced-block)
//!
//! Use **4 backticks** for top-level directives. A 3-backtick directive will be normalised
//! to 4 on the first write-back and is then stable. Nested directives use 3 backticks
//! inside a 4-backtick outer fence (the only stable nesting depth).
//!
//! ## Extension points
//!
//! **Adding a directive with a deferred pipeline**:
//! 1. Add a `DirectiveDef` entry with unique `name`; implement `queries` and `builder`.
//! 2. `MdCodec::parse()` emits the derived sentinel automatically from both `CodeBlock`
//!    and `Code` detection paths when `builder.is_some()`. No per-arm special-casing needed.
//! 3. Document it in this module doc comment.
//!
//! **Adding a codespan relation verb**:
//! 1. Add a `DirectiveDef` entry with `weight_kind: Some(...)` and `ref_role: Some(...)`.
//!    Set `directive: ""`, `queries: &[]`, `builder: None`.
//! 2. [`global_verb_context`] picks it up automatically. No other changes needed.
//! 3. Document the verb in the codespan directive forms list above.

use std::collections::BTreeMap;

use crate::{
    beliefbase::{BeliefContext, BeliefGraph},
    codec::{CODECS, WALK_CODECS},
    error::BuildonomyError,
    paths::AnchorPath,
    properties::{Bid, WeightKind},
    query::spec::{
        DepthCount, ProjectionStep, QuerySpec, Role, TapeFn, TraversalDepth, TraversalSpec,
    },
};
use enumset::EnumSet;

/// The role of the **referent** (referenced node) in a codespan relation directive.
///
/// In the subject/verb/referent model, the authoring document is always the subject/owner.
/// The verb determines which graph slot the referent occupies; the subject takes the other
/// slot automatically:
///
/// - `Source`: referent is the graph source (tributary); subject is the sink (ocean).
///   Links push to `IRNode::upstream`.
/// - `Sink`: referent is the graph sink (ocean); subject is the source (tributary).
///   Links push to `IRNode::downstream`.
///
/// See `docs/design/dag_model.md` §3 for the full subject/verb/referent model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceRole {
    /// Referent is the graph source → subject is sink → links go to `upstream`.
    Source,
    /// Referent is the graph sink → subject is source → links go to `downstream`.
    Sink,
}

impl From<ReferenceRole> for petgraph::Direction {
    fn from(role: ReferenceRole) -> Self {
        match role {
            ReferenceRole::Source => petgraph::Direction::Incoming,
            ReferenceRole::Sink => petgraph::Direction::Outgoing,
        }
    }
}

impl From<petgraph::Direction> for ReferenceRole {
    fn from(dir: petgraph::Direction) -> Self {
        match dir {
            petgraph::Direction::Incoming => ReferenceRole::Source,
            petgraph::Direction::Outgoing => ReferenceRole::Sink,
        }
    }
}

/// Query refiner function type for MyST directive deferred-render pipelines.
///
/// Each refiner receives the resolved node's [`BeliefContext`] and the slice of
/// [`BeliefGraph`] results accumulated by preceding refiners, and returns a
/// [`QuerySpec`] to evaluate against the global BeliefBase.
pub type DirectiveRefiner = fn(&BeliefContext, &[BeliefGraph]) -> QuerySpec;

/// Sync builder function type for MyST directive deferred-render pipelines.
///
/// Receives the resolved node's [`BeliefContext`] and the full slice of
/// [`BeliefGraph`] results from the query pipeline, and returns an HTML fragment.
pub type DirectiveBuilder = fn(&BeliefContext, &[BeliefGraph]) -> Result<String, BuildonomyError>;

/// Full definition of a noet MyST directive.
///
/// The [`DIRECTIVES`] array is the single source of truth for all directive metadata.
/// All derived operations (`lookup`, `is_block_opener`, `global_verb_context`,
/// `process_deferred_directives`) iterate this array.
///
/// ## Sentinel derivation
///
/// Sentinels are **not stored** in `DirectiveDef`. They are derived on demand from the
/// directive name as `<!--@@noet-{name}@@-->` (underscores → hyphens) via [`sentinel`].
/// Only directives with `builder: Some(...)` produce a sentinel; calling [`sentinel`] on
/// a parse-only directive returns an empty string.
///
/// ## Adding a directive with a deferred pipeline
///
/// 1. Add a `DirectiveDef` entry with a unique `name`; implement `queries` and `builder`.
/// 2. `render_html_body` emits `sentinel(name)` automatically from both `CodeBlock` and
///    `Code` detection paths when `builder.is_some()`. No per-arm special-casing needed.
/// 3. Document it in the module doc comment above.
///
/// ## Adding a codespan relation verb
///
/// 1. Add a `DirectiveDef` entry with `weight_kind: Some(...)` and `ref_role: Some(...)`.
///    Set `directive: ""`, `queries: &[]`, `builder: None`.
/// 2. [`global_verb_context`] picks it up automatically. No other changes needed.
pub struct DirectiveDef {
    /// The name used in the backtick-fence info string or codespan, e.g. `"network_children"`.
    pub name: &'static str,
    /// The canonical fenced-block source form written to new files by `noet init` (e.g.
    /// `"````{network_children}"`). Empty string for directives never written programmatically.
    pub directive: &'static str,
    /// For relation verbs: the role of the **referent** (referenced node) in the edge.
    /// `Some(Source)` → referent is graph source, subject is sink → links go to `upstream`.
    /// `Some(Sink)` → referent is graph sink, subject is source → links go to `downstream`.
    /// `None` for directives that do not participate in relation context.
    pub ref_role: Option<ReferenceRole>,
    /// For relation verbs: the `WeightKind` of edges produced while this context is active.
    /// `None` for directives that do not participate in relation context.
    pub weight_kind: Option<WeightKind>,
    /// Async query pipeline, run by `generate_html_for_path` before the sync builder.
    ///
    /// Each refiner receives the resolved node's [`BeliefContext`] and the slice of
    /// `BeliefGraph` results accumulated by preceding refiners (`graphs[graphs.len()-1]`
    /// is the immediately preceding step's result). The `QuerySpec` returned is passed to
    /// `evaluate`; the result is appended to the slice before the next refiner is called.
    ///
    /// Empty slice means no deferred phase.
    pub queries: &'static [DirectiveRefiner],
    /// Sync deferred-render builder.
    ///
    /// Receives the resolved node's [`BeliefContext`] and the slice of `BeliefGraph` results
    /// accumulated by the pipeline (one entry per step in `queries`, in order).
    ///
    /// **Builders must filter by edge kind** — the slice contains everything fetched by all
    /// prior steps; do not assume it contains only the edges you queried for.
    ///
    /// `None` for parse-only directives (relation verbs, `{end}`, etc.).
    pub builder: Option<DirectiveBuilder>,
}

/// Registry of all noet MyST directives.
///
/// This is the **single source of truth** for directive metadata. All helper functions
/// (`lookup`, `is_block_opener`, `global_verb_context`, `process_deferred_directives`)
/// derive their behaviour from this array. To add a directive, add one entry here.
///
/// Entries with `builder: Some(...)` participate in the deferred-render phase; their
/// sentinel is derived automatically via [`sentinel`]. Parse-only entries (relation verbs,
/// `{end}`) produce no HTML output. Entries with `weight_kind` and `ref_role` set are
/// codespan relation verbs registered automatically in [`global_verb_context`].
///
/// **SYNC REQUIRED**: verb entries with `ref_role.is_some()` are also recognised as
/// named traversal shorthands in `src/query/parser.rs::is_shorthand_name`. When adding
/// or renaming a verb directive, update that function and `parse_named_shorthand` too.
pub static DIRECTIVES: &[DirectiveDef] = &[
    // --- Fenced-block directives (deferred render pipeline) ---
    DirectiveDef {
        name: "network_children",
        directive: "````{network_children}",
        ref_role: None,
        weight_kind: None,
        queries: &[net_path_in],
        builder: Some(
            build_listing_html
                as fn(&BeliefContext, &[BeliefGraph]) -> Result<String, BuildonomyError>,
        ),
    },
    DirectiveDef {
        name: "requirements_table",
        directive: "",
        ref_role: None,
        weight_kind: None,
        queries: &[net_path_in, req_table_step2],
        builder: Some(build_requirements_table_html),
    },
    DirectiveDef {
        name: "maps_to",
        directive: "",
        ref_role: None,
        weight_kind: None,
        queries: &[mapping_table_query],
        builder: Some(build_mapping_table_html_unfiltered),
    },
    // `covers` is a synonym for `maps_to` — preferred name going forward.
    // `maps_to` remains for backward compatibility with existing source documents.
    DirectiveDef {
        name: "covers",
        directive: "",
        ref_role: None,
        weight_kind: None,
        queries: &[mapping_table_query],
        builder: Some(build_mapping_table_html_unfiltered),
    },
    DirectiveDef {
        name: "query",
        directive: "",
        ref_role: None,
        weight_kind: None,
        queries: &[],
        builder: None,
    },
    // DirectiveDef { name: "toc", directive: "", ref_role: None, weight_kind: None,
    //     queries: &[toc_query], builder: Some(build_toc_html) },  // TODO(Issue N)

    // --- Codespan relation verbs ---
    // Referent role: Source → referent is graph source, subject is sink → upstream.
    // Referent role: Sink   → referent is graph sink, subject is source → downstream.
    DirectiveDef {
        name: "uses",
        directive: "",
        ref_role: Some(ReferenceRole::Source),
        weight_kind: Some(WeightKind::Pragmatic),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "implements", // legacy alias for {uses}
        directive: "",
        ref_role: Some(ReferenceRole::Source),
        weight_kind: Some(WeightKind::Pragmatic),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "used_by",
        directive: "",
        ref_role: Some(ReferenceRole::Sink),
        weight_kind: Some(WeightKind::Pragmatic),
        queries: &[],
        builder: None,
    },
    // Epistemic / normative-coupling verbs (EMO §7.2: normative couplings on the epistemic axis)
    // constrained_by / constrains are the canonical pair; draws_from / underlies are aliases.
    DirectiveDef {
        name: "draws_from",
        directive: "",
        ref_role: Some(ReferenceRole::Source),
        weight_kind: Some(WeightKind::Epistemic),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "underlies",
        directive: "",
        ref_role: Some(ReferenceRole::Sink),
        weight_kind: Some(WeightKind::Epistemic),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "constrained_by",
        directive: "",
        ref_role: Some(ReferenceRole::Source),
        weight_kind: Some(WeightKind::Epistemic),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "constrains",
        directive: "",
        ref_role: Some(ReferenceRole::Sink),
        weight_kind: Some(WeightKind::Epistemic),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "consists_of",
        directive: "",
        ref_role: Some(ReferenceRole::Source),
        weight_kind: Some(WeightKind::Section),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "composed_of",
        directive: "",
        ref_role: Some(ReferenceRole::Source),
        weight_kind: Some(WeightKind::Section),
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "component_of",
        directive: "",
        ref_role: Some(ReferenceRole::Sink),
        weight_kind: Some(WeightKind::Section),
        queries: &[],
        builder: None,
    },
    // --- Codespan closers / special (no relation context, no render output) ---
    DirectiveDef {
        name: "end",
        directive: "",
        ref_role: None,
        weight_kind: None,
        queries: &[],
        builder: None,
    },
];

/// Return the collision-safe sentinel for a directive name.
///
/// Sentinels are derived from the name as `<!--@@noet-{name}@@-->` (underscores replaced
/// with hyphens). Returns `""` for unknown names or directives without a deferred pipeline
/// (`builder` is `None`). Directives with `builder: Some(...)` return the derived sentinel.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::sentinel;
/// assert_eq!(sentinel("network_children"), "<!--@@noet-network-children@@-->");
/// assert_eq!(sentinel("requirements_table"), "<!--@@noet-requirements-table@@-->");
/// assert_eq!(sentinel("implements"), "");
/// assert_eq!(sentinel("end"), "");
/// assert_eq!(sentinel("unknown"), "");
/// ```
pub fn sentinel(directive_name: &str) -> String {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .filter(|d| d.builder.is_some())
        .map(|d| format!("<!--@@noet-{}@@-->", d.name.replace('_', "-")))
        .unwrap_or_default()
}

/// Return the author-facing source directive form for a directive name, or `""` if none.
///
/// This is the opening-line string written to new files by `noet init`
/// (e.g. `"````{network_children}"`). Empty string for directives never written
/// programmatically by the tool.
pub fn directive(directive_name: &str) -> &'static str {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .map(|d| d.directive)
        .unwrap_or("")
}

/// Return the `(WeightKind, ReferenceRole)` for a codespan relation verb, or `None` if the
/// name is not a registered relation verb.
///
/// This is the **global tier** of the two-tier verb lookup. The per-document session
/// registry in `MdCodec` takes priority; this function is the fallback.
///
/// `{end}`, `{relation}`, and pipeline directives (`{network_children}`, etc.) return `None`.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::{global_verb_context, ReferenceRole};
/// use noet_core::properties::WeightKind;
/// assert_eq!(global_verb_context("uses"),        Some((WeightKind::Pragmatic, ReferenceRole::Source)));
/// assert_eq!(global_verb_context("implements"),  Some((WeightKind::Pragmatic, ReferenceRole::Source)));
/// assert_eq!(global_verb_context("used_by"),     Some((WeightKind::Pragmatic, ReferenceRole::Sink)));
/// assert_eq!(global_verb_context("draws_from"),  Some((WeightKind::Epistemic, ReferenceRole::Source)));
/// assert_eq!(global_verb_context("underlies"),   Some((WeightKind::Epistemic, ReferenceRole::Sink)));
/// assert_eq!(global_verb_context("consists_of"), Some((WeightKind::Section,   ReferenceRole::Source)));
/// assert_eq!(global_verb_context("composed_of"), Some((WeightKind::Section,   ReferenceRole::Source)));
/// assert_eq!(global_verb_context("component_of"),Some((WeightKind::Section,   ReferenceRole::Sink)));
/// assert_eq!(global_verb_context("end"),         None);
/// assert_eq!(global_verb_context("maps_to"),     None);
/// assert_eq!(global_verb_context("unknown"),     None);
/// ```
pub fn global_verb_context(name: &str) -> Option<(WeightKind, ReferenceRole)> {
    DIRECTIVES
        .iter()
        .find(|d| d.name == name)
        .and_then(|d| d.weight_kind.zip(d.ref_role))
}

/// Parse the args of a `{relation}` codespan directive.
///
/// Accepts a comma-separated list of `key=value` pairs. Recognised keys:
/// - `kind`: `pragmatic`, `epistemic`, or `section` (case-insensitive)
/// - `ref`: `source` or `sink` (case-insensitive)
/// - `name`: optional custom verb name to register in the session registry
///
/// Returns `(Option<verb_name>, WeightKind, ReferenceRole)` on success, or `None` if
/// `kind` or `ref` are missing or unrecognised. Unrecognised keys are silently ignored.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::{parse_relation_args, ReferenceRole};
/// use noet_core::properties::WeightKind;
/// assert_eq!(
///     parse_relation_args("kind=pragmatic, ref=source"),
///     Some((None, WeightKind::Pragmatic, ReferenceRole::Source))
/// );
/// assert_eq!(
///     parse_relation_args("kind=epistemic, ref=sink"),
///     Some((None, WeightKind::Epistemic, ReferenceRole::Sink))
/// );
/// assert_eq!(
///     parse_relation_args("name=mitigates, kind=pragmatic, ref=source"),
///     Some((Some("mitigates".to_string()), WeightKind::Pragmatic, ReferenceRole::Source))
/// );
/// assert_eq!(parse_relation_args(""), None);
/// assert_eq!(parse_relation_args("kind=unknown, ref=source"), None);
/// assert_eq!(parse_relation_args("garbage"), None);
/// ```
pub fn parse_relation_args(args: &str) -> Option<(Option<String>, WeightKind, ReferenceRole)> {
    let mut kind: Option<WeightKind> = None;
    let mut ref_role: Option<ReferenceRole> = None;
    let mut verb_name: Option<String> = None;

    for pair in args.split(',') {
        let pair = pair.trim();
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        match key {
            "kind" => {
                kind = match value.to_lowercase().as_str() {
                    "pragmatic" => Some(WeightKind::Pragmatic),
                    "epistemic" => Some(WeightKind::Epistemic),
                    "section" => Some(WeightKind::Section),
                    _ => return None,
                };
            }
            "ref" => {
                ref_role = match value.to_lowercase().as_str() {
                    "source" => Some(ReferenceRole::Source),
                    "sink" => Some(ReferenceRole::Sink),
                    _ => return None,
                };
            }
            "name" if !value.is_empty() => {
                verb_name = Some(value.to_string());
            }
            _ => {}
        }
    }

    Some((verb_name, kind?, ref_role?))
}

/// Map a known MyST directive name to `Some(sentinel)`, or `None` for unknown names.
///
/// Returns `Some("")` for parse-only directives (relation verbs, `{end}`) and
/// `Some("<!--@@noet-{name}@@-->")` for pipeline directives. Returns `None` only for
/// completely unknown names — callers should emit a warning in that case.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::lookup;
/// assert!(lookup("network_children").is_some());
/// assert!(lookup("implements").is_some());
/// assert!(lookup("uses").is_some());
/// assert!(lookup("end").is_some());
/// assert_eq!(lookup("unknown_foo"), None);
/// assert_eq!(lookup(""), None);
/// ```
/// Returns `true` if the named directive is a pipeline directive (has a deferred render
/// builder). Pipeline directives (`{network_children}`, `{requirements_table}`, `{maps_to}`)
/// are valid known directives but are NOT relation verbs — they produce no relation context
/// and should not be dispatched to `dispatch_relation_directive`.
///
/// This predicate allows callers to gate on `lookup().is_some()` for "is this name known?"
/// while still distinguishing pipeline directives from relation verbs.
pub fn is_pipeline_directive(directive_name: &str) -> bool {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .is_some_and(|d| d.builder.is_some())
}

/// Return the `DirectiveDef` for a given name, or `None` if unknown.
///
/// This is the primary registry-driven dispatch helper. Callers use the returned def to
/// determine how to handle a directive without enumerating names explicitly:
///
/// - `def.weight_kind.is_some()` → relation verb; call `dispatch_relation_directive`
/// - `def.builder.is_some()` → pipeline directive; set `has_deferred_render`
/// - `def.queries.is_empty() && def.builder.is_none() && def.weight_kind.is_none()`
///   → control keyword (`end`, etc.); handled by `dispatch_relation_directive` directly
pub fn directive_def(name: &str) -> Option<&'static DirectiveDef> {
    DIRECTIVES.iter().find(|d| d.name == name)
}

pub fn lookup(directive_name: &str) -> Option<String> {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .map(|d| {
            if d.builder.is_some() {
                format!("<!--@@noet-{}@@-->", d.name.replace('_', "-"))
            } else {
                String::new()
            }
        })
}

/// Returns `true` if the named directive is a codespan relation verb (pushes a relation
/// context onto the stack).
///
/// This is a derived convenience: equivalent to `global_verb_context(name).is_some()`.
/// `{end}`, fenced-block directives, and unknown names return `false`.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::is_block_opener;
/// assert!(is_block_opener("implements"));
/// assert!(is_block_opener("uses"));
/// assert!(is_block_opener("draws_from"));
/// assert!(!is_block_opener("end"));
/// assert!(!is_block_opener("network_children"));
/// assert!(!is_block_opener("requirements_table"));
/// assert!(!is_block_opener("unknown"));
/// ```
pub fn is_block_opener(directive_name: &str) -> bool {
    global_verb_context(directive_name).is_some()
}

/// Parse a fenced code block info string and return `(name, args)` if it is a MyST directive.
///
/// A directive info string has the form `{name}` or `{name} args`. This function returns
/// `None` for plain info strings (e.g. `"rust"`, `"python"`) so callers can distinguish
/// ordinary code blocks from directives without extra checks.
///
/// The returned `name` has the surrounding braces stripped. `args` is the trimmed remainder
/// after the closing `}`, which may be empty.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::parse_directive_info;
/// assert_eq!(parse_directive_info("{network_children}"), Some(("network_children", "")));
/// assert_eq!(parse_directive_info("{figure} image.png"), Some(("figure", "image.png")));
/// assert_eq!(parse_directive_info("rust"), None);
/// assert_eq!(parse_directive_info(""), None);
/// assert_eq!(parse_directive_info("{"), None);  // no closing brace
/// assert_eq!(parse_directive_info("{}"), Some(("", "")));  // empty name — caller handles
/// ```
pub fn parse_directive_info(info: &str) -> Option<(&str, &str)> {
    let rest = info.strip_prefix('{')?;
    let close = rest.find('}')?;
    let name = &rest[..close];
    let args = rest[close + 1..].trim();
    Some((name, args))
}

/// Replace all known render-time markers in `body` with their collision-safe sentinels.
///
/// Iterates [`DIRECTIVES`] and replaces each non-empty `marker` with its `sentinel` when
/// present. Called from `generate_html` (both `MdCodec` and `NetworkCodec`) after
/// `render_html_body`. Directives with an empty marker or empty sentinel are skipped.
/// Documents that do not contain a given marker are unaffected.
/// Return the anchor-and-index-parameterized collision-safe sentinel for a `{maps_to}` directive.
///
/// The marker embeds the owning section's heading anchor (e.g. `trace-mapping`) and a
/// per-section 0-based index so that multiple `{maps_to}` directives in the same section
/// each get a distinct sentinel.
///
/// Format: `<!--@@noet-mapping-table:ANCHOR:INDEX@@-->` where ANCHOR is the URL-safe heading id.
pub fn mapping_table_sentinel(owner_anchor: &str, index: usize) -> String {
    format!("<!--@@noet-mapping-table:{owner_anchor}:{index}@@-->")
}

/// Scan an HTML string for all anchor-and-index-parameterized mapping-table sentinels
/// and return `(anchor, index)` tuples in document order.
///
/// Finds all occurrences of `<!--@@noet-mapping-table:ANCHOR:INDEX@@-->` and extracts
/// both the ANCHOR and the INDEX.  Entries with empty anchors or non-numeric indices
/// are silently skipped.
pub fn mapping_table_sentinel_anchors(html: &str) -> Vec<(String, usize)> {
    const PREFIX: &str = "<!--@@noet-mapping-table:";
    const SUFFIX: &str = "@@-->";
    let mut results = Vec::new();
    let mut remaining = html;
    while let Some(start) = remaining.find(PREFIX) {
        remaining = &remaining[start + PREFIX.len()..];
        if let Some(end) = remaining.find(SUFFIX) {
            let candidate = &remaining[..end];
            remaining = &remaining[end + SUFFIX.len()..];
            if candidate.is_empty() {
                continue;
            }
            // Format is "ANCHOR:INDEX" — split on the last colon so anchors with colons
            // (e.g. from bref-derived ids) are handled correctly.
            if let Some(colon_pos) = candidate.rfind(':') {
                let anchor = &candidate[..colon_pos];
                let idx_str = &candidate[colon_pos + 1..];
                if !anchor.is_empty() {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        results.push((anchor.to_string(), idx));
                    }
                }
            }
        } else {
            break;
        }
    }
    results
}

/// Parse MyST directive options from a fenced block body.
///
/// Options are `:key: value` lines at the start of the body. The first line
/// that does NOT match the `:key: value` pattern ends the options section.
/// Blank lines within the options section are skipped.
///
/// Returns `(options, remaining_body)` where `remaining_body` is the text
/// after all option lines, with leading whitespace trimmed.
pub fn parse_directive_options(body: &str) -> (BTreeMap<String, String>, String) {
    let mut options = BTreeMap::new();
    let mut consumed_up_to = 0;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            consumed_up_to += line.len() + 1; // +1 for the newline
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(':') {
            if let Some(colon_pos) = rest.find(':') {
                let key = &rest[..colon_pos];
                // Key must be non-empty and contain only alphanumeric, hyphens, underscores
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    let value = rest[colon_pos + 1..].trim();
                    options.insert(key.to_string(), value.to_string());
                    consumed_up_to += line.len() + 1; // +1 for the newline
                    continue;
                }
            }
        }
        // First non-option, non-blank line ends the options section
        break;
    }
    let remaining = &body[consumed_up_to.min(body.len())..];
    (options, remaining.trim_start().to_string())
}

/// Return the per-instance sentinel for a `{query}` directive block.
///
/// Format: `<!--@@noet-query:N@@-->` where N is the 0-based block index
/// within the document.
pub fn query_sentinel(index: usize) -> String {
    format!("<!--@@noet-query:{index}@@-->")
}

/// Scan an HTML string for all per-instance query sentinels and return
/// the block indices found, in document order.
pub fn query_sentinel_indices(html: &str) -> Vec<usize> {
    const PREFIX: &str = "<!--@@noet-query:";
    const SUFFIX: &str = "@@-->";
    let mut indices = Vec::new();
    let mut remaining = html;
    while let Some(start) = remaining.find(PREFIX) {
        remaining = &remaining[start + PREFIX.len()..];
        if let Some(end) = remaining.find(SUFFIX) {
            let candidate = &remaining[..end];
            if let Ok(idx) = candidate.parse::<usize>() {
                indices.push(idx);
            }
            remaining = &remaining[end + SUFFIX.len()..];
        } else {
            break;
        }
    }
    indices
}

/// Splice pre-built HTML fragments into an existing on-disk HTML file by replacing sentinels.
///
/// `replacements` is a slice of `(sentinel, html)` pairs. Each sentinel that is present in
/// the file is replaced with the corresponding HTML. Sentinels absent from the file are
/// silently skipped (author opt-out).
///
/// Returns `true` if at least one replacement was made and the file was rewritten,
/// `false` if nothing was changed.
pub(crate) fn splice_sentinels(
    path: &std::path::Path,
    replacements: &[(&str, &str)],
) -> Result<bool, BuildonomyError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        BuildonomyError::Codec(format!("Failed to read existing HTML at {:?}: {}", path, e))
    })?;

    let mut merged = content;
    let mut wrote_something = false;

    for (sentinel, html) in replacements {
        if html.is_empty() {
            continue;
        }
        if merged.contains(sentinel) {
            merged = merged.replace(sentinel, html);
            wrote_something = true;
        } else {
            tracing::debug!(
                "[myst] sentinel {:?} not found in {:?}, skipping",
                sentinel,
                path
            );
        }
    }

    if wrote_something {
        std::fs::write(path, merged).map_err(|e| {
            BuildonomyError::Codec(format!(
                "Failed to write deferred HTML to {:?}: {}",
                path, e
            ))
        })?;
    }
    Ok(wrote_something)
}

// ── Query refiners ────────────────────────────────────────────────────────────
//
// Each refiner is a `fn(&[BeliefGraph]) -> QuerySpec`.
//
// Convention:
//   graphs[0]              — node-resolution graph (always the resolved document node)
//   graphs[graphs.len()-1] — result of the immediately preceding pipeline step
//
// The returned QuerySpec is passed to `evaluate`; the result is appended to the
// accumulated slice before the next refiner (or the builder) is called.

/// Refiner for `requirements_table` step 1 of 2.
///
/// Uses the resolved node's [`BeliefContext`] to find the home network BID, then returns
/// a QuerySpec that fetches every node belonging to that network (i.e. every node whose
/// `net` key equals the home network's bref).
fn net_path_in(ctx: &BeliefContext, _graphs: &[BeliefGraph]) -> QuerySpec {
    let home_net_bid = if ctx.node.kind.is_network() {
        ctx.node.bid
    } else {
        ctx.home_net
    };
    // Section traversal toward leaves from network root collects all
    // descendant nodes. The network node is the sink (consumer) of Section
    // edges from its dependencies (sources), so input=Sink, output=Source.
    QuerySpec::seed_then(
        TapeFn::Bids(vec![home_net_bid]),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Sink.into(),
            kind_filter: WeightKind::Section.into(),
            output_roles: Role::Source.into(),
            depth: TraversalDepth {
                count: DepthCount::Max,
                edge_filter: None,
            },
            inverted: false,
        })],
    )
}

/// Refiner for `requirements_table` step 2 of 2.
///
/// `graphs[0]` (the result of step 1) contains all nodes in the home network. Collects
/// their BIDs and returns a QuerySpec that fetches every Pragmatic-weighted edge whose
/// source is one of those nodes.
fn req_table_step2(ctx: &BeliefContext, graphs: &[BeliefGraph]) -> QuerySpec {
    // graphs[0] is the home-network node set from step 1.
    let home_net_bid = if ctx.node.kind.is_network() {
        ctx.node.bid
    } else {
        ctx.home_net
    };
    let all_net_bids: Vec<Bid> = if let Some(home_net_graph) = graphs.first() {
        let all_net_bids: Vec<Bid> = home_net_graph
            .relations
            .as_subgraph_seeded(WeightKind::Section, true, home_net_bid)
            .nodes()
            .collect();
        if all_net_bids.is_empty() {
            tracing::debug!(
                "[req_table_step2] home net graph is empty! results:\n{home_net_graph}",
            );
        }
        all_net_bids
    } else {
        // Fallback: use only the document node itself.
        tracing::debug!("[req_table_step2] no results from net_path_in query!");
        vec![ctx.node.bid]
    };

    // "Give me every node that is a source on a Pragmatic edge pointing into
    // a home-network node." Seed = home-network nodes as sinks, traverse
    // incoming Pragmatic edges, collect the sources (implementors).
    QuerySpec::seed_then(
        TapeFn::Bids(all_net_bids),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Sink.into(),
            kind_filter: WeightKind::Pragmatic.into(),
            output_roles: Role::Source.into(),
            depth: TraversalDepth::count(1),
            inverted: false,
        })],
    )
}

/// Build the child-listing HTML fragment for the `network_children` directive.
///
/// `graphs` layout:
/// - `graphs[0]` — result of [`network_children_query`]: all nodes that have a
///   `WeightKind::Section` edge **into** the network node (its direct children).
///
/// `ctx` is the resolved network node's [`BeliefContext`].
///
/// Produces an HTML `<ul>` of linked child documents sorted by `WEIGHT_SORT_KEY`.
/// Returns an empty-state message when there are no children.
pub(crate) fn build_listing_html(
    ctx: &BeliefContext,
    graphs: &[BeliefGraph],
) -> Result<String, BuildonomyError> {
    use crate::beliefbase::ExtendedRelation;
    use crate::properties::WEIGHT_SORT_KEY;

    let node_bid = ctx.node.bid;

    // graphs[0] holds the children query result; fall back to an empty graph when absent.
    let static_empty = BeliefGraph::default();
    let children_graph = graphs.first().unwrap_or(&static_empty);

    // Build a temporary BeliefBase from the children graph so we can call
    // ExtendedRelation::new, which requires a BeliefBase for path/bref lookups.
    // Union in the node-resolution graph from ctx so the network node's state is
    // available for path resolution.
    let mut bb = ctx.beliefbase().clone();
    bb.merge(children_graph);

    let relations = bb.relations();
    let graph = relations.as_graph();

    // Collect all Section-weighted edges whose sink is node_bid.
    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    let graph_edges: Vec<_> = graph.edge_references().collect();
    let mut children: Vec<(ExtendedRelation<'_>, u16)> = graph_edges
        .iter()
        .filter_map(|edge| {
            let sink_bid = graph[edge.target()];
            if sink_bid != node_bid {
                return None;
            }
            let section_weight = edge.weight().get(&WeightKind::Section)?;
            let sort_key: u16 = section_weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
            let source_bid = graph[edge.source()];
            let rel = ExtendedRelation::new(source_bid, node_bid, edge.weight(), &bb)?;
            Some((rel, sort_key))
        })
        .collect();

    children.sort_by_key(|(_, sort_key)| *sort_key);

    if children.is_empty() {
        return Ok("<p><em>No documents in this network yet.</em></p>\n".to_string());
    }

    let mut html = String::from("<ul>\n");
    let mut last_subdir: Option<String> = None;

    for (edge, _sort_key) in &children {
        if !edge.other.kind.is_document() {
            // Only render documents, not file contents
            continue;
        }
        let link_ap = AnchorPath::from(&edge.root_path);
        // Convert the source path to its HTML equivalent before computing a relative link.
        let html_path = if CODECS.get(&link_ap).is_some()
            || WALK_CODECS.should_track(std::path::Path::new(link_ap.filepath()))
        {
            if link_ap.is_dir() {
                link_ap.join("index.html").into_string()
            } else {
                link_ap.replace_extension("html")
            }
        } else {
            edge.root_path.clone()
        };

        // Compute a relative path from the rendering document (ctx.root_path) to the
        // child's HTML path. Both are network-relative, so rooted=false is correct.
        let ctx_ap = AnchorPath::from(&ctx.root_path);
        let rel_link = ctx_ap.path_to(&html_path, false);

        let title = edge.other.display_title();
        if link_ap.dir().is_empty() {
            if last_subdir.is_some() {
                html.push_str("</ul></li>");
                last_subdir = None;
            }
        } else if let Some(ref last_dir) = last_subdir {
            if link_ap.dir() != last_dir {
                html.push_str(&format!("</ul></li><li><span>{}</span><ul>", link_ap.dir()));
                last_subdir = Some(link_ap.dir().to_string());
            }
        } else {
            html.push_str(&format!("<li><span>{}</span><ul>", link_ap.dir()));
            last_subdir = Some(link_ap.dir().to_string());
        }

        let bref_attr = format!("bref://{}", edge.other.bid.bref());

        html.push_str(&format!(
            "  <li><a href=\"{}\"{}>{}</a></li>\n",
            rel_link, bref_attr, title
        ));
    }

    if last_subdir.is_some() {
        html.push_str("</ul></li>\n");
    }
    html.push_str("</ul>\n");
    Ok(html)
}

/// Render all parsed events to an HTML body string, rewriting document links to `.html`.
///
/// This is the shared rendering kernel used by both `generate_html` (which derives the
/// output filename from the source path) and `NetworkCodec::generate_html` (which always
/// uses `index.html` as the output filename). Keeping the rendering logic in one place
/// ensures link-rewriting behaviour stays consistent across both code paths.
/// Build the requirements-table HTML fragment for the `requirements_table` directive.
///
/// `graphs` layout:
/// - `graphs[0]` — result of [`req_table_step1`]: all nodes in the home network.
/// - `graphs[1]` — result of [`req_table_step2`]: all `Pragmatic`-weighted edges whose
///   source is a home-network node.
///
/// `ctx` is the resolved document node's [`BeliefContext`].
///
/// Each `Pragmatic` edge represents an `{implements}` link:
///   `source` = the implementing node (inside the home network)
///   `sink`   = the requirement node (external)
///
/// Produces an HTML table with two columns:
///   | Requirement | Implemented By |
///
/// One row per unique requirement (sink), listing all implementing nodes (sources)
/// in the second column as linked titles.
///
/// Returns an empty-state message when no Pragmatic relations are found.
pub(crate) fn build_requirements_table_html(
    ctx: &BeliefContext,
    graphs: &[BeliefGraph],
) -> Result<String, BuildonomyError> {
    // ── Step 1: collect all BIDs in the home network (graphs[0]) ─────────
    let static_empty = BeliefGraph::default();
    let home_net_graph = graphs.first().unwrap_or(&static_empty);
    let pragmatic_graph = graphs.get(1).unwrap_or(&static_empty);

    let home_net_bid = if ctx.node.kind.is_network() {
        ctx.node.bid
    } else {
        ctx.home_net
    };
    let all_net_bids: Vec<Bid> = if let Some(home_net_graph) = graphs.first() {
        home_net_graph
            .relations
            .as_subgraph_seeded(WeightKind::Section, true, home_net_bid)
            .nodes()
            .collect()
    } else {
        // Fallback: use only the document node itself.
        tracing::debug!("[build_requirements_table_html] no results from net_path_in query!");
        vec![ctx.node.bid]
    };

    if all_net_bids.is_empty() {
        tracing::debug!("[build_requirements_table_html] home network graph is empty");
        return Ok("<p><em>No requirements found for this section.</em></p>\n".to_string());
    }

    // ── Step 2: group by requirement (sink): sink_bid → Vec<source_bid> ──
    // Source = implementor (in home network); sink = requirement (external).
    // BTreeMap for stable ordering.
    let mut req_to_implementors: BTreeMap<Bid, Vec<Bid>> = BTreeMap::new();
    let req_graph = pragmatic_graph.relations.as_graph();
    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    let req_edges: Vec<_> = req_graph.edge_references().collect();
    for edge in req_edges {
        let source_bid = req_graph[edge.source()]; // implementor
        let sink_bid = req_graph[edge.target()]; // requirement
                                                 // Only include sinks that are NOT in the home network (they are external requirements).
        if !edge.weight().weights.contains_key(&WeightKind::Pragmatic) {
            continue;
        }
        if !all_net_bids.contains(&sink_bid) {
            continue;
        }
        req_to_implementors
            .entry(source_bid)
            .or_default()
            .push(sink_bid);
    }

    if req_to_implementors.is_empty() {
        tracing::debug!("[build_requirements_table_html] req_to_implementors is empty");
        return Ok("<p><em>No requirements found for this section.</em></p>\n".to_string());
    }

    // ── Step 3: build a unified BeliefBase for title/path resolution ──────
    // Union the node-resolution BB from ctx with all pipeline graphs so we can
    // resolve both home-net nodes and external requirement nodes.
    let mut bb = ctx.beliefbase().clone();
    bb.merge(home_net_graph);
    bb.merge(pragmatic_graph);
    let pmm = bb.paths();

    // ── Step 4: render the table ──────────────────────────────────────────
    // Helper: resolve a BID to (display_title, Option<relative_html_url>).
    //
    // Prefer net_indexed_path so we get the network-local path (no cross-net BID prefix),
    // then compute a relative link from the rendering document's location (ctx.root_path).
    let mut table_file_path = ctx.root_path.clone();
    let mut table_doc_ap = AnchorPath::from(&table_file_path);
    if table_doc_ap.is_dir() || table_doc_ap.ext().is_empty() {
        table_file_path = format!("{}/index.html", ctx.root_path.trim_end_matches('/'));
        table_doc_ap = AnchorPath::new_file(&table_file_path);
    }

    let resolve = |bid: &Bid| -> (String, Option<String>) {
        let title = bb
            .states()
            .get(bid)
            .map(|n| n.display_title())
            .unwrap_or_else(|| bid.bref().to_string());
        // Try the home network first; fall back to indexed_path for external nodes.
        let maybe_net_path = pmm
            .get_map(&ctx.root_net.bref())
            .and_then(|pm| pm.path(bid, &pmm));
        let url = maybe_net_path.map(|(_home_net, path, _order)| {
            let ap = AnchorPath::from(&path);
            let html_path = if ap.ext().eq_ignore_ascii_case("md") {
                ap.replace_extension("html")
            } else if ap.is_dir() || ap.ext().is_empty() {
                format!("{}/index.html", path.trim_end_matches('/'))
            } else {
                path.clone()
            };
            // Both ctx.root_path and the resolved path are network-relative, so
            // rooted=false produces a correct relative link between them.
            table_doc_ap.path_to(&html_path, true)
        });
        (title, url)
    };

    let mut html = String::from(
        "<table class=\"noet-requirements-table\">\n\
         <thead><tr><th>Requirement</th><th>Implemented By</th></tr></thead>\n\
         <tbody>\n",
    );

    for (req_bid, implementor_bids) in &req_to_implementors {
        let (req_title, req_url) = resolve(req_bid);
        let req_cell = match req_url {
            Some(url) => format!(
                "<a href=\"{}\" title=\"bref://{}\">{}</a>",
                url,
                req_bid.bref(),
                req_title
            ),
            None => req_title,
        };

        let impl_cells: Vec<String> = implementor_bids
            .iter()
            .map(|impl_bid| {
                let (impl_title, impl_url) = resolve(impl_bid);
                match impl_url {
                    Some(url) => format!(
                        "<a href=\"{}\" title=\"bref://{}\">{}</a>",
                        url,
                        impl_bid.bref(),
                        impl_title
                    ),
                    None => impl_title,
                }
            })
            .collect();
        let impl_cell = impl_cells.join(", ");

        html.push_str(&format!(
            "  <tr><td>{}</td><td>{}</td></tr>\n",
            req_cell, impl_cell
        ));
    }

    html.push_str("</tbody>\n</table>\n");
    tracing::debug!(
        "[build_requirements_table_html] generated table for {}: {}",
        ctx.node.bid.bref(),
        ctx.node.display_title()
    );
    Ok(html)
}

/// Refiner for `maps_to` / `mapping_table` step 1 of 1.
///
/// Returns a [`QuerySpec`] that fetches all edges owned by the rendering node's bref
/// (i.e. edges where `WEIGHT_OWNED_BY` equals the node's bref string).
fn mapping_table_query(ctx: &BeliefContext, _graphs: &[BeliefGraph]) -> QuerySpec {
    // "Give me all edges owned by this section, and both their endpoints."
    // Owner input role scans edges for WEIGHT_OWNED_BY matching the section bref.
    QuerySpec::seed_then(
        TapeFn::Bids(vec![ctx.node.bid]),
        vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Owner.into(),
            kind_filter: EnumSet::all(),
            output_roles: Role::Source | Role::Sink,
            depth: TraversalDepth::count(1),
            inverted: false,
        })],
    )
}

/// Build the mapping table HTML fragment for the `maps_to` directive.
///
/// `graphs[0]` contains all edges owned by the rendering section node (fetched by
/// [`mapping_table_query`]). For each owned edge, renders a row with Source / Kind / Sink
/// columns plus any extra payload columns (sparse; blank when absent for a given row).
///
/// Returns an empty-state message when no owned edges are found.
/// Wrapper for use as a [`DirectiveBuilder`] function pointer — calls
/// [`build_mapping_table_html`] with no per-directive filters (shows all owned edges).
fn build_mapping_table_html_unfiltered(
    ctx: &BeliefContext,
    graphs: &[BeliefGraph],
) -> Result<String, BuildonomyError> {
    build_mapping_table_html(ctx, graphs, None, None)
}

pub(crate) fn build_mapping_table_html(
    ctx: &BeliefContext,
    graphs: &[BeliefGraph],
    filter_sources: Option<&[Bid]>,
    filter_sinks: Option<&[Bid]>,
) -> Result<String, BuildonomyError> {
    use crate::properties::{WEIGHT_OWNED_BY, WEIGHT_SORT_KEY};
    use petgraph::visit::{EdgeRef, IntoEdgeReferences};

    let static_empty = BeliefGraph::default();
    let owned_graph = graphs.first().unwrap_or(&static_empty);

    // Collect all edges owned by this node's bref.
    let owner_bref = ctx.node.bid.bref();
    let owner_bref_str = owner_bref.to_string();

    // Build a unified belief base for title/path resolution.
    let mut bb = ctx.beliefbase().clone();
    bb.merge(owned_graph);
    let pmm = bb.paths();

    // Resolve a BID to (display_title, Option<relative_html_url>).
    let mut table_file_path = ctx.root_path.clone();
    let mut table_doc_ap = AnchorPath::from(&table_file_path);
    if table_doc_ap.is_dir() || table_doc_ap.ext().is_empty() {
        table_file_path = format!("{}/index.html", ctx.root_path.trim_end_matches('/'));
        table_doc_ap = AnchorPath::new_file(&table_file_path);
    }

    let resolve = |bid: &Bid| -> (String, Option<String>) {
        let title = bb
            .states()
            .get(bid)
            .map(|n| n.display_title())
            .unwrap_or_else(|| bid.bref().to_string());
        let maybe_net_path = pmm
            .get_map(&ctx.root_net.bref())
            .and_then(|pm| pm.path(bid, &pmm));
        let url = maybe_net_path.map(|(_home_net, path, _order)| {
            let ap = AnchorPath::from(&path);
            let html_path = if ap.ext().eq_ignore_ascii_case("md") {
                ap.replace_extension("html")
            } else if ap.is_dir() || ap.ext().is_empty() {
                format!("{}/index.html", path.trim_end_matches('/'))
            } else {
                path.clone()
            };
            table_doc_ap.path_to(&html_path, true)
        });
        (title, url)
    };
    // Row type: (source_bid, sink_bid, kind, extras)
    type MappingTableRow = (Bid, Bid, WeightKind, Vec<(String, String)>);

    // Collect owned edges.
    let rel_graph = owned_graph.relations.as_graph();
    let mut rows: Vec<MappingTableRow> = rel_graph
        .edge_references()
        .filter_map(|edge| {
            let source_bid = rel_graph[edge.source()];
            let sink_bid = rel_graph[edge.target()];
            let weights = edge.weight();
            for (kind, weight) in weights.weights.iter() {
                let owned_by: Option<String> = weight.get(WEIGHT_OWNED_BY);
                if owned_by.as_deref() == Some(&owner_bref_str) {
                    // If per-directive filters are set, only include edges whose source
                    // and sink BIDs appear in the declared sets for this directive.
                    if let (Some(fsrc), Some(fsnk)) = (filter_sources, filter_sinks) {
                        if !fsrc.contains(&source_bid) || !fsnk.contains(&sink_bid) {
                            continue;
                        }
                    }
                    let skip = [WEIGHT_OWNED_BY, WEIGHT_SORT_KEY];
                    let extras: Vec<(String, String)> = weight
                        .payload
                        .iter()
                        .filter(|(k, _)| !skip.contains(&k.as_str()))
                        .filter_map(|(k, v)| {
                            v.as_str().map(|s| (k.clone(), s.to_string())).or_else(|| {
                                Some((
                                    k.clone(),
                                    v.as_integer()
                                        .map(|i| i.to_string())
                                        .or_else(|| v.as_float().map(|f| f.to_string()))
                                        .or_else(|| v.as_bool().map(|b| b.to_string()))
                                        .unwrap_or_default(),
                                ))
                            })
                        })
                        .collect();
                    return Some((source_bid, sink_bid, *kind, extras));
                }
            }
            None
        })
        .collect();

    if rows.is_empty() {
        return Ok("<p><em>No mappings declared.</em></p>\n".to_string());
    }

    // Stable sort: kind → sink BID → source BID for deterministic output.
    rows.sort_by(|a, b| a.2.cmp(&b.2).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));

    // Collect all extra payload column names (stable order across all rows).
    let mut extra_cols: Vec<String> = Vec::new();
    for (_, _, _, extras) in &rows {
        for (k, _) in extras {
            if !extra_cols.contains(k) {
                extra_cols.push(k.clone());
            }
        }
    }

    // Group rows by WeightKind, preserving sorted order within each kind.
    // Use a Vec of (kind, rows) to preserve kind order from the sort above.
    let mut kinds_seen: Vec<WeightKind> = Vec::new();
    for (_, _, kind, _) in &rows {
        if !kinds_seen.contains(kind) {
            kinds_seen.push(*kind);
        }
    }

    let mut html = String::new();

    for kind in &kinds_seen {
        let kind_rows: Vec<&MappingTableRow> =
            rows.iter().filter(|(_, _, k, _)| k == kind).collect();

        // Build extra header cells for this table.
        let mut extra_headers = String::new();
        for col in &extra_cols {
            extra_headers.push_str(&format!("<th>{}</th>", col));
        }

        html.push_str(&format!(
            "<table class=\"noet-mapping-table\">\n\
             <caption>{kind:?}</caption>\n\
             <thead><tr><th>Sink</th><th>Source</th>{extra_headers}</tr></thead>\n\
             <tbody>\n"
        ));

        // Group by sink within this kind to compute rowspans.
        // sink_groups: Vec<(sink_bid, Vec<&MappingTableRow>)> in sink order.
        let mut sink_groups: Vec<(Bid, Vec<&MappingTableRow>)> = Vec::new();
        for row in &kind_rows {
            let sink_bid = row.1;
            if let Some(group) = sink_groups.iter_mut().find(|(b, _)| *b == sink_bid) {
                group.1.push(row);
            } else {
                sink_groups.push((sink_bid, vec![row]));
            }
        }

        for (sink_bid, sink_rows) in &sink_groups {
            let (sink_title, sink_url) = resolve(sink_bid);
            let sink_link = match sink_url {
                Some(ref url) => format!(
                    "<a href=\"{}\" title=\"bref://{}\">{}</a>",
                    url,
                    sink_bid.bref(),
                    sink_title
                ),
                None => sink_title,
            };

            let rowspan = sink_rows.len();
            let rowspan_attr = if rowspan > 1 {
                format!(" rowspan=\"{rowspan}\"")
            } else {
                String::new()
            };

            for (idx, (source_bid, _, _, extras)) in sink_rows.iter().enumerate() {
                let (source_title, source_url) = resolve(source_bid);
                let source_link = match source_url {
                    Some(ref url) => format!(
                        "<a href=\"{}\" title=\"bref://{}\">{}</a>",
                        url,
                        source_bid.bref(),
                        source_title
                    ),
                    None => source_title,
                };

                let mut extra_cells = String::new();
                for col in &extra_cols {
                    let val = extras
                        .iter()
                        .find(|(k, _)| k == col)
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("");
                    extra_cells.push_str(&format!("<td>{val}</td>"));
                }

                if idx == 0 {
                    // First source for this sink: emit sink cell with rowspan.
                    html.push_str(&format!(
                        "  <tr><td{rowspan_attr}>{sink_link}</td><td>{source_link}</td>{extra_cells}</tr>\n"
                    ));
                } else {
                    // Continuation rows: sink cell already spanned, omit it.
                    html.push_str(&format!("  <tr><td>{source_link}</td>{extra_cells}</tr>\n"));
                }
            }
        }

        html.push_str("</tbody>\n</table>\n");
    }

    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- sentinel ---

    #[test]
    fn test_sentinel_network_children() {
        assert_eq!(
            sentinel("network_children"),
            "<!--@@noet-network-children@@-->"
        );
    }

    #[test]
    fn test_sentinel_requirements_table() {
        assert_eq!(
            sentinel("requirements_table"),
            "<!--@@noet-requirements-table@@-->"
        );
    }

    #[test]
    fn test_sentinel_maps_to() {
        assert_eq!(sentinel("maps_to"), "<!--@@noet-maps-to@@-->");
    }

    #[test]
    fn test_sentinel_parse_only_is_empty() {
        assert_eq!(sentinel("implements"), "");
        assert_eq!(sentinel("uses"), "");
        assert_eq!(sentinel("end"), "");
    }

    #[test]
    fn test_sentinel_unknown_is_empty() {
        assert_eq!(sentinel("unknown"), "");
    }

    // --- lookup ---

    #[test]
    fn test_lookup_network_children() {
        assert_eq!(
            lookup("network_children"),
            Some(sentinel("network_children"))
        );
    }

    #[test]
    fn test_lookup_implements() {
        assert_eq!(lookup("implements"), Some(String::new()));
    }

    #[test]
    fn test_lookup_end() {
        assert_eq!(lookup("end"), Some(String::new()));
    }

    #[test]
    fn test_lookup_requirements_table() {
        assert_eq!(
            lookup("requirements_table"),
            Some(sentinel("requirements_table"))
        );
    }

    #[test]
    fn test_lookup_uses() {
        assert_eq!(lookup("uses"), Some(String::new()));
    }

    #[test]
    fn test_lookup_draws_from() {
        assert_eq!(lookup("draws_from"), Some(String::new()));
    }

    #[test]
    fn test_lookup_unknown_returns_none() {
        assert_eq!(lookup("unknown_foo"), None);
    }

    #[test]
    fn test_lookup_empty_returns_none() {
        assert_eq!(lookup(""), None);
    }

    // --- is_block_opener ---

    #[test]
    fn test_is_block_opener_implements() {
        assert!(is_block_opener("implements"));
    }

    #[test]
    fn test_is_block_opener_uses() {
        assert!(is_block_opener("uses"));
    }

    #[test]
    fn test_is_block_opener_used_by() {
        assert!(is_block_opener("used_by"));
    }

    #[test]
    fn test_is_block_opener_constrained_by() {
        assert!(is_block_opener("constrained_by"));
    }

    #[test]
    fn test_is_block_opener_constrains() {
        assert!(is_block_opener("constrains"));
    }

    #[test]
    fn test_is_block_opener_draws_from() {
        assert!(is_block_opener("draws_from"));
    }

    #[test]
    fn test_is_block_opener_underlies() {
        assert!(is_block_opener("underlies"));
    }

    #[test]
    fn test_is_block_opener_consists_of() {
        assert!(is_block_opener("consists_of"));
    }

    #[test]
    fn test_is_block_opener_composed_of() {
        assert!(is_block_opener("composed_of"));
    }

    #[test]
    fn test_is_block_opener_component_of() {
        assert!(is_block_opener("component_of"));
    }

    #[test]
    fn test_is_block_opener_end_is_false() {
        assert!(!is_block_opener("end"));
    }

    #[test]
    fn test_is_block_opener_network_children_is_false() {
        assert!(!is_block_opener("network_children"));
    }

    #[test]
    fn test_is_block_opener_requirements_table_is_false() {
        assert!(!is_block_opener("requirements_table"));
    }

    #[test]
    fn test_is_block_opener_unknown_is_false() {
        assert!(!is_block_opener("unknown"));
    }

    // --- global_verb_context ---

    #[test]
    fn test_global_verb_context_uses() {
        assert_eq!(
            global_verb_context("uses"),
            Some((WeightKind::Pragmatic, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_global_verb_context_implements() {
        assert_eq!(
            global_verb_context("implements"),
            Some((WeightKind::Pragmatic, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_global_verb_context_used_by() {
        assert_eq!(
            global_verb_context("used_by"),
            Some((WeightKind::Pragmatic, ReferenceRole::Sink))
        );
    }

    #[test]
    fn test_global_verb_context_constrained_by() {
        assert_eq!(
            global_verb_context("constrained_by"),
            Some((WeightKind::Epistemic, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_global_verb_context_constrains() {
        assert_eq!(
            global_verb_context("constrains"),
            Some((WeightKind::Epistemic, ReferenceRole::Sink))
        );
    }

    #[test]
    fn test_global_verb_context_draws_from() {
        assert_eq!(
            global_verb_context("draws_from"),
            Some((WeightKind::Epistemic, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_global_verb_context_underlies() {
        assert_eq!(
            global_verb_context("underlies"),
            Some((WeightKind::Epistemic, ReferenceRole::Sink))
        );
    }

    #[test]
    fn test_global_verb_context_consists_of() {
        assert_eq!(
            global_verb_context("consists_of"),
            Some((WeightKind::Section, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_global_verb_context_composed_of() {
        assert_eq!(
            global_verb_context("composed_of"),
            Some((WeightKind::Section, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_global_verb_context_component_of() {
        assert_eq!(
            global_verb_context("component_of"),
            Some((WeightKind::Section, ReferenceRole::Sink))
        );
    }

    #[test]
    fn test_global_verb_context_end_is_none() {
        assert_eq!(global_verb_context("end"), None);
    }

    #[test]
    fn test_global_verb_context_maps_to_is_none() {
        assert_eq!(global_verb_context("maps_to"), None);
    }

    #[test]
    fn test_global_verb_context_unknown_is_none() {
        assert_eq!(global_verb_context("unknown"), None);
    }

    // --- parse_relation_args ---

    #[test]
    fn test_parse_relation_args_pragmatic_source() {
        assert_eq!(
            parse_relation_args("kind=pragmatic, ref=source"),
            Some((None, WeightKind::Pragmatic, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_parse_relation_args_epistemic_sink() {
        assert_eq!(
            parse_relation_args("kind=epistemic, ref=sink"),
            Some((None, WeightKind::Epistemic, ReferenceRole::Sink))
        );
    }

    #[test]
    fn test_parse_relation_args_section_source() {
        assert_eq!(
            parse_relation_args("kind=section, ref=source"),
            Some((None, WeightKind::Section, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_parse_relation_args_with_name() {
        assert_eq!(
            parse_relation_args("name=mitigates, kind=pragmatic, ref=source"),
            Some((
                Some("mitigates".to_string()),
                WeightKind::Pragmatic,
                ReferenceRole::Source
            ))
        );
    }

    #[test]
    fn test_parse_relation_args_case_insensitive() {
        assert_eq!(
            parse_relation_args("kind=Pragmatic, ref=Source"),
            Some((None, WeightKind::Pragmatic, ReferenceRole::Source))
        );
    }

    #[test]
    fn test_parse_relation_args_unknown_kind_returns_none() {
        assert_eq!(parse_relation_args("kind=unknown, ref=source"), None);
    }

    #[test]
    fn test_parse_relation_args_unknown_ref_returns_none() {
        assert_eq!(parse_relation_args("kind=pragmatic, ref=unknown"), None);
    }

    #[test]
    fn test_parse_relation_args_missing_kind_returns_none() {
        assert_eq!(parse_relation_args("ref=source"), None);
    }

    #[test]
    fn test_parse_relation_args_missing_ref_returns_none() {
        assert_eq!(parse_relation_args("kind=pragmatic"), None);
    }

    #[test]
    fn test_parse_relation_args_empty_returns_none() {
        assert_eq!(parse_relation_args(""), None);
    }

    #[test]
    fn test_parse_relation_args_garbage_returns_none() {
        assert_eq!(parse_relation_args("garbage"), None);
    }

    // --- parse_directive_info ---

    #[test]
    fn test_parse_directive_info_simple() {
        assert_eq!(
            parse_directive_info("{network_children}"),
            Some(("network_children", ""))
        );
    }

    #[test]
    fn test_parse_directive_info_with_args() {
        assert_eq!(
            parse_directive_info("{figure} image.png"),
            Some(("figure", "image.png"))
        );
    }

    #[test]
    fn test_parse_directive_info_relation_precise_form() {
        assert_eq!(
            parse_directive_info("{relation}kind=pragmatic, ref=source"),
            Some(("relation", "kind=pragmatic, ref=source"))
        );
    }

    #[test]
    fn test_parse_directive_info_args_trimmed() {
        assert_eq!(
            parse_directive_info("{note}   some text  "),
            Some(("note", "some text"))
        );
    }

    #[test]
    fn test_parse_directive_info_plain_language_tag() {
        assert_eq!(parse_directive_info("rust"), None);
    }

    #[test]
    fn test_parse_directive_info_empty_string() {
        assert_eq!(parse_directive_info(""), None);
    }

    #[test]
    fn test_parse_directive_info_open_brace_only() {
        // No closing brace — not a directive
        assert_eq!(parse_directive_info("{"), None);
    }

    #[test]
    fn test_parse_directive_info_empty_name() {
        // Empty name between braces — caller is responsible for treating this as unknown
        assert_eq!(parse_directive_info("{}"), Some(("", "")));
        assert_eq!(lookup(""), None); // lookup correctly rejects it
    }

    #[test]
    fn test_parse_directive_info_no_leading_brace() {
        assert_eq!(parse_directive_info("json"), None);
        assert_eq!(parse_directive_info("python3"), None);
    }

    // --- query directive ---

    #[test]
    fn test_sentinel_query_is_empty() {
        assert_eq!(sentinel("query"), "");
    }

    #[test]
    fn test_lookup_query() {
        assert_eq!(lookup("query"), Some(String::new()));
    }

    // --- parse_directive_options ---

    #[test]
    fn test_parse_directive_options_basic() {
        let body = ":view: depth0\n\nSome body text";
        let (opts, remaining) = parse_directive_options(body);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts.get("view").unwrap(), "depth0");
        assert_eq!(remaining, "Some body text");
    }

    #[test]
    fn test_parse_directive_options_multiple() {
        let body = ":view: depth0\n:columns: title,status\n\nBody here";
        let (opts, remaining) = parse_directive_options(body);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts.get("view").unwrap(), "depth0");
        assert_eq!(opts.get("columns").unwrap(), "title,status");
        assert_eq!(remaining, "Body here");
    }

    #[test]
    fn test_parse_directive_options_no_options() {
        let body = "Just a body with no options";
        let (opts, remaining) = parse_directive_options(body);
        assert!(opts.is_empty());
        assert_eq!(remaining, "Just a body with no options");
    }

    #[test]
    fn test_parse_directive_options_blank_lines() {
        let body = "\n:view: depth0\n\n:limit: 10\n\nBody";
        let (opts, remaining) = parse_directive_options(body);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts.get("view").unwrap(), "depth0");
        assert_eq!(opts.get("limit").unwrap(), "10");
        assert_eq!(remaining, "Body");
    }

    // --- query_sentinel ---

    #[test]
    fn test_query_sentinel() {
        assert_eq!(query_sentinel(0), "<!--@@noet-query:0@@-->");
        assert_eq!(query_sentinel(3), "<!--@@noet-query:3@@-->");
    }

    // --- query_sentinel_indices ---

    #[test]
    fn test_query_sentinel_indices() {
        let html = "<p>hello</p><!--@@noet-query:0@@--><div>stuff</div><!--@@noet-query:2@@-->";
        let indices = query_sentinel_indices(html);
        assert_eq!(indices, vec![0, 2]);
    }
}
