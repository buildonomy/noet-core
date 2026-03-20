//! MyST directive support for noet.
//!
//! noet extends Markdown with a small set of block directives using the **backtick-fence**
//! syntax from the [MyST spec](https://mystmd.org/guide/syntax-overview). A directive is
//! written as a fenced code block whose info string has the form `{name}`, e.g.
//! `` ````{network_children} `` / `` ```` `` (4-backtick zero-body form).
//!
//! The colon-fence form (`:::`) is **not supported**. It is fatally broken under
//! `pulldown-cmark` with `ENABLE_DEFINITION_LIST`: the serialiser corrupts the closing
//! `:::` to `: ::` on every write-back, and a blank line in the body terminates the
//! underlying `DefinitionList` structure entirely. See
//! `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` for the full empirical analysis.
//!
//! ## Authoring convention
//!
//! Use **4 backticks** for top-level directives. A 3-backtick directive will be normalised to
//! 4 on the first write-back and is then stable. Nested directives use 3 backticks inside a
//! 4-backtick outer fence (the only stable nesting depth).
//!
//! ## How pulldown-cmark represents directives
//!
//! With `buildonomy_md_options()`, pulldown-cmark treats any fenced code block whose info
//! string matches `{...}` as an ordinary fenced code block — it does **not** parse it as a
//! directive. A zero-body directive such as `` ````{network_children}\n```` `` produces
//! exactly two events: `Start(CodeBlock(Fenced("{network_children}")))` followed immediately
//! by `End(CodeBlock)` with no intervening `Text` event.
//!
//! Detection happens in `MdCodec::parse()` by inspecting the `Fenced` info string via
//! [`parse_directive_info`]. Rendering the directive to HTML happens at render time in
//! `render_html_body` / `NetworkCodec::generate_html` — identical to how heading rewriting
//! is deferred to render time.
//!
//! ## Paired block directives
//!
//! Some directives act as **block openers** that change parse behaviour for the content that
//! follows them. They are closed by a matching `` ````{end} `` directive (the universal
//! closer) or automatically when a heading is encountered. Nesting is not supported: opening
//! a second block directive while one is already open emits a warning and implicitly closes
//! the first.
//!
//! Currently supported block opener: `{implements}` — all Markdown links inside the block
//! are recorded as `WeightKind::Pragmatic` upstream relations instead of the default
//! `WeightKind::Epistemic`. The directive itself and the closing `{end}` are both suppressed
//! from HTML output.
//!
//! ## Extension point
//!
//! Add new directives here:
//! 1. Define a `pub const MY_DIRECTIVE_MARKER: &str = "...";` (the internal marker string).
//! 2. Add a mapping arm in [`lookup()`].
//! 3. If it is a block opener, return `true` from [`is_block_opener()`].
//! 4. Document the directive in the module doc comment above.

use std::collections::BTreeMap;

use crate::{
    beliefbase::{BeliefBase, BeliefGraph},
    codec::{md::build_title_attribute, CODECS},
    error::BuildonomyError,
    paths::AnchorPath,
    properties::{Bid, WeightKind},
    query::{Expression, RelationPred},
};

/// Full definition of a noet MyST directive.
///
/// The [`DIRECTIVES`] array is the single source of truth for all directive metadata.
/// All derived operations (`lookup`, `is_block_opener`, `promote_markers`,
/// `process_deferred_directives`) iterate this array.
///
/// ## Adding a new directive
///
/// 1. Add a `DirectiveDef` entry to [`DIRECTIVES`].
/// 2. If it has a deferred render phase, implement query refiners in `queries` and a
///    `fn(&[BeliefGraph]) -> Result<String, BuildonomyError>` builder and set
///    `builder: Some(my_builder)`.
/// 3. Document it in the module doc comment above.
pub struct DirectiveDef {
    /// The name used in the backtick-fence info string, e.g. `"network_children"`.
    pub name: &'static str,
    /// Render-time intermediate HTML comment emitted by `render_html_body` in place of the
    /// directive's `CodeBlock` events. Empty string means the directive is suppressed from
    /// HTML output entirely (e.g. `{implements}`, `{end}`).
    pub marker: &'static str,
    /// Collision-safe placeholder that replaces `marker` in `generate_html` and is later
    /// replaced by sentinel splicing in `generate_html_for_path`. Empty string means no
    /// deferred phase.
    pub sentinel: &'static str,
    /// The opening-line source form written to new files by `noet init` (e.g.
    /// `"````{network_children}"`). Empty string for directives that are never written
    /// programmatically.
    pub directive: &'static str,
    /// Whether this directive opens a parse-behaviour block (changes link weight kind until
    /// `{end}` or the next heading).
    pub is_block_opener: bool,
    /// Async query pipeline, run by `generate_html_for_path` before the sync builder.
    ///
    /// `graphs[0]` is always the node-resolution graph (resolved before this pipeline runs).
    /// Each refiner receives the full `&[BeliefGraph]` slice accumulated so far — use
    /// `graphs[0]` to reference the resolved node and `graphs[graphs.len()-1]` to reference
    /// the immediately preceding step's result.  The `Expression` returned is passed to
    /// `eval_query`; the result is appended to the slice before the next refiner is called.
    ///
    /// Empty slice means no deferred phase.
    pub queries: &'static [fn(&[BeliefGraph]) -> Expression],
    /// Sync deferred-render builder.
    ///
    /// Receives the full `Vec<BeliefGraph>` accumulated by the pipeline:
    /// - `graphs[0]`   — node-resolution graph (always present)
    /// - `graphs[1..]` — one entry per step in `queries`, in order
    ///
    /// **Builders must filter by edge kind** — the slice contains everything fetched by all
    /// prior steps; do not assume it contains only the edges you queried for.
    ///
    /// `None` for parse-only or marker-only directives (i.e. when `queries` is empty).
    pub builder: Option<fn(&[BeliefGraph]) -> Result<String, BuildonomyError>>,
}

/// Registry of all noet MyST directives.
///
/// This is the **single source of truth** for directive metadata. All helper functions
/// (`lookup`, `is_block_opener`, `promote_markers`, `process_deferred_directives`) derive
/// their behaviour from this array. To add a directive, add one entry here.
///
/// Entries with an empty `sentinel` participate only in the parse and render phases.
/// Entries with a non-empty `sentinel` also participate in the deferred-render phase via
/// their `builder`.
pub static DIRECTIVES: &[DirectiveDef] = &[
    DirectiveDef {
        name: "network_children",
        marker: "<!-- network-children -->",
        sentinel: "<!--@@noet-network-children@@-->",
        directive: "````{network_children}",
        is_block_opener: false,
        queries: &[network_children_query],
        builder: Some(build_listing_html),
    },
    DirectiveDef {
        name: "implements",
        marker: "",
        sentinel: "",
        directive: "",
        is_block_opener: true,
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "end",
        marker: "",
        sentinel: "",
        directive: "",
        is_block_opener: false,
        queries: &[],
        builder: None,
    },
    DirectiveDef {
        name: "requirements_table",
        marker: "<!-- noet-requirements-table -->",
        sentinel: "<!--@@noet-requirements-table@@-->",
        directive: "",
        is_block_opener: false,
        queries: &[req_table_step1, req_table_step2],
        builder: Some(build_requirements_table_html),
    },
    // DirectiveDef { name: "toc", marker: "<!-- noet-toc -->", sentinel: "<!--@@noet-toc@@-->",
    //     directive: "", is_block_opener: false, queries: &[toc_query], builder: Some(build_toc_html) },  // TODO(Issue N)
];

/// Return the render-time marker for a directive name, or `""` if unknown.
///
/// The marker is the intermediate HTML comment emitted by `render_html_body` in place of
/// the directive's `CodeBlock` events. An empty string means the directive is suppressed
/// from HTML output (e.g. `{implements}`, `{end}`).
pub fn marker(directive_name: &str) -> &'static str {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .map(|d| d.marker)
        .unwrap_or("")
}

/// Return the collision-safe sentinel for a directive name, or `""` if unknown or none.
///
/// The sentinel is the placeholder injected by `generate_html` and replaced by
/// `process_deferred_directives`. An empty string means the directive has no deferred phase.
pub fn sentinel(directive_name: &str) -> &'static str {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .map(|d| d.sentinel)
        .unwrap_or("")
}

/// Return the author-facing source directive form for a directive name, or `""` if none.
///
/// This is the opening-line string written to new files by `noet init`
/// (e.g. `"````{network_children}"`). Empty string for directives that are never written
/// programmatically.
pub fn directive(directive_name: &str) -> &'static str {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .map(|d| d.directive)
        .unwrap_or("")
}

/// Map a known MyST directive name to its render-time marker string.
///
/// Returns `None` for unknown directive names (the caller should emit a
/// [`crate::codec::diagnostic::ParseDiagnostic`] warning and pass the events through
/// unchanged).
///
/// # Examples
/// ```
/// use noet_core::codec::myst::{lookup, marker};
/// assert_eq!(lookup("network_children"), Some(marker("network_children")));
/// assert_eq!(lookup("implements"),       Some(marker("implements")));
/// assert_eq!(lookup("end"),              Some(marker("end")));
/// assert_eq!(lookup("requirements_table"), Some(marker("requirements_table")));
/// assert_eq!(lookup("unknown_foo"), None);
/// assert_eq!(lookup(""), None);
/// ```
pub fn lookup(directive_name: &str) -> Option<&'static str> {
    DIRECTIVES
        .iter()
        .find(|d| d.name == directive_name)
        .map(|d| d.marker)
}

/// Returns `true` if the named directive opens a parse-behaviour block.
///
/// Block openers change how subsequent content is interpreted (e.g. link relation kind)
/// until a matching `{end}` or a heading is encountered.  The `{end}` directive itself
/// is never a block opener.
///
/// # Examples
/// ```
/// use noet_core::codec::myst::is_block_opener;
/// assert!(is_block_opener("implements"));
/// assert!(!is_block_opener("end"));
/// assert!(!is_block_opener("network_children"));
/// assert!(!is_block_opener("requirements_table"));
/// assert!(!is_block_opener("unknown"));
/// ```
pub fn is_block_opener(directive_name: &str) -> bool {
    DIRECTIVES
        .iter()
        .any(|d| d.name == directive_name && d.is_block_opener)
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
pub(crate) fn promote_markers(body: &str) -> String {
    let mut out = body.to_string();
    for d in DIRECTIVES {
        if !d.marker.is_empty() && !d.sentinel.is_empty() && out.contains(d.marker) {
            out = out.replace(d.marker, d.sentinel);
        }
    }
    out
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
            tracing::info!(
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
// Each refiner is a `fn(&[BeliefGraph]) -> Expression`.
//
// Convention:
//   graphs[0]              — node-resolution graph (always the resolved document node)
//   graphs[graphs.len()-1] — result of the immediately preceding pipeline step
//
// The returned Expression is passed to `eval_query`; the result is appended to the
// accumulated slice before the next refiner (or the builder) is called.

/// Refiner for `network_children` (step 1 of 1).
///
/// `graphs[0]` contains the resolved network node. Returns an Expression that fetches
/// all nodes that have a Section-weighted edge **into** that node (i.e. its direct
/// children in the document tree).
fn network_children_query(graphs: &[BeliefGraph]) -> Expression {
    let node_bid = node_bid_from_graphs(graphs);
    Expression::RelationIn(RelationPred::SinkIn(vec![node_bid]))
}

/// Refiner for `requirements_table` step 1 of 2.
///
/// `graphs[0]` contains the resolved document node. Finds that node's home network BID
/// and returns an Expression that fetches every node belonging to that network
/// (i.e. every node whose `net` key equals the home network's bref).
fn req_table_step1(graphs: &[BeliefGraph]) -> Expression {
    let node_bid = node_bid_from_graphs(graphs);
    // Walk states looking for the first network-kind ancestor.
    // The node-resolution graph produced by a seed-only Query contains the document
    // node itself in states; the network node is its Section-edge sink and may not be
    // present — use the node's own bid as a fallback (covers the network-index case).
    // graphs[0] is an eval_query, which is balanced
    let bb = BeliefBase::from(graphs[0].clone());
    let home_net_bid = bb
        .paths()
        .path(&node_bid)
        .map(|(home_net, _)| home_net)
        .unwrap_or(node_bid);
    // StatePred::NetPathIn(Bid) returns all nodes whose path is registered under
    // the given network BID — i.e. every document in the home network.
    Expression::StateIn(StatePred::NetPathIn(home_net_bid))
}

/// Refiner for `requirements_table` step 2 of 2.
///
/// `graphs[1]` (the result of step 1) contains all nodes in the home network. Collects
/// their BIDs and returns an Expression that fetches every Pragmatic-weighted edge whose
/// source is one of those nodes.
fn req_table_step2(graphs: &[BeliefGraph]) -> Expression {
    // graphs[1] is the home-network node set from step 1.
    let all_bids: Vec<Bid> = if graphs.len() >= 2 {
        graphs[1].states.keys().copied().collect()
    } else {
        // Fallback: use only the document node itself.
        vec![node_bid_from_graphs(graphs)]
    };
    Expression::RelationIn(RelationPred::SinkIn(all_bids))
}

/// Extract the single resolved node BID from `graphs[0]`.
///
/// `graphs[0]` is the node-resolution graph produced by the initial `eval_query` call in
/// `generate_html_for_path`. It contains the document node as the sole non-Trace state,
/// plus any Trace nodes pulled in via its immediate edges. We must find the non-Trace node
/// specifically — `BTreeMap` ordering means `.next()` may return a Trace neighbour first.
///
/// Panics only if called before the node-resolution graph has been pushed, which cannot
/// happen in a correctly constructed pipeline.
fn node_bid_from_graphs(graphs: &[BeliefGraph]) -> Bid {
    use crate::properties::BeliefKind;
    graphs[0]
        .states
        .values()
        .find(|n| !n.kind.contains(BeliefKind::Trace))
        .or_else(|| {
            tracing::warn!("Could not find the non-trace node from our initial query!");
            graphs[0].states.values().next()
        })
        .map(|n| n.bid)
        .expect("graphs[0] is the node-resolution graph and must be non-empty")
}

/// Build the child-listing HTML fragment for the `network_children` directive.
///
/// `graphs` layout:
/// - `graphs[0]` — node-resolution graph; the single state is the network node being rendered.
/// - `graphs[1]` — result of [`network_children_query`]: all nodes that have a
///   `WeightKind::Section` edge **into** the network node (its direct children).
///
/// Produces an HTML `<ul>` of linked child documents sorted by `WEIGHT_SORT_KEY`.
/// Returns an empty-state message when there are no children.
pub(crate) fn build_listing_html(graphs: &[BeliefGraph]) -> Result<String, BuildonomyError> {
    use crate::beliefbase::ExtendedRelation;
    use crate::properties::WEIGHT_SORT_KEY;

    let node_bid = node_bid_from_graphs(graphs);

    // graphs[1] holds the children query result; fall back to an empty graph when absent.
    let static_empty = BeliefGraph::default();
    let children_graph = graphs.get(1).unwrap_or(&static_empty);

    // Build a temporary BeliefBase from the children graph so we can call
    // ExtendedRelation::new, which requires a BeliefBase for path/bref lookups.
    // Union in graphs[0] so the network node's state is available for path resolution.
    let mut bb = BeliefBase::from(graphs[0].clone());
    bb.merge(children_graph);

    let relations = bb.relations();
    let graph = relations.as_graph();

    // Collect all Section-weighted edges whose sink is node_bid.
    let mut children: Vec<(ExtendedRelation<'_>, u16)> = graph
        .raw_edges()
        .iter()
        .filter_map(|edge| {
            let sink_bid = graph[edge.target()];
            if sink_bid != node_bid {
                return None;
            }
            let section_weight = edge.weight.get(&WeightKind::Section)?;
            let sort_key: u16 = section_weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
            let source_bid = graph[edge.source()];
            let rel = ExtendedRelation::new(source_bid, node_bid, &edge.weight, &bb)?;
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
        let mut link_path = edge.root_path.clone();
        let link_ap = AnchorPath::from(&edge.root_path);
        if CODECS.get(&link_ap).is_some() {
            if link_ap.is_dir() {
                link_path = link_ap.join("index.html").into_string();
            } else {
                link_path = link_ap.replace_extension("html");
            }
        }

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

        let bref_attr = bref_attr_for_bid(edge.other.bid, &bb);

        html.push_str(&format!(
            "  <li><a href=\"/{}\"{}>{}</a></li>\n",
            link_path, bref_attr, title
        ));
    }

    if last_subdir.is_some() {
        html.push_str("</ul></li>\n");
    }
    html.push_str("</ul>\n");
    Ok(html)
}

// ── bref_attr helper (used by build_listing_html) ────────────────────────────

fn bref_attr_for_bid(bid: Bid, bb: &BeliefBase) -> String {
    bb.brefs()
        .iter()
        .find_map(|(bref, b)| {
            if b == &bid {
                Some(format!(
                    " title=\"{}\"",
                    build_title_attribute(&format!("bref://{}", bref), false, None)
                ))
            } else {
                None
            }
        })
        .unwrap_or_default()
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
/// - `graphs[0]` — node-resolution graph (the document node being rendered).
/// - `graphs[1]` — result of [`req_table_step1`]: all nodes in the home network.
/// - `graphs[2]` — result of [`req_table_step2`]: all `Pragmatic`-weighted edges whose
///   source is a home-network node.
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
    graphs: &[BeliefGraph],
) -> Result<String, BuildonomyError> {
    // ── Step 1: collect all BIDs in the home network (graphs[1]) ─────────
    let static_empty = BeliefGraph::default();
    let home_net_graph = graphs.get(1).unwrap_or(&static_empty);
    let pragmatic_graph = graphs.get(2).unwrap_or(&static_empty);

    let mut all_bids: Vec<Bid> = home_net_graph.states.keys().copied().collect();
    all_bids.sort();
    all_bids.dedup();

    if all_bids.is_empty() {
        tracing::warn!("[build_requirements_table_html] home network graph is empty");
        return Ok("<p><em>No requirements found for this section.</em></p>\n".to_string());
    }
    tracing::debug!("[build_requirements_table_html] {all_bids:?}");

    // ── Step 2: group by requirement (sink): sink_bid → Vec<source_bid> ──
    // Source = implementor (in home network); sink = requirement (external).
    // BTreeMap for stable ordering.
    let mut req_to_implementors: BTreeMap<Bid, Vec<Bid>> = BTreeMap::new();
    let req_graph = pragmatic_graph.relations.as_graph();
    for edge in req_graph.raw_edges() {
        let source_bid = req_graph[edge.source()]; // implementor
        let sink_bid = req_graph[edge.target()]; // requirement
                                                 // Only include sinks that are NOT in the home network (they are external requirements).
        tracing::debug!("{source_bid} -> {sink_bid}");
        if all_bids.contains(&sink_bid) {
            continue;
        }
        if !edge.weight.get(&WeightKind::Pragmatic).is_some() {
            continue;
        }
        req_to_implementors
            .entry(sink_bid)
            .or_default()
            .push(source_bid);
    }

    if req_to_implementors.is_empty() {
        tracing::warn!("[build_requirements_table_html] req_to_implementors is empty");
        return Ok("<p><em>No requirements found for this section.</em></p>\n".to_string());
    }

    // ── Step 3: build a unified BeliefBase for title/path resolution ──────
    // Union all pipeline graphs so we can resolve both home-net nodes and
    // external requirement nodes.
    let mut bb = BeliefBase::from(graphs[0].clone());
    bb.merge(home_net_graph);
    bb.merge(pragmatic_graph);
    let paths = bb.paths();

    // ── Step 4: render the table ──────────────────────────────────────────
    // Helper: resolve a BID to (display_title, Option<html_url>).
    let resolve = |bid: &Bid| -> (String, Option<String>) {
        let title = bb
            .states()
            .get(bid)
            .map(|n| n.display_title())
            .unwrap_or_else(|| bid.bref().to_string());
        let url = paths.indexed_path(bid).map(|(_net, path, _order)| {
            let ap = AnchorPath::from(&path);
            if ap.ext().eq_ignore_ascii_case("md") {
                format!("/{}", ap.replace_extension("html"))
            } else if ap.is_dir() || ap.ext().is_empty() {
                format!("/{}/index.html", path.trim_end_matches('/'))
            } else {
                format!("/{}", path)
            }
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
            Some(url) => format!("<a href=\"{}\">{}</a>", url, req_title),
            None => req_title,
        };

        let impl_cells: Vec<String> = implementor_bids
            .iter()
            .map(|impl_bid| {
                let (impl_title, impl_url) = resolve(impl_bid);
                match impl_url {
                    Some(url) => format!("<a href=\"{}\">{}</a>", url, impl_title),
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
    Ok(html)
}

// ── StatePred import for query refiners ──────────────────────────────────────
// StatePred is used by req_table_step1; imported here so refiners can reference
// it without a full module path in their fn bodies.
use crate::query::StatePred;

#[cfg(test)]
mod tests {
    use super::*;

    // --- lookup ---

    #[test]
    fn test_lookup_network_children() {
        assert_eq!(lookup("network_children"), Some(marker("network_children")));
    }

    #[test]
    fn test_lookup_implements() {
        assert_eq!(lookup("implements"), Some(marker("implements")));
    }

    #[test]
    fn test_lookup_end() {
        assert_eq!(lookup("end"), Some(marker("end")));
    }

    #[test]
    fn test_lookup_requirements_table() {
        assert_eq!(
            lookup("requirements_table"),
            Some(marker("requirements_table"))
        );
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
}
