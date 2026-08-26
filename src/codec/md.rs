#[cfg(feature = "server-math")]
use katex::{render_with_opts, Opts as KatexOpts};

use once_cell::sync::Lazy;

use crate::codec::md_options::buildonomy_md_options;
use crate::codec::myst::{global_verb_context, parse_relation_args, ReferenceRole};
use pulldown_cmark::{
    BrokenLink, CodeBlockKind as MdCodeBlockKind, CowStr, Event as MdEvent, HeadingLevel, LinkType,
    MetadataBlockKind, Parser as MdParser, Tag as MdTag, TagEnd as MdTagEnd,
};
use pulldown_cmark_to_cmark::{
    cmark_resume_with_source_range_and_options, Options as CmarkToCmarkOptions,
};
use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{BufRead, BufReader, Read},
    mem::replace,
    ops::Range,
    path::Path,
    result::Result,
    str::FromStr,
};
use titlecase::titlecase;
/// Utilities for parsing various document types into BeliefBases
use toml_edit::value;

use crate::{
    beliefbase::BeliefContext,
    codec::{
        belief_ir::{IRNode, IntermediateRelation},
        byte_offset_to_location,
        diagnostic::ParseDiagnostic,
        DocCodec,
    },
    error::BuildonomyError,
    nodekey::{href_to_nodekey, NodeKey},
    paths::{as_anchor, os_path_to_string, to_anchor, AnchorPath},
    properties::{href_namespace, BeliefKind, BeliefNode, Bid, Bref, Weight, WeightKind},
};

/// Compute a `source_url` for `node` given its parse context.
///
/// Returns `None` when:
/// - The ancestor network node has no `metadata["git"]["remote_url"]` (git tracking
///   disabled, no recognised remote, or no git repo).
/// - `root_path` is empty.
///
/// The network node's `payload["git_remote_url"]` overrides the auto-detected remote,
/// allowing operators to hard-code a base URL (Gitea, Bitbucket, forks, etc.).
///
/// `source_line` is read directly from the `IRNode` — no intermediate encoding needed.
fn compute_source_url(node: &IRNode, ctx: &BeliefContext<'_>) -> Option<String> {
    // Look up the ancestor network node.
    let network_node = ctx.beliefbase().get(&NodeKey::Bid { bid: ctx.root_net })?;

    // Helper: look up a string field inside metadata["git"].
    let git_str = |key: &str| -> Option<&str> {
        network_node
            .metadata
            .get("git")
            .and_then(|g: &toml::Value| g.as_table())
            .and_then(|t: &toml::value::Table| t.get(key))
            .and_then(|v: &toml::Value| v.as_str())
    };

    // Determine the remote base URL:
    // 1. Explicit payload override on the network node takes precedence.
    // 2. Fall back to auto-detected remote_url stored in metadata["git"].
    let remote_base: String = if let Some(override_url) = network_node
        .payload
        .get("git_remote_url")
        .and_then(|v: &toml::Value| v.as_str())
    {
        if override_url.is_empty() {
            // Explicit empty string suppresses source_url for this network.
            return None;
        }
        override_url.to_string()
    } else {
        git_str("remote_url").map(|s| s.to_string())?
    };

    // Determine branch: prefer metadata["git"]["branch"], fall back to "HEAD".
    let branch = git_str("branch").unwrap_or("HEAD");

    // root_path is network-root-relative (e.g. "subnet1/file.md").
    // network_prefix is the git-workdir-relative path to the network directory
    // (e.g. "tests/network_1"). Joining them gives the git-root-relative path.
    // When network_prefix is absent or empty the network IS the git root, so
    // root_path is already correct.
    let root_path = &ctx.root_path;
    if root_path.is_empty() {
        return None;
    }
    let full_path = match git_str("network_prefix") {
        Some(prefix) if !prefix.is_empty() => format!("{}/{}", prefix, root_path),
        _ => root_path.clone(),
    };

    // Build the blob URL.
    let base = format!(
        "{}/blob/{}/{}",
        remote_base.trim_end_matches('/'),
        branch,
        full_path
    );

    // Append line anchor if available.
    if let Some(line) = node.source_line {
        Some(format!("{}#L{}", base, line))
    } else {
        Some(base)
    }
}

pub use pulldown_cmark;

/// A markdown event with optional source range information
type MdEventWithRange = (MdEvent<'static>, Option<Range<usize>>);

/// A queue of markdown events with range information
type MdEventQueue = VecDeque<MdEventWithRange>;

/// A proto node paired with its markdown event queue
type ProtoNodeWithEvents = (IRNode, MdEventQueue);

/// Magic heading ID that triggers section merging with the prior node.
///
/// When a heading carries the explicit anchor `{#__continue}`, the parser folds
/// it back into the preceding node's event stream instead of creating a new
/// section node. This lets authors use a heading as a visual separator in source
/// without introducing a new belief node.
///
/// The annotation is preserved in the written-back source for idempotency.
///
/// Example:
/// ```markdown
/// ## My Section
///
/// Some content.
///
/// ## Continued {#__continue}
///
/// More content that belongs to "My Section".
/// ```
pub const MAGIC_CONTINUE_ID: &str = "__continue";

/// Maps pulldown-cmark links to [href_to_nodekey].
///
/// If the link doesn't resolve in-page, returns a [NodeKey], and whether the link attributes
/// contain a link-specific title, otherwise None.
/// Strip a single layer of surrounding ASCII double-quotes from a URL string.
///
/// pulldown-cmark preserves literal quote characters in `dest_url` when the author writes a quoted
/// inline URL, e.g `[text]("bref://abc" "title")`. The resulting CowStr is `"bref://abc"` (with the
/// quote characters), which then fails to parse as a known NodeKey scheme and falls through to the
/// Extrnal/href path, producing a spurious href node.
///
/// This helper removes exactly one leading and trailing `"` if boath are present, leaving the URL
/// content intact for further processing.
/// Parse a Markdown string fragment and return upstream relations extracted from links.
///
/// This is a lightweight alternative to running `MdCodec::parse()` on a full document.
/// It reuses `LinkAccumulator` and `link_to_relation` but skips heading, frontmatter,
/// directive, and implements-block machinery entirely.
///
/// ## Usage
///
/// Call this from binary codecs (e.g. `XlsxCodec`) on individual cell values that
/// contain Markdown text. The returned `IntermediateRelation` list can be merged
/// directly into `IRNode::upstream`.
///
/// ```ignore
/// let relations = parse_markdown_relations(cell_text, &node.path, diagnostics);
/// node.upstream.extend(relations);
/// ```
///
/// ## Link semantics
///
/// All links are emitted with `WeightKind::Epistemic` by default (evidence-backed).
/// Callers that want `Pragmatic` weight should post-process the returned list.
/// Unresolvable links (those `link_to_relation` cannot map to a `NodeKey`) are
/// silently dropped — no diagnostic is emitted, consistent with markdown parse behaviour.
pub fn parse_markdown_relations(
    content: &str,
    base_path: &str,
    _diagnostics: &mut Vec<ParseDiagnostic>,
) -> Vec<IntermediateRelation> {
    let mut relations: Vec<IntermediateRelation> = Vec::new();
    let mut link_stack: Vec<LinkAccumulator> = Vec::new();

    for (event, offset) in MdParser::new_with_broken_link_callback(
        content,
        buildonomy_md_options(),
        Some(|link: BrokenLink<'_>| {
            let reference = link.reference.into_static();
            Some((reference.clone(), reference))
        }),
    )
    .into_offset_iter()
    {
        if let Some(link_data) = LinkAccumulator::new(event.borrow(), &Some(offset.clone())) {
            link_stack.push(link_data);
        }
        let mut push_relation = false;
        if let Some(link_data) = link_stack.last_mut() {
            push_relation = link_data.push(event.borrow(), &Some(offset.clone()));
        }
        if push_relation {
            let link_data = link_stack
                .pop()
                .expect("push_relation only true when stack non-empty");
            let mut node_keys = link_to_relation(
                &link_data.link_type,
                &link_data.rel_url,
                &CowStr::from(link_data.title_string()),
                &link_data.id,
            );
            if let Some(primary_key) = node_keys.first().cloned() {
                let node_key = primary_key.resolve_against(base_path);
                let title = link_data.title_string();
                let payload = if !title.is_empty()
                    && title != link_data.rel_url.as_ref()
                    && title != link_data.id.as_ref()
                {
                    let mut weight = Weight::default();
                    weight.set::<String>("title", title).ok();
                    Some(weight)
                } else {
                    None
                };
                let fallback_keys: Vec<NodeKey> = node_keys.drain(1..).collect();
                let mut relation =
                    IntermediateRelation::new(node_key, WeightKind::Epistemic, payload)
                        .with_fallback_keys(fallback_keys);
                if let Some(byte_offset) = link_data.range.as_ref().map(|r| r.start) {
                    relation = relation.with_location(byte_offset);
                }
                relations.push(relation);
            }
        }
    }

    relations
}

fn strip_url_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn link_to_relation(
    link_type: &LinkType,
    dest_url_cowstr: &CowStr<'_>,
    title: &CowStr<'_>,
    id: &CowStr<'_>,
) -> Vec<NodeKey> {
    let dest_url = strip_url_quotes(dest_url_cowstr.as_ref());
    match link_type {
        // Autolink like `<http://foo.bar/baz>`
        // with NodeKey::Path(href_net, http://foo.bar/baz)
        LinkType::Autolink => vec![href_to_nodekey(dest_url)],

        // Email address in autolink like `<john@example.org>`
        // with NodeKey::Path(href_net, email:john@example.org)
        LinkType::Email => vec![href_to_nodekey(&format!("email:{dest_url}"))],

        // Inline link like `[foo](bar)`
        // with NodeKey::Path(api, bar)
        LinkType::Inline => vec![href_to_nodekey(dest_url)],

        // Reference link like `[foo][bar]`
        // Reference without destination in the document, but resolved by the broken_link_callback
        LinkType::Reference => vec![],
        // Wikilinks: prioritize Id lookup, fall back to relative Path.
        // `[[engine-controller]]` produces:
        //   1. NodeKey::Id { net: default, id: "engine-controller" }  (title-slug match)
        //   2. NodeKey::Path { net: default, path: "engine-controller" } (relative file match)
        // The Path fallback resolves when the wikilink text matches a neighboring
        // filename whose title-derived slug differs (e.g. file `engine-controller.md`
        // with title "Vast Engine Controller" → slug `vast-engine-controller`).
        LinkType::WikiLink { .. } => {
            let id_key = href_to_nodekey(&format!("id:{dest_url}"));
            let path_key = NodeKey::Path {
                net: Bref::default(),
                path: to_anchor(dest_url),
            };
            vec![id_key, path_key]
        }
        LinkType::ReferenceUnknown => vec![href_to_nodekey(dest_url)],

        // Collapsed link like `[foo][]`
        // change to [[net:]title]
        // with NodeKey::?(foo)
        // Collapsed link without destination in the document, but resolved by the broken_link_callback
        LinkType::Collapsed => vec![],
        LinkType::CollapsedUnknown => vec![href_to_nodekey(title)],
        // Shortcut link like `[foo]`
        // change to [net:title]
        // with NodeKey::?(foo)
        LinkType::Shortcut => vec![href_to_nodekey(dest_url)],
        // Shortcut without destination in the document, but resolved by the broken_link_callback
        LinkType::ShortcutUnknown => vec![href_to_nodekey(id)],
    }
}

#[derive(Debug, Clone)]
struct LinkAccumulator {
    link_type: LinkType,
    rel_url: CowStr<'static>,
    id: CowStr<'static>,
    range: Option<Range<usize>>,
    title_events: Vec<MdEvent<'static>>,
    is_image: bool,
    title: CowStr<'static>,
}

impl LinkAccumulator {
    fn new(event: &MdEvent<'_>, range: &Option<Range<usize>>) -> Option<LinkAccumulator> {
        match event {
            MdEvent::Start(MdTag::Link {
                link_type,
                dest_url,
                id,
                title,
                ..
            }) => Some(LinkAccumulator {
                link_type: *link_type,
                rel_url: dest_url.clone().into_static(),
                id: id.clone().into_static(),
                range: range.clone(),
                title_events: vec![],
                is_image: false,
                title: title.clone().into_static(),
            }),
            MdEvent::Start(MdTag::Image {
                link_type,
                dest_url,
                id,
                title,
            }) => Some(LinkAccumulator {
                link_type: *link_type,
                rel_url: dest_url.clone().into_static(),
                id: id.clone().into_static(),
                range: range.clone(),
                title_events: vec![],
                is_image: true,
                title: title.clone().into_static(),
            }),
            _ => None,
        }
    }

    // Returns whether event is a [MdTagEnd::Link] or [MdTagEnd::Image]
    fn push(&mut self, event: &MdEvent<'_>, range: &Option<Range<usize>>) -> bool {
        match event {
            MdEvent::End(MdTagEnd::Link) if !self.is_image => return true,
            MdEvent::End(MdTagEnd::Image) if self.is_image => return true,
            _ => {}
        }
        self.title_events.push(event.clone().into_static());
        if self.range.is_none() {
            self.range = range.clone();
        } else if let Some(self_range) = self.range.as_mut() {
            if let Some(pushed_range) = range {
                self_range.end = pushed_range.end;
            }
        }
        false
    }

    fn title_string(&self) -> String {
        let title_string = self
            .title_events
            .iter()
            .fold(String::new(), |mut text, event| {
                if let MdEvent::Text(cow_str) = event {
                    if !text.is_empty() {
                        text += " ";
                    }
                    text += &cow_str
                        .split("\n")
                        .map(|line| line.trim().to_string())
                        .collect::<Vec<String>>()
                        .join(" ");
                }
                text
            });
        title_string
    }
}

/// Parsed components from a markdown link title attribute.
///
/// Title attribute format: `"bref://abc123 {\"auto_title\":true} User Words"`
#[derive(Debug, Clone, PartialEq)]
struct TitleAttributeParts {
    /// Bref extracted from title attribute (e.g., "bref://abc123")
    bref: Option<Bref>,
    /// Whether link text should auto-update when target title changes
    auto_title: bool,
    /// Relation type encoded as "{weight_kind},{ref_role}" (e.g. "pragmatic,source").
    /// Set at inject_context time when the link resolves to a known relation.
    /// Used at render time to emit a CSS class on the `<a>` tag.
    rel: Option<String>,
    /// Any additional user-provided words in the title attribute
    user_words: Option<String>,
}

/// Builds a title attribute for HTML links containing bref and optional metadata.
///
/// The title attribute format is: `bref://[bref] [metadata] [user_words]`
/// where metadata and user_words are optional.
///
/// # Arguments
/// * `bref` - The bref string (should already include "bref://" prefix)
/// * `auto_title` - If true, adds `auto_title` to the JSON config blob
/// * `rel` - Optional relation type string e.g. `"pragmatic,source"`. Encoded in the
///   JSON config blob and extracted at render time to set a CSS class on the `<a>` tag.
/// * `user_words` - Optional user-provided text to append
///
/// # Examples
/// ```
/// use noet_core::codec::md::build_title_attribute;
/// let attr = build_title_attribute("bref://abc123", false, None, None);
/// assert_eq!(attr, "bref://abc123");
///
/// let attr = build_title_attribute("bref://abc123", true, None, Some("My Note"));
/// assert_eq!(attr, "bref://abc123 {\"auto_title\":true} My Note");
///
/// let attr = build_title_attribute("bref://abc123", false, Some("pragmatic,source"), None);
/// assert_eq!(attr, "bref://abc123 {\"rel\":\"pragmatic,source\"}");
/// ```
pub fn build_title_attribute(
    bref: &str,
    auto_title: bool,
    rel: Option<&str>,
    user_words: Option<&str>,
) -> String {
    let mut parts = vec![bref.to_string()];

    // Build JSON config blob if any config fields are set.
    if auto_title || rel.is_some() {
        let mut config_parts = Vec::new();
        if auto_title {
            config_parts.push("\"auto_title\":true".to_string());
        }
        if let Some(r) = rel {
            config_parts.push(format!("\"rel\":\"{}\"", r));
        }
        parts.push(format!("{{{}}}", config_parts.join(",")));
    }

    if let Some(words) = user_words {
        parts.push(words.to_string());
    }

    parts.join(" ")
}

/// Parse a markdown link title attribute to extract Bref, config, and user words.
///
/// Format: `"bref://abc123 {\"auto_title\":true} User Description"`
///
/// # Examples
///
/// ```text
/// let parts = parse_title_attribute("bref://abc123");
/// assert!(parts.bref.is_some());
/// assert_eq!(parts.auto_title, false);
/// assert_eq!(parts.user_words, None);
///
/// let parts = parse_title_attribute("bref://abc123 {\"auto_title\":true} My Note");
/// assert_eq!(parts.auto_title, true);
/// assert_eq!(parts.user_words, Some("My Note".to_string()));
/// ```
///
/// Note: This function is tested via unit tests in the `tests` module.
fn parse_title_attribute(title: &str) -> TitleAttributeParts {
    let mut bref = None;
    let mut auto_title = false;
    let mut rel: Option<String> = None;
    let mut word_parts = Vec::new();
    let mut in_json = false;
    let mut json_buffer = String::new();

    for word in title.split_whitespace() {
        if word.starts_with("bref://") {
            // Parse Bref from URL-style reference
            let bref_str = word.trim_start_matches("bref://");
            if let Ok(parsed_bref) = Bref::try_from(bref_str) {
                bref = Some(parsed_bref);
            }
        } else if word.starts_with("bid://") {
            // Parse Bref from BID URL-style reference
            let bid_str = word.trim_start_matches("bid://");
            if let Ok(parsed_bid) = Bid::try_from(bid_str) {
                bref = Some(parsed_bid.bref());
            }
        } else if word.starts_with('{') {
            // Start of JSON config
            in_json = true;
            json_buffer.push_str(word);
            if word.ends_with('}') {
                // Single-word JSON object
                if let Ok(config) = serde_json::from_str::<serde_json::Value>(&json_buffer) {
                    if let Some(auto_val) = config.get("auto_title") {
                        auto_title = auto_val.as_bool().unwrap_or(false);
                    }
                    if let Some(rel_val) = config.get("rel").and_then(|v| v.as_str()) {
                        rel = Some(rel_val.to_string());
                    }
                }
                in_json = false;
                json_buffer.clear();
            }
        } else if in_json {
            // Continuation of multi-word JSON
            json_buffer.push(' ');
            json_buffer.push_str(word);
            if word.ends_with('}') {
                if let Ok(config) = serde_json::from_str::<serde_json::Value>(&json_buffer) {
                    if let Some(auto_val) = config.get("auto_title") {
                        auto_title = auto_val.as_bool().unwrap_or(false);
                    }
                    if let Some(rel_val) = config.get("rel").and_then(|v| v.as_str()) {
                        rel = Some(rel_val.to_string());
                    }
                }
                in_json = false;
                json_buffer.clear();
            }
        } else {
            // Regular word - part of user description
            word_parts.push(word);
        }
    }

    let user_words = if word_parts.is_empty() {
        None
    } else {
        Some(word_parts.join(" "))
    };

    TitleAttributeParts {
        bref,
        auto_title,
        rel,
        user_words,
    }
}

fn check_for_link_and_push(
    events_in: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
    ctx: &BeliefContext<'_>,
    doc_abs_path: &str,
    _source: &str,
    events_out: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
    stop_event: Option<&MdEvent<'_>>,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> bool {
    let mut changed = false;
    let mut collector_stack: Vec<LinkAccumulator> = Vec::new();
    let mut maybe_event = events_in.pop_front();
    while let Some((event, range)) = maybe_event.take() {
        let stop_event_match = stop_event.filter(|e| **e == event).is_some();
        let mut process_link = false;
        if let Some(link_accumulator) = LinkAccumulator::new(&event, &range) {
            collector_stack.push(link_accumulator);
        } else if let Some(link_accumulator) = collector_stack.last_mut() {
            process_link = link_accumulator.push(&event, &range);
        }

        // Don't push events if we're collecting a link
        if collector_stack.is_empty() {
            events_out.push_back((event, range));
        } else if process_link {
            let mut link_data = collector_stack
                .pop()
                .expect("Process_link is only true if collector stack is not empty.");

            let link_text = link_data.title_string();

            // Parse the title attribute to check for existing Bref
            let title_parts = parse_title_attribute(link_data.title.as_ref());

            // Determine the key to use for matching
            // If title attribute contains a Bref, prioritize it
            let key = if let Some(bref) = &title_parts.bref {
                NodeKey::Bref { bref: *bref }
            } else {
                // Otherwise parse from normalized dest_url
                let title = CowStr::from(link_text.clone());
                let parsed_keys = link_to_relation(
                    &link_data.link_type,
                    &link_data.rel_url,
                    &title,
                    &link_data.id,
                );
                if let Some(parsed_key) = parsed_keys.into_iter().next() {
                    parsed_key
                } else {
                    // Can't parse - leave link/image unchanged
                    link_data.link_type = match title.is_empty() || title == link_data.id {
                        true => LinkType::Shortcut,
                        false => LinkType::Reference,
                    };
                    let start_event = if link_data.is_image {
                        MdEvent::Start(MdTag::Image {
                            link_type: link_data.link_type,
                            dest_url: link_data.rel_url,
                            title: link_data.title,
                            id: link_data.id,
                        })
                    } else {
                        MdEvent::Start(MdTag::Link {
                            link_type: link_data.link_type,
                            dest_url: link_data.rel_url,
                            title: link_data.title,
                            id: link_data.id,
                        })
                    };
                    events_out.push_back((start_event, None));
                    for title_event in link_data.title_events.into_iter() {
                        events_out.push_back((title_event, None));
                    }

                    let new_range = match (link_data.range, range) {
                        (Some(link_range), Some(link_end_range)) => {
                            Some(link_range.start..link_end_range.end)
                        }
                        (Some(link_range), _) => Some(link_range.clone()),
                        (_, Some(link_end_range)) => Some(link_end_range.clone()),
                        _ => None,
                    };
                    let end_event = if link_data.is_image {
                        MdEvent::End(MdTagEnd::Image)
                    } else {
                        MdEvent::End(MdTagEnd::Link)
                    };
                    events_out.push_back((end_event, new_range));

                    if stop_event_match {
                        break;
                    }
                    maybe_event = events_in.pop_front();
                    continue;
                }
            };

            // Normalize the key against the repo-relative doc path, then regularize
            let normalized_abs = key.resolve_against(doc_abs_path);

            // Whether this key needs repo-relative path alignment to compute root_abs_path.
            // External links (href_namespace: https://, mailto:, etc.) and asset-namespace
            // keys are already fully scoped — they don't require root_abs_path at all.
            // Skipping the alignment check for them prevents spurious mismatches when
            // ctx.root_path is set incorrectly (e.g. due to a stale PathMap entry).
            let needs_path_alignment = !matches!(
                &normalized_abs,
                NodeKey::Path { net, .. }
                    if *net == crate::properties::href_namespace().bref()
                    || *net == crate::properties::asset_namespace().bref()
            );

            // ctx.root_path may differ from doc_abs_path (node.path) in several ways:
            //   1. Network dir form:  doc="/tmp/.../subnet1"   ctx="subnet1/index.md"
            //   2. Index file form:   doc="/tmp/.../subnet1/index.md"  ctx="subnet1"
            //   3. Extensionless key: doc="/tmp/.../test.md"   ctx="test"
            //
            // We need to find root_abs_path = the absolute prefix that, when concatenated
            // with ctx_stem, gives doc_stem (i.e. doc_stem.ends_with(ctx_stem)).
            //
            // Strategy: reduce both paths to their containing directory using AnchorPath.
            //
            // ctx_filepath comes from ctx.root_path, which may be:
            //   - a real file path:     "array/symbol.iterator/index.md"
            //   - a dotted dir name:    "array/symbol.iterator"   (document node, no index.md)
            //   - a plain dir name:     "array"
            //
            // AnchorPath::new() mis-classifies "array/symbol.iterator" as a file with
            // extension "iterator", so dir() returns only "array" — losing the last component.
            // Use new_dir() + drop_index_file() for the same treatment as doc_stem below.
            //
            // doc_abs_path is the absolute filesystem path of the document being parsed.
            // It may be a real file (".../array/index.md") or a dotted directory name
            // (".../array/symbol.iterator") that AnchorPath::new() would mis-classify as
            // a file with extension "iterator". Using new_dir() unconditionally forces
            // directory semantics, giving dir() = the full path. Then drop_index_file()
            // strips a trailing "/index.md" (or bare "index.anything") to normalise the
            // file-path case back to the parent directory, matching what ctx_stem produces.
            //
            //   ".../array/index.md"       → new_dir → dir = ".../array/index.md"
            //                              → drop_index_file → ".../array"            ✓
            //   ".../array/symbol.iterator"→ new_dir → dir = ".../array/symbol.iterator"
            //                              → drop_index_file → ".../array/symbol.iterator" ✓
            //   ".../array"                → new_dir → dir = ".../array"
            //                              → drop_index_file → ".../array"            ✓
            //
            // AnchorPath is drive-letter-aware (C:/... is a plain absolute path, not a
            // URL schema), so dir() preserves the drive prefix correctly.
            // Forward slashes are always used here (os_path_to_string guarantees that).

            /// Strip a trailing "index" file segment (with any extension, e.g. "index.md",
            /// "index.html") from an absolute path, returning the parent directory.
            /// A bare "index" (no extension) is also stripped.
            /// Non-index last components are returned unchanged.
            ///
            /// "…/array/index.md"  → "…/array"
            /// "…/array"           → "…/array"   (unchanged)
            /// "index.md"          → ""           (repo root)
            fn drop_index_file(p: &str) -> &str {
                let last_slash = p.rfind('/').map(|i| i + 1).unwrap_or(0);
                let last_component = &p[last_slash..];
                if last_component == "index" || last_component.starts_with("index.") {
                    // Strip the last component and the preceding slash (if any).
                    &p[..last_slash.saturating_sub(1)]
                } else {
                    p
                }
            }

            let ctx_filepath = AnchorPath::new(&ctx.root_path).filepath();
            let ctx_stem = drop_index_file(AnchorPath::new_dir(ctx_filepath).dir());
            let doc_stem = drop_index_file(AnchorPath::new_dir(doc_abs_path).dir());

            if needs_path_alignment && !doc_stem.ends_with(ctx_stem) {
                // Mismatch means we cannot safely compute root_abs_path.
                // Emit the link unchanged (same as the "Can't parse" path above) and
                // record a diagnostic rather than panicking or producing garbage paths.
                tracing::warn!(
                    "[check_for_link_and_push] Path mismatch: proto abs path \"{doc_abs_path}\" \
                    does not align with ctx repo-relative path \"{ctx_filepath}\". \
                    Leaving link unchanged."
                );
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "Could not rewrite link: document path \"{doc_abs_path}\" \
                    does not align with context path \"{ctx_filepath}\""
                )));
                let link_text_cow = CowStr::from(link_text.clone());
                link_data.link_type = match link_text.is_empty() || link_text_cow == link_data.id {
                    true => LinkType::Shortcut,
                    false => LinkType::Reference,
                };
                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: link_data.title,
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: link_data.title,
                        id: link_data.id,
                    })
                };
                events_out.push_back((start_event, None));
                for title_event in link_data.title_events.into_iter() {
                    events_out.push_back((title_event, None));
                }
                let new_range = match (link_data.range, range) {
                    (Some(link_range), Some(link_end_range)) => {
                        Some(link_range.start..link_end_range.end)
                    }
                    (Some(link_range), _) => Some(link_range.clone()),
                    (_, Some(link_end_range)) => Some(link_end_range.clone()),
                    _ => None,
                };
                let end_event = if link_data.is_image {
                    MdEvent::End(MdTagEnd::Image)
                } else {
                    MdEvent::End(MdTagEnd::Link)
                };
                events_out.push_back((end_event, new_range));
                if stop_event_match {
                    break;
                }
                maybe_event = events_in.pop_front();
                continue;
            };
            // root_abs_path is the absolute prefix before the repo-relative portion.
            // For non-repo-scoped keys (href, asset) we never reach here because
            // needs_path_alignment is false and the mismatch guard above continues.
            // Use an empty root_abs_path as a safe fallback for those cases should
            // the guard ever be bypassed.
            let root_abs_path = if needs_path_alignment {
                &doc_stem[0..(doc_stem.len() - ctx_stem.len())]
            } else {
                ""
            };
            let regularized =
                normalized_abs.regularize_unchecked(ctx.root_net, &ctx.root_path, root_abs_path);

            // Mirror push_relation's dir-reclassification: if the regularized key is a
            // repo-namespace path that resolves to a non-network directory on disk,
            // reclassify it to asset_namespace so the ctx.sources() lookup below finds
            // the asset node that push_relation already stored under that key.
            let regularized = match &regularized {
                NodeKey::Path { net, path } if *net == ctx.root_net.bref() || net.is_default() => {
                    use crate::paths::string_to_os_path;
                    let abs_path = string_to_os_path(
                        &crate::paths::AnchorPathBuf::from(root_abs_path.to_string())
                            .as_anchor_path()
                            .join(path),
                    );
                    if abs_path.is_dir()
                        && crate::codec::network::detect_network_file(&abs_path).is_none()
                    {
                        NodeKey::Path {
                            net: crate::properties::asset_namespace().bref(),
                            path: path.clone(),
                        }
                    } else {
                        regularized
                    }
                }
                _ => regularized,
            };

            let keys = [regularized];

            // Check sources (upstream) for the link target. Assets and document links are sources
            // (upstream)
            let sources = ctx.sources();

            let maybe_keyed_relation = keys.iter().find_map(|link_key| {
                // First check sources (upstream relations - documents this node sources from (links
                // to))
                let direct_match = sources.iter().find(|rel| {
                    rel.other
                        .keys(Some(ctx.root_net), None, ctx.beliefbase())
                        .iter()
                        .chain(
                            rel.other
                                .keys(Some(rel.home_net), None, ctx.beliefbase())
                                .iter(),
                        )
                        .any(|ctx_source_key| ctx_source_key == link_key)
                });
                if direct_match.is_some() {
                    return direct_match;
                }
                // Fallback: resolve the link key to a BID through the BeliefBase,
                // then match by BID.  This handles two cases:
                //
                // 1. Cross-document links where the target node was added to doc_bb
                //    via NodeUpsert (from push_relation) but has no PathMap entry
                //    (no Section edge to a parent network).  Without a PathMap entry,
                //    node.keys() returns only BID/ID keys — not the path key the
                //    link URL resolves to.  The direct key match above misses.
                //
                // 2. Href-aliased links where push_relation resolved the URL to
                //    the content node via the alias PathMap and created an edge
                //    directly to that content node.  The content node's keys()
                //    won't include the href URL.
                if let Some(resolved_node) = ctx.beliefbase().get(link_key) {
                    return sources
                        .iter()
                        .find(|rel| rel.other.bid == resolved_node.bid);
                }
                None
            });

            // Href-alias resolution for nodes not found in sources.
            // This covers self-references (a node's Jira link pointing to itself)
            // and cross-references where push_relation resolved the alias but the
            // edge topology doesn't place the target in ctx.sources().
            let href_resolved_node = if maybe_keyed_relation.is_none() {
                match &keys[0] {
                    NodeKey::Path { net, .. } if *net == href_namespace().bref() => {
                        ctx.beliefbase().get(&keys[0]).filter(|n| {
                            // Only annotate if the resolved node is a real content node,
                            // not an External|Trace stub (which would just be the href
                            // leaf node, not useful for navigation).
                            !n.kind.contains(BeliefKind::External)
                        })
                    }
                    _ => None,
                }
            } else {
                None
            };

            if let Some(relation) = maybe_keyed_relation {
                // Generate canonical format: [text](relative/path.md#anchor "bref://abc config")

                let is_href_aliased_link = matches!(
                    &keys[0],
                    NodeKey::Path { net, .. } if *net == href_namespace().bref()
                );
                let relative_path = if relation.home_net == href_namespace() {
                    relation.root_path.clone()
                } else if is_href_aliased_link {
                    // href-aliased link: the content node was resolved via its URL
                    // alias. Keep the original URL as the href rather than rewriting
                    // it to a document-relative path.
                    link_data.rel_url.to_string()
                } else {
                    // 1. Calculate relative path from source to target
                    // Strip any existing anchor from home_path to avoid double anchors
                    let ctx_ap = AnchorPath::from(&ctx.root_path);

                    let mut relative_path = ctx_ap.path_to(&relation.root_path, true);
                    let relative_ap = AnchorPath::from(&relative_path);

                    if relation.other.kind.is_anchor() {
                        let anchor = relation.other.id.anchor();
                        if !anchor.is_empty() {
                            relative_path = relative_ap.join(as_anchor(anchor)).into();
                        }
                    }
                    relative_path
                };

                // 3. Build title attribute: "bref://abc123 {config} user words"
                let bref_str = format!("bref://{}", relation.other.bid.bref());

                // Determine if auto_title should be enabled
                // Default to false unless link text matches target title
                let should_auto_title = if title_parts.auto_title {
                    // User explicitly set auto_title
                    true
                } else if !link_text.is_empty() && link_text == relation.other.title {
                    // Link text matches target title - enable auto update
                    true
                } else {
                    // User provided custom text - don't auto update
                    false
                };

                // Derive rel string from the matched relation's weight kinds and direction.
                // Links found in ctx.sources() have ref_role=source (referent is source).
                // An edge may carry multiple WeightKinds — encode one "kind:role" pair per
                // kind, space-separated (e.g. "pragmatic:source epistemic:source").
                // At render time each pair becomes its own CSS class.
                let rel_str: Option<String> = {
                    let pairs: Vec<String> = relation
                        .weight
                        .weights
                        .keys()
                        .map(|k| {
                            let kind_str = match k {
                                WeightKind::Pragmatic => "pragmatic",
                                WeightKind::Epistemic => "epistemic",
                                WeightKind::Section => "section",
                            };
                            // All links in check_for_link_and_push come from ctx.sources(),
                            // so the referent role is always "source" here.
                            format!("{kind_str}:source")
                        })
                        .collect();
                    if pairs.is_empty() {
                        None
                    } else {
                        Some(pairs.join(" "))
                    }
                };

                let new_title_attr = build_title_attribute(
                    &bref_str,
                    should_auto_title,
                    rel_str.as_deref(),
                    title_parts.user_words.as_deref(),
                );

                // 4. Determine link text
                let new_link_text = if should_auto_title {
                    // Use target's current title
                    relation.other.title.clone()
                } else {
                    // Keep user's original text
                    link_text.clone()
                };

                // 5. Check if link changed
                if link_data.rel_url.as_ref() != relative_path
                    || link_data.title.as_ref() != new_title_attr
                    || link_text != new_link_text
                {
                    changed = true;
                    link_data.rel_url = CowStr::from(relative_path);
                    link_data.title_events = vec![MdEvent::Text(CowStr::from(new_link_text))];
                }

                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: CowStr::from(new_title_attr),
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: CowStr::from(new_title_attr),
                        id: link_data.id,
                    })
                };
                events_out.push_back((start_event, None));
            } else if let Some(resolved_node) = href_resolved_node {
                // Href-aliased link to a known internal node that wasn't in sources
                // (self-reference or different relationship topology). Annotate with
                // bref so the SPA viewer can navigate to the node. Keep the original
                // URL as the href.
                let bref_str = format!("bref://{}", resolved_node.bid.bref());
                let new_title_attr = build_title_attribute(
                    &bref_str,
                    false,
                    None,
                    title_parts.user_words.as_deref(),
                );

                if link_data.title.as_ref() != new_title_attr {
                    changed = true;
                }

                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: CowStr::from(new_title_attr),
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: CowStr::from(new_title_attr),
                        id: link_data.id,
                    })
                };
                events_out.push_back((start_event, None));
            } else {
                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: link_data.title,
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.rel_url,
                        title: link_data.title,
                        id: link_data.id,
                    })
                };
                events_out.push_back((start_event, None));
            }

            // Push link text events
            for title_event in link_data.title_events.into_iter() {
                events_out.push_back((title_event, None));
            }

            let new_range = match (link_data.range, range) {
                (Some(link_range), Some(link_end_range)) => {
                    Some(link_range.start..link_end_range.end)
                }
                (Some(link_range), _) => Some(link_range.clone()),
                (_, Some(link_end_range)) => Some(link_end_range.clone()),
                _ => None,
            };
            let end_event = if link_data.is_image {
                MdEvent::End(MdTagEnd::Image)
            } else {
                MdEvent::End(MdTagEnd::Link)
            };
            events_out.push_back((end_event, new_range));
        }

        if stop_event_match {
            break;
        }
        maybe_event = events_in.pop_front();
    }
    changed
}

fn find_frontmatter_end<'a>(
    events: &VecDeque<(MdEvent<'a>, Option<Range<usize>>)>,
) -> Option<usize> {
    let mut header_end = None;
    let mut meta_end = None;

    for (idx, (event, _)) in events.iter().enumerate() {
        match event {
            MdEvent::End(MdTagEnd::Heading(_)) if header_end.is_none() => {
                header_end = Some(idx + 1)
            }
            MdEvent::End(MdTagEnd::MetadataBlock(_)) => {
                meta_end = Some(idx + 1);
                break;
            }
            _ => {}
        }
    }
    if meta_end.is_some() {
        meta_end
    } else {
        header_end
    }
}

/// Title should be the first MdEvent if there is one. Metadata block should
/// start right after the title, or be the first event if there is no title.
fn update_or_insert_frontmatter(
    events: &mut MdEventQueue,
    node_string: &str,
) -> Result<bool, BuildonomyError> {
    let mut changed = false;
    let mut header_events = VecDeque::new();
    let mut metadata_events = VecDeque::new();
    let mut toml_string_range: Option<Range<usize>> = None;

    let starts_with_title = events
        .front()
        .map(|(event, _)| matches!(event, MdEvent::Start(MdTag::Heading { .. })))
        .unwrap_or(false);

    // Push title events onto our temporary vecdeque, and map title ranges to
    // our toml_string_range variable
    if starts_with_title {
        while let Some((event, range)) = events.pop_front() {
            let end = match &event {
                // Track range for text-like content
                MdEvent::Text(_) | MdEvent::InlineHtml(_) | MdEvent::Code(_) => {
                    if let Some(ref title_range) = range {
                        toml_string_range = Some(title_range.end..title_range.end)
                    }
                    false
                }
                MdEvent::Start(MdTag::Heading { .. }) => false,
                MdEvent::End(MdTagEnd::Heading(_)) => true,
                // Accept all other inline elements (emphasis, strong, links, images, etc.)
                // without warnings - these are valid CommonMark inside headings
                _ => false,
            };
            header_events.push_back((event, range));
            if end {
                break;
            }
        }
    }

    let has_metadata = events
        .front()
        .map(|(event, _)| matches!(event, MdEvent::Start(MdTag::MetadataBlock(_))))
        .unwrap_or(false);

    if has_metadata {
        let mut toml_string = String::new();
        while let Some((event, range)) = events.pop_front() {
            let end = match &event {
                MdEvent::Text(ref cow_str)
                | MdEvent::InlineHtml(ref cow_str)
                | MdEvent::Code(ref cow_str) => {
                    toml_string += cow_str.as_ref();
                    toml_string_range = match (&toml_string_range, &range) {
                        (Some(toml_range), Some(text_range)) => {
                            Some(toml_range.start..text_range.end)
                        }
                        (Some(toml_range), _) => Some(toml_range.clone()),
                        (_, Some(text_range)) => Some(text_range.clone()),
                        _ => None,
                    };
                    false
                }
                MdEvent::Start(MdTag::MetadataBlock(_)) => false,
                MdEvent::End(MdTagEnd::MetadataBlock(_)) => true,
                // Metadata blocks should only contain text-like content,
                // but accept other events without warning for robustness
                _ => false,
            };
            metadata_events.push_back((event, range));
            if end {
                break;
            }
        }
        if node_string != toml_string {
            // tracing::debug!(
            //     "Existing toml string does not match expected toml.\nexpected_toml:\n\t{}\nexisting_toml\n\t{}",
            //     expected_toml_string.replace("\n", "\n\t"),
            //     toml_string.replace("\n", "\n\t")
            // );
            changed = true;
        }
    } else {
        changed = true;
    }

    if changed {
        header_events.push_back((
            MdEvent::Start(MdTag::MetadataBlock(MetadataBlockKind::YamlStyle)),
            None,
        ));
        header_events.push_back((
            MdEvent::Text(CowStr::from(node_string.to_string())),
            toml_string_range,
        ));
        header_events.push_back((
            MdEvent::End(MdTagEnd::MetadataBlock(MetadataBlockKind::YamlStyle)),
            None,
        ));
    } else {
        header_events.append(&mut metadata_events);
    }
    let mut rest = replace(events, header_events);
    events.append(&mut rest);
    Ok(changed)
}

/// Parse sections field from frontmatter into flat metadata map.
/// Returns HashMap<NodeKey, TomlTable> for matching against heading nodes.
fn parse_sections_metadata(sections: &toml_edit::Item) -> HashMap<NodeKey, toml_edit::Table> {
    let mut metadata = HashMap::new();

    if let Some(table) = sections.as_table() {
        for (key_str, value) in table.iter() {
            // Parse key as NodeKey
            match NodeKey::from_str(key_str) {
                Ok(node_key) => {
                    // Extract value as TomlTable
                    if let Some(value_table) = value.as_table() {
                        metadata.insert(node_key, value_table.clone());
                    } else {
                        tracing::warn!(
                            "[parse_sections_metadata] Could not process {:?} as a table!",
                            value
                        )
                    }
                }
                Err(e) => {
                    tracing::warn!("Could not parse section key {}. Error: {}", key_str, e);
                }
            }
        }
    }
    metadata
}

/// Find metadata match for a IRNode with priority: BID > Anchor > Title.
///
/// Returns a reference to the matching metadata table if found.
fn find_metadata_match<'a>(
    node: &IRNode,
    metadata: &'a HashMap<NodeKey, toml_edit::Table>,
) -> Option<(NodeKey, &'a toml_edit::Table)> {
    // Priority 1: Match by BID (most explicit)
    if let Some(bid_value) = node.document.get("bid") {
        if let Some(bid_str) = bid_value.as_str() {
            if let Ok(bid) = Bid::try_from(bid_str) {
                let bid_key = NodeKey::Bid { bid };
                if let Some(meta) = metadata.get(&bid_key) {
                    return Some((bid_key, meta));
                }
            }
        }
    }

    // Priority 2: Match by anchor (medium specificity)
    if let Some(anchor) = node.id() {
        // Try as Id variant (anchors are IDs within a document)
        let anchor_key = NodeKey::Id {
            net: Bref::default(),
            id: anchor,
        };
        if let Some(meta) = metadata.get(&anchor_key) {
            return Some((anchor_key, meta));
        }
    }

    // Priority 3: Match by title anchor (least specific)
    // Use Id variant since titles are only guaranteed unique for documents
    if let Some(title_value) = node.document.get("title") {
        if let Some(title) = title_value.as_str() {
            let anchor = to_anchor(title);
            let id_key = NodeKey::Id {
                net: Bref::default(),
                id: anchor,
            };
            if let Some(meta) = metadata.get(&id_key) {
                return Some((id_key, meta));
            }
        }
    }

    None
}

/// Merge metadata from a TomlTable into a IRNode's document.
/// Preserves existing fields, adds new fields from metadata.
/// Merges fields from `metadata` into `node.document`, skipping any key already present.
/// Returns `true` if at least one new field was inserted, `false` if nothing changed.
fn merge_metadata_into_node(node: &mut IRNode, metadata: &toml_edit::Table) -> bool {
    let mut changed = false;
    for (key, value) in metadata.iter() {
        // Don't overwrite existing fields in the node
        if !node.document.contains_key(key) {
            node.document.insert(key, value.clone());
            changed = true;
        }
    }
    changed
}

pub fn to_html(content: &str, output: &mut String) -> Result<(), BuildonomyError> {
    let parser = MdParser::new_ext(content, buildonomy_md_options());
    pulldown_cmark::html::write_html_fmt(output, parser)?;
    Ok(())
}

fn read_frontmatter<R: Read>(reader: R) -> std::io::Result<Option<String>> {
    let mut buf_reader = BufReader::new(reader);
    let mut frontmatter = String::new();
    let mut line = String::new();

    // Skip leading blank lines, then check for frontmatter delimiter
    loop {
        let bytes_read = buf_reader.read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF before finding any content
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
        line.clear();
    }
    if line.trim() != "---" {
        // No frontmatter
        return Ok(None);
    }

    // Read until we hit the second delimiter
    loop {
        line.clear();
        let bytes_read = buf_reader.read_line(&mut line)?;

        if bytes_read == 0 {
            // EOF before closing delimiter
            return Ok(None);
        }

        if line.trim() == "---" {
            // Found closing delimiter - return frontmatter without the delimiter
            return Ok(Some(frontmatter));
        }

        frontmatter.push_str(&line);
    }
}

/// Extract an inline anchor ID from a text fragment.
///
/// Scans for `{#id}` where `id` matches the [`ANCHOR_CHAR_CLASS`] output alphabet
/// of [`to_anchor`]. Returns the first match's ID string (unnormalized — caller
/// should apply `to_anchor()` if needed).
///
/// This is used to detect inline anchors in non-heading blocks (paragraphs, list
/// items) where pulldown-cmark does not parse `{#id}` natively.
fn extract_inline_anchor(s: &str) -> Option<String> {
    static RE: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(&format!(r"\{{#({}+)\}}", crate::paths::ANCHOR_CHAR_CLASS))
            .expect("inline anchor regex")
    });
    RE.captures(s).map(|caps| caps[1].to_string())
}

/// Regex that matches `{#id}` including optional surrounding whitespace, for
/// stripping from HTML output. Uses the same [`ANCHOR_CHAR_CLASS`] as
/// [`extract_inline_anchor`].
/// Strip the `{#id}` marker and any leading whitespace before it, but preserve
/// trailing whitespace so that `{#id} text` renders as ` text` (the space
/// between the anchor and the content is kept).
static STRIP_INLINE_ANCHOR_RE: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(&format!(r"\s*\{{#{}+\}}", crate::paths::ANCHOR_CHAR_CLASS))
        .expect("strip anchor regex")
});

/// Scan buffered proto events for an inline anchor `{#id}`.
///
/// Searches `Text` and `InlineHtml` events from index `from` onward.
/// Returns the first anchor ID found, or `None`.
fn scan_for_inline_anchor(events: &MdEventQueue, from: usize) -> Option<String> {
    for (event, _range) in events.iter().skip(from) {
        match event {
            MdEvent::Text(s) | MdEvent::InlineHtml(s) => {
                if let Some(id) = extract_inline_anchor(s) {
                    return Some(id);
                }
            }
            _ => {}
        }
    }
    None
}

/// Dispatch a relation context directive by name and args, updating the stack and
/// session verb registry.
///
/// This is a free function (not a method) so it can be called inside the `MdParser`
/// loop without conflicting with the immutable borrow on `self.content`. It handles
/// only relation context operations — pipeline directives (`{network_children}`,
/// `{maps_to}`, etc.) are handled separately in the `CodeBlock` arm.
///
/// Handles:
/// - `{end}` — pop the relation context stack; warn if empty.
/// - `{relation}name=X, kind=K, ref=R` — register custom verb in session registry.
/// - `{relation}kind=K, ref=R` — push precise relation context onto stack.
/// - known relation verb (e.g. `{uses}`, `{implements}`) — push context from registry.
///
/// `label` is the directive text as written (used in diagnostic messages).
fn dispatch_relation_directive(
    name: &str,
    args: &str,
    label: &str,
    relation_context_stack: &mut Vec<(WeightKind, ReferenceRole, String)>,
    session_verb_registry: &mut HashMap<String, (WeightKind, ReferenceRole)>,
    diagnostics: &mut Vec<ParseDiagnostic>,
) {
    // Non-verb directives ({network_children}, {requirements_table}, {maps_to},
    // {query}, …) are valid known directives but produce no relation context.
    // Silently skip any DIRECTIVES entry that lacks weight_kind (i.e. is not a
    // relation verb), EXCEPT {end} which is handled explicitly below.
    // This covers both pipeline directives (builder: Some) and special-case
    // directives like {query} (builder: None, per-instance sentinels).
    if name != "end" {
        if let Some(d) = crate::codec::myst::directive_def(name) {
            if d.weight_kind.is_none() {
                return;
            }
        }
    }

    if name == "end" {
        if relation_context_stack.pop().is_none() {
            diagnostics.push(ParseDiagnostic::warning(
                "`{end}` encountered with no open relation context".to_string(),
            ));
        }
        return;
    }

    if name == "relation" {
        match parse_relation_args(args) {
            None => {
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "`{{relation}}` args could not be parsed: {args:?}; \
                     expected `kind=<pragmatic|epistemic|section>, ref=<source|sink>`"
                )));
            }
            Some((Some(verb_name), kind, ref_role)) => {
                // Custom verb registration — last-one-wins.
                if global_verb_context(&verb_name).is_some() {
                    diagnostics.push(ParseDiagnostic::warning(format!(
                        "`{{relation}}name={verb_name}` shadows a built-in verb"
                    )));
                }
                session_verb_registry.insert(verb_name, (kind, ref_role));
                // Registration only — do not push a context onto the stack.
            }
            Some((None, kind, ref_role)) => {
                // Precise form — push context directly.
                relation_context_stack.push((kind, ref_role, label.to_string()));
            }
        }
        return;
    }

    // Named verb — look up in session registry then global.
    let ctx = session_verb_registry
        .get(name)
        .copied()
        .or_else(|| global_verb_context(name));
    match ctx {
        Some((kind, ref_role)) => {
            relation_context_stack.push((kind, ref_role, label.to_string()));
        }
        None => {
            diagnostics.push(ParseDiagnostic::warning(format!(
                "unrecognized relation verb: `{{{name}}}`"
            )));
        }
    }
}

/// Parsed data from a single `{query}` directive block.
#[derive(Debug, Clone)]
struct QueryBlockData {
    /// JSON-serialized `QuerySpec`, or a JSON object `{"error":"msg"}` if parse failed.
    spec_json: String,
    /// Directive options as a `toml::Table`, JSON-serialized.
    options_json: String,
    /// The raw query text as authored by the user.
    query_text: String,
}

#[derive(Debug, Default, Clone)]
pub struct MdCodec {
    pub current_events: Vec<ProtoNodeWithEvents>,
    content: String,
    /// Track which section keys have been matched during inject_context phase
    matched_sections: HashSet<NodeKey>,
    /// Track heading IDs within current document for collision detection
    seen_ids: HashSet<String>,
    /// Byte offset of the most recently opened heading start tag, for position hints in diagnostics
    heading_start_offset: Option<usize>,
    /// Per-document session verb registry. Populated by `{relation}name=X, ...` codespan
    /// declarations (last-one-wins). Starts empty each parse; `directive_verb_context`
    /// consults this first, then falls back to `global_verb_context` from `DIRECTIVES`.
    session_verb_registry: HashMap<String, (WeightKind, ReferenceRole)>,
    /// Relation context stack. Each entry is pushed by a recognized codespan verb or
    /// `{relation}kind=K, ref=R` form, and popped by `` `{end}` `` or an implicit
    /// node-boundary close (heading). The tuple is `(WeightKind, ReferenceRole, label)`
    /// where `label` is the directive text as written, used in diagnostic messages.
    ///
    /// Links are routed per the top-of-stack context. When the stack is empty, the
    /// default `(WeightKind::Epistemic, ReferenceRole::Source)` applies (upstream).
    relation_context_stack: Vec<(WeightKind, ReferenceRole, String)>,
    /// Set while accumulating the body of a `{maps_to}` directive fenced block.
    /// Cleared when the matching `End(CodeBlock)` is encountered.
    in_maps_to_block: bool,
    /// Accumulates the body text of the current `{maps_to}` directive block.
    /// Reset to an empty string each time a `{maps_to}` block opens.
    maps_to_body_accum: String,
    /// Stores the `weight_kind` parsed from the `{maps_to}` info-string arg (e.g.
    /// `{maps_to} Pragmatic`). Takes precedence over a `weight_kind` key in the body.
    /// `None` when no info-string arg is present.
    maps_to_weight_kind_override: Option<WeightKind>,
    /// Set to `true` once the first heading is encountered during `parse()`. YAML
    /// frontmatter (`---` delimiters) is only valid before any heading; after a heading,
    /// pulldown-cmark's `MetadataBlock` events are actually horizontal rules and must be
    /// skipped to avoid corrupting the accumulator.
    past_first_heading: bool,
    /// Set to `true` during `parse()` when a deferred work directive (i.e. `{requirements_table`) is
    /// encountered. Drives `should_defer()` so the compiler calls `generate_deferred_html()` for
    /// this doc.
    pub(crate) has_deferred_render: bool,
    /// Set to `true` during `parse()` when a `{network_children}` MyST directive or the legacy
    /// `<!-- network-children -->` HTML comment marker is encountered. Used by
    /// `NetworkCodec::should_defer()` to signal that deferred child-listing replacement is needed.
    pub(crate) has_network_children: bool,
    /// Set while accumulating the body of a `{query}` directive fenced block.
    /// Cleared when the matching `End(CodeBlock)` is encountered.
    in_query_block: bool,
    /// Accumulates the body text of the current `{query}` directive block.
    query_body_accum: String,
    /// Accumulated `{query}` directive blocks in document order.
    /// Each entry stores the parsed result and directive options.
    /// Populated during `parse()`, consumed during `inject_context()`.
    query_blocks: Vec<QueryBlockData>,
}

impl MdCodec {
    pub fn new() -> Self {
        MdCodec {
            current_events: Vec::new(),
            content: String::new(),
            matched_sections: HashSet::new(),
            seen_ids: HashSet::new(),
            heading_start_offset: None,
            session_verb_registry: HashMap::new(),
            relation_context_stack: Vec::new(),
            in_maps_to_block: false,
            maps_to_body_accum: String::new(),
            maps_to_weight_kind_override: None,
            past_first_heading: false,
            has_deferred_render: false,
            has_network_children: false,
            in_query_block: false,
            query_body_accum: String::new(),
            query_blocks: Vec::new(),
        }
    }

    pub fn events_to_text<'a, I>(content: &str, events: I) -> Option<String>
    where
        I: Iterator<Item = (MdEvent<'a>, Option<Range<usize>>)>,
    {
        // Single pass: collect shortcuts and events simultaneously using inspect
        let mut shortcuts = Vec::new();
        let events_vec: Vec<(MdEvent<'a>, Option<Range<usize>>)> = events
            .inspect(|(e, _r)| {
                if let MdEvent::Start(MdTag::Link {
                    link_type: LinkType::Shortcut | LinkType::Reference,
                    dest_url,
                    title,
                    id,
                }) = e
                {
                    shortcuts.push((id.to_string(), dest_url.to_string(), title.to_string()));
                }
            })
            .collect();

        let mut buf = String::with_capacity(content.len() + 128);
        // panic!(
        //     "events:\n{}",
        //     self.current_events
        //         .iter()
        //         .map(|(_p, events)| events)
        //         .flatten()
        //         .map(|e| format!("{:?}", e))
        //         .collect::<Vec<String>>()
        //         .join(",\n")
        // );
        let options = CmarkToCmarkOptions::default();
        let events_with_refs = events_vec.iter().map(|(e, r)| (e, r.clone()));
        match cmark_resume_with_source_range_and_options(
            events_with_refs,
            content,
            &mut buf,
            None,
            options,
        ) {
            Ok(mut state) => {
                if !shortcuts.is_empty() {
                    state.shortcuts = shortcuts;
                    match state.finalize(&mut buf) {
                        Ok(_) => Some(buf),
                        Err(e) => {
                            tracing::error!(
                                "Could not finalize render of markdown file! Error(s): {:?}",
                                e
                            );
                            None
                        }
                    }
                } else {
                    Some(buf)
                }
            }
            Err(e) => {
                tracing::error!("Could not render updated markdown file! Error(s): {:?}", e);
                None
            }
        }
    }

    pub fn render_html_body(&self) -> String {
        /// Rewrite a link's dest_url from .md to .html, and extract the rel class if present.
        /// Returns `(rewritten_event, Option<rel_class>)` where `rel_class` is e.g.
        /// `"noet-rel-pragmatic-source"` when a relation type is encoded in the title attribute.
        fn rewrite_md_link(event: MdEvent<'static>) -> (MdEvent<'static>, Option<String>) {
            match event {
                MdEvent::Start(MdTag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => {
                    let url_ap = AnchorPath::from(&dest_url);
                    let should_rewrite = title.contains("bref://");

                    // Extract rel classes before URL rewriting consumes the title.
                    // The rel field is a space-separated list of "kind:role" pairs
                    // (e.g. "pragmatic:source epistemic:source"). Each pair becomes
                    // one CSS class: "noet-rel-pragmatic-source noet-rel-epistemic-source".
                    let rel_class: Option<String> = if should_rewrite && title.contains("\"rel\"") {
                        let parts = parse_title_attribute(&title);
                        parts.rel.map(|r| {
                            r.split_whitespace()
                                .map(|pair| {
                                    // "pragmatic:source" → "noet-rel-pragmatic-source"
                                    format!("noet-rel-{}", pair.replace(':', "-"))
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                    } else {
                        None
                    };

                    let new_url = if should_rewrite {
                        if url_ap.is_anchor() {
                            CowStr::from(as_anchor(url_ap.anchor()))
                        } else if !url_ap.ext().is_empty()
                            && crate::codec::is_known_codec_extension(&url_ap)
                        {
                            // Rewrite links for any extension known to CODECS or WALK_CODECS.
                            // is_known_codec_extension checks both registries, covering .md
                            // (via MdWalkCodec), built-ins (.xlsx etc via CODECS), and any
                            // shim-registered extensions. The !ext.is_empty() guard prevents
                            // the (None,None) CODECS wildcard from matching extensionless paths
                            // (Gemfile, Makefile, bare dirs). WALK_CODECS-only files without a
                            // claiming codec won't have bref:// links, so the should_rewrite
                            // gate above already excludes them in practice.
                            CowStr::from(
                                url_ap
                                    .normalize()
                                    .as_anchor_path()
                                    .replace_extension("html"),
                            )
                        } else {
                            tracing::trace!("no codec extension for {dest_url}, leaving unchanged");
                            dest_url
                        }
                    } else {
                        tracing::trace!("no bref element in title attribute for {dest_url}");
                        dest_url
                    };

                    (
                        MdEvent::Start(MdTag::Link {
                            link_type,
                            dest_url: new_url,
                            title,
                            id,
                        }),
                        rel_class,
                    )
                }
                other => (other, None),
            }
        }

        // For each proto node, rewrite link URLs (.md → .html) and inject CSS classes for
        // relation-typed links. Links with a `rel` field in their title attribute get their
        // Start(Link) replaced with a raw Html open-tag carrying class="noet-rel-…", and
        // their End(Link) replaced with a raw </a>.
        //
        // Priority mirrors generate_terminal_path so the NavTree path and the HTML anchor
        // are always in sync:
        //   1. Explicit {#id} from source (proto.id() when it differs from the bref fallback)
        //   2. to_anchor(title) slug
        //
        // The old approach always used to_anchor(title), which broke NavTree links whenever
        // an author supplied an explicit {#id} override — generate_terminal_path would put
        // the explicit id in the PathMap, but the HTML rendered a different anchor.
        // Build per-proto event lists with heading-anchor rewriting and directive marker
        // substitution performed together, while proto context (especially proto.bid for
        // bref-parameterized markers) is still available.
        //
        // For most directives the marker is a static string (from `myst::lookup`).
        // For `{maps_to}` the marker is parameterized with the owning section's bref so
        // that `generate_html_for_path` can build a per-section BeliefContext and query
        // only the edges owned by that specific section node.
        let query_block_counter = std::cell::Cell::new(0usize);
        // Per-section {maps_to} counter. Resets when the owning section anchor changes.
        // Both vars must live outside the per-section flat_map so the counter increments
        // correctly across multiple {maps_to} blocks within the same section.
        let maps_to_block_counter = std::cell::Cell::new(0usize);
        let mut maps_to_last_anchor = String::new();
        let events: Vec<MdEvent<'static>> = self
            .current_events
            .iter()
            .flat_map(|(proto, events)| {
                // Compute the anchor for this proto's heading (if it has one).
                // Section nodes (heading > 2) get an anchor; document/network nodes are
                // navigated to by document path, not by in-page anchor.
                let html_anchor: Option<CowStr<'static>> = if proto.heading > 2 {
                    // Use the explicit id when present (matches generate_terminal_path priority).
                    // Fall back to the title slug, matching the generate_terminal_path fallback.
                    proto
                        .id()
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            proto
                                .title()
                                .as_deref()
                                .map(to_anchor)
                                .filter(|s| !s.is_empty())
                        })
                        .map(CowStr::from)
                } else {
                    None
                };

                // Detect inline-anchor protos: explicit ID, heading > 2, but no
                // Start(Heading) in their event list. These need a rendered
                // <a id="..." class="noet-inline-anchor"></a> and {#id} stripping.
                let is_inline_anchor = proto.heading > 2
                    && proto.id().is_some()
                    && !events
                        .iter()
                        .any(|(e, _)| matches!(e, MdEvent::Start(MdTag::Heading { .. })));

                // Collect this proto's events with heading-anchor rewriting applied.
                let proto_events: Vec<MdEvent<'static>> = events
                    .iter()
                    .map(|(e, _)| {
                        if let (
                            Some(anchor),
                            MdEvent::Start(MdTag::Heading {
                                level,
                                id: _,
                                classes,
                                attrs,
                            }),
                        ) = (&html_anchor, e)
                        {
                            MdEvent::Start(MdTag::Heading {
                                level: *level,
                                id: Some(anchor.clone()),
                                classes: classes.clone(),
                                attrs: attrs.clone(),
                            })
                        } else if is_inline_anchor {
                            // Strip {#id} from Text events in inline-anchor protos.
                            if let MdEvent::Text(s) = e {
                                if extract_inline_anchor(s).is_some() {
                                    let stripped =
                                        STRIP_INLINE_ANCHOR_RE.replace(s.as_ref(), "").to_string();
                                    return MdEvent::Text(CowStr::from(stripped));
                                }
                            }
                            e.clone()
                        } else {
                            e.clone()
                        }
                    })
                    .flat_map(|event| {
                        let (rewritten, rel_class) = rewrite_md_link(event);
                        if let Some(class) = rel_class {
                            // Replace Start(Link) with a raw Html open tag carrying the
                            // CSS class. The End(Link) is handled below in the out loop.
                            if let MdEvent::Start(MdTag::Link {
                                dest_url, title, ..
                            }) = &rewritten
                            {
                                let open_tag = format!(
                                    r#"<a href="{}" title="{}" class="{}">"#,
                                    dest_url, title, class
                                );
                                return vec![
                                    MdEvent::Html(CowStr::from(open_tag)),
                                    // Sentinel to mark the matching End(Link) for replacement.
                                    MdEvent::Html(CowStr::from(
                                        "<!--noet-rel-close-->".to_string(),
                                    )),
                                ];
                            }
                        }
                        vec![rewritten]
                    })
                    .collect::<Vec<_>>();

                // Replace End(Link) events that follow a noet-rel-close sentinel
                // with a raw </a> tag, and remove the sentinel itself.
                let proto_events: Vec<MdEvent<'static>> = {
                    let mut result = Vec::with_capacity(proto_events.len());
                    let mut i = 0;
                    while i < proto_events.len() {
                        if let MdEvent::Html(ref s) = proto_events[i] {
                            if s.as_ref() == "<!--noet-rel-close-->" {
                                // Skip the sentinel; the next End(Link) becomes </a>.
                                i += 1;
                                // Consume events until we hit End(Link), emitting them.
                                while i < proto_events.len() {
                                    if matches!(proto_events[i], MdEvent::End(MdTagEnd::Link)) {
                                        result.push(MdEvent::Html(CowStr::from("</a>")));
                                        i += 1;
                                        break;
                                    }
                                    result.push(proto_events[i].clone());
                                    i += 1;
                                }
                                continue;
                            }
                        }
                        result.push(proto_events[i].clone());
                        i += 1;
                    }
                    result
                };

                // Substitute MyST directive CodeBlock event pairs with their HTML marker.
                // For `{maps_to}`, emit an anchor-parameterized marker so the deferred render
                // phase can associate each sentinel with its owning section node.
                // A directive produces: Start(CodeBlock(Fenced("{name}"))), [Text...], End(CodeBlock).
                // Unknown directives and plain fenced blocks are passed through unchanged.
                //
                // We use the section's heading anchor (e.g. "trace-mapping") rather than the
                // bref because section BIDs are ephemeral (time-based) and change on every
                // parse until the file is written to disk.  The anchor is derived from the
                // heading text, is stable across parses, and is unique within the document.
                let owner_anchor = proto
                    .id()
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        proto
                            .title()
                            .as_deref()
                            .map(crate::codec::md::to_anchor)
                            .filter(|s| !s.is_empty())
                    })
                    .unwrap_or_default();
                let mut out: Vec<MdEvent<'static>> = Vec::with_capacity(proto_events.len() + 1);

                // For inline-anchor protos, build the <a> element to inject INSIDE
                // the first block element (paragraph or list item).  Placing it
                // inside rather than before the block lets `p:hover` / `li:hover`
                // reveal the 🔗 link without :has() selectors.
                let inline_anchor_html: Option<MdEvent<'static>> = if is_inline_anchor {
                    html_anchor.as_ref().map(|anchor| {
                        let title_attr = proto
                            .document
                            .get("bid")
                            .and_then(|v| v.as_str())
                            .and_then(|bid_str| {
                                crate::properties::Bid::try_from(bid_str)
                                    .ok()
                                    .map(|bid| format!(" title=\"bref://{}\"", bid.bref()))
                            })
                            .unwrap_or_default();
                        MdEvent::Html(CowStr::from(format!(
                            "<a id=\"{anchor}\" class=\"noet-inline-anchor\"{title_attr}></a>",
                        )))
                    })
                } else {
                    None
                };
                let mut inline_anchor_injected = false;

                let mut i = 0;
                while i < proto_events.len() {
                    if let MdEvent::Start(MdTag::CodeBlock(MdCodeBlockKind::Fenced(ref info))) =
                        proto_events[i]
                    {
                        if let Some((name, _args)) =
                            crate::codec::myst::parse_directive_info(info.as_ref())
                        {
                            if crate::codec::myst::lookup(name).is_some() {
                                // Consume Start, any intervening Text/body, and End(CodeBlock).
                                i += 1;
                                while i < proto_events.len() {
                                    if matches!(proto_events[i], MdEvent::End(MdTagEnd::CodeBlock))
                                    {
                                        i += 1; // consume End
                                        break;
                                    }
                                    i += 1; // skip body Text events
                                }
                                // For {maps_to}, embed the owning section's anchor and a
                                // per-section index in the sentinel so the deferred render
                                // phase can differentiate multiple directives in the same section.
                                // For all other directives use the derived sentinel string.
                                let sentinel_str: String = if name == "maps_to" {
                                    if maps_to_last_anchor != owner_anchor {
                                        maps_to_last_anchor = owner_anchor.clone();
                                        maps_to_block_counter.set(0);
                                    }
                                    let idx = maps_to_block_counter.get();
                                    maps_to_block_counter.set(idx + 1);
                                    crate::codec::myst::mapping_table_sentinel(&owner_anchor, idx)
                                } else if name == "query" {
                                    // Per-instance sentinel; counter tracks document-wide
                                    // block index.
                                    let idx = query_block_counter.get();
                                    query_block_counter.set(idx + 1);
                                    crate::codec::myst::query_sentinel(idx)
                                } else {
                                    crate::codec::myst::sentinel(name)
                                };
                                if !sentinel_str.is_empty() {
                                    out.push(MdEvent::Html(CowStr::from(sentinel_str)));
                                }
                                continue;
                            }
                        }
                    }
                    // --- Codespan directive suppression ---
                    // Recognized codespan directives are suppressed from HTML output entirely
                    // (relation verbs, {end}, {relation}...). Pipeline directives emit their
                    // sentinel per the DirectiveDef invariant: non-empty sentinel → emit here.
                    if let MdEvent::Code(ref cow_str) = proto_events[i] {
                        let trimmed = cow_str.trim();
                        if let Some((name, args)) =
                            crate::codec::myst::parse_directive_info(trimmed)
                        {
                            // Bare codespans (no arguments) of non-verb directives
                            // are prose mentions — render as ordinary <code>, not
                            // as directive sentinels. Only relation verbs, {end},
                            // {relation}, and session verbs act as bare codespans.
                            let is_verb_or_control = crate::codec::myst::is_block_opener(name)
                                || name == "relation"
                                || name == "end"
                                || self.session_verb_registry.contains_key(name);
                            let is_known = if args.is_empty() {
                                is_verb_or_control
                            } else {
                                crate::codec::myst::lookup(name).is_some() || is_verb_or_control
                            };
                            if is_known {
                                // Emit sentinel if this directive has a deferred pipeline.
                                let sentinel_str: String = if name == "maps_to" {
                                    if maps_to_last_anchor != owner_anchor {
                                        maps_to_last_anchor = owner_anchor.clone();
                                        maps_to_block_counter.set(0);
                                    }
                                    let idx = maps_to_block_counter.get();
                                    maps_to_block_counter.set(idx + 1);
                                    crate::codec::myst::mapping_table_sentinel(&owner_anchor, idx)
                                } else if name == "query" {
                                    let idx = query_block_counter.get();
                                    query_block_counter.set(idx + 1);
                                    crate::codec::myst::query_sentinel(idx)
                                } else {
                                    crate::codec::myst::sentinel(name)
                                };
                                if !sentinel_str.is_empty() {
                                    out.push(MdEvent::Html(CowStr::from(sentinel_str)));
                                }
                                // Suppress the Code event itself — no <code> tag emitted.
                                i += 1;
                                continue;
                            }
                        }
                    }
                    out.push(proto_events[i].clone());
                    // Inject the id-bearing <a> right after the opening block tag
                    // so fragment navigation scrolls to the top of the block.
                    // The visible 🔗 link is appended at the end by content.js.
                    if !inline_anchor_injected {
                        if let Some(ref anchor_event) = inline_anchor_html {
                            if matches!(
                                proto_events[i],
                                MdEvent::Start(MdTag::Paragraph) | MdEvent::Start(MdTag::Item)
                            ) {
                                out.push(anchor_event.clone());
                                inline_anchor_injected = true;
                            }
                        }
                    }
                    i += 1;
                }
                out
            })
            .collect();

        // pulldown-cmark's html::push_html calls escape_html() on InlineMath and
        // DisplayMath content, turning '&' → '&amp;', '<' → '&lt;', etc.
        // This corrupts LaTeX: '&' is a column separator in \begin{align} and
        // \begin{bmatrix}, so KaTeX receives malformed input and either warns.
        //
        // When the `server-math` feature is enabled, we pre-render each math span
        // to HTML via the katex crate (which embeds a pure-Rust JS engine) so the
        // browser receives fully-rendered MathML/HTML and noetRenderMath() is a
        // no-op. This prevents the Chrome renderer OOM crash caused by 300+
        // simultaneous client-side katex.render() calls on math-heavy pages.
        //
        // Without `server-math` (e.g. wasm builds where quick-js cannot target
        // wasm32), we fall back to emitting raw LaTeX inside typed spans so the
        // client-side noetRenderMath() can render them in batches.
        let events: Vec<MdEvent> = events
            .into_iter()
            .map(|event| match event {
                MdEvent::InlineMath(text) => {
                    #[cfg(feature = "server-math")]
                    {
                        let opts = KatexOpts::builder()
                            .display_mode(false)
                            .throw_on_error(false)
                            .build()
                            .unwrap_or_default();
                        match render_with_opts(&text, opts) {
                            Ok(rendered) => MdEvent::Html(CowStr::from(rendered)),
                            Err(e) => {
                                tracing::warn!(
                                    "[md] KaTeX inline render failed: {e} | tex: {}",
                                    &*text
                                );
                                MdEvent::Html(CowStr::from(format!(
                                    r#"<span class="math math-inline">{}</span>"#,
                                    text
                                )))
                            }
                        }
                    }
                    #[cfg(not(feature = "server-math"))]
                    MdEvent::Html(CowStr::from(format!(
                        r#"<span class="math math-inline">{}</span>"#,
                        text
                    )))
                }

                MdEvent::DisplayMath(text) => {
                    #[cfg(feature = "server-math")]
                    {
                        let opts = KatexOpts::builder()
                            .display_mode(true)
                            .throw_on_error(false)
                            .build()
                            .unwrap_or_default();
                        match render_with_opts(&text, opts) {
                            Ok(rendered) => MdEvent::Html(CowStr::from(rendered)),
                            Err(e) => {
                                tracing::warn!(
                                    "[md] KaTeX display render failed: {e} | tex: {}",
                                    &*text
                                );
                                MdEvent::Html(CowStr::from(format!(
                                    r#"<span class="math math-display">{}</span>"#,
                                    text
                                )))
                            }
                        }
                    }
                    #[cfg(not(feature = "server-math"))]
                    MdEvent::Html(CowStr::from(format!(
                        r#"<span class="math math-display">{}</span>"#,
                        text
                    )))
                }
                other => other,
            })
            .collect();

        let mut html_body = String::new();
        pulldown_cmark::html::push_html(&mut html_body, events.into_iter());
        html_body
    }
}

impl DocCodec for MdCodec {
    /// Parse a path into a proto node by reading the metadata frontmatter (if any)
    fn proto(&self, path: &Path) -> Result<Option<IRNode>, BuildonomyError> {
        if path.is_relative() {
            return Err(BuildonomyError::Codec(format!(
                "[ProtoBeliefState::new] supplied path must be absolute. Received \"{path:?}\""
            )));
        };
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|&ext| ext == "md")
            .is_none()
        {
            tracing::debug!(
                "MdCodec::proto called with path \"{path:?}\", which has a non-'md' \
                file extension. Returning None"
            );
            return Ok(None);
        }
        let reader = File::open(path)?;
        let frontmatter = read_frontmatter(reader)?;

        let mut proto = if let Some(fm) = frontmatter {
            if !fm.is_empty() {
                IRNode::from_str(&fm)?
            } else {
                IRNode::default()
            }
        } else {
            // No frontmatter is fine for regular markdown documents
            IRNode::default()
        };
        if proto.title().unwrap_or_default().is_empty() {
            if let Some(filestem) = path
                .file_stem()
                .filter(|stem| !stem.is_empty())
                .and_then(|stem| stem.to_str())
            {
                let title = filestem
                    .split("_")
                    .map(titlecase)
                    .collect::<Vec<_>>()
                    .join(" ");
                proto.document.insert("title", value(title));
            }
        }
        proto.path = os_path_to_string(path);
        // Document heading
        proto.heading = 2;
        proto.kind.insert(BeliefKind::Document);
        Ok(Some(proto))
    }

    fn set_node_bid(&mut self, proto_idx: usize, bid: crate::properties::Bid) {
        if let Some((proto, _)) = self.current_events.get_mut(proto_idx) {
            // Only inject when the proto document has no bid yet. SourceFile nodes
            // already have the correct on-disk BID in their frontmatter; overwriting
            // them here would clobber collision-resolved BIDs before inject_context
            // can propagate the final value, causing "Written BIDs != cached BIDs".
            // Generated and GlobalCache section nodes have no bid in the heading's own
            // frontmatter (it lives in the document-level [sections] table), so they
            // always trigger this path and get the push()-resolved BID pre-populated.
            if proto.document.get("bid").is_none() {
                proto
                    .document
                    .insert("bid", toml_edit::value(bid.to_string()));
            }
        }
    }

    /// convert proto,
    /// insert bid into source if proto.bid is none
    /// rewrite links according to builder.doc_bb relations
    fn nodes(&self) -> Vec<IRNode> {
        self.current_events
            .iter()
            .map(|(proto, _)| proto.clone())
            .collect()
    }

    fn inject_context(
        &mut self,
        proto_idx: usize,
        node: &IRNode,
        ctx: &BeliefContext<'_>,
        diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<Option<BeliefNode>, BuildonomyError> {
        // Phase 2: Section Metadata Enrichment ("Look Up" Pattern)
        // Extract sections metadata BEFORE taking mutable borrow
        let sections_metadata = if node.heading > 2 {
            self.current_events
                .first()
                .and_then(|doc_node| doc_node.0.document.get("sections"))
                .map(parse_sections_metadata)
        } else {
            None
        };

        let current_events_len = self.current_events.len();
        let proto_events = self
            .current_events
            .get_mut(proto_idx)
            .ok_or(BuildonomyError::Codec(format!(
                "MdCodec::inject_context: proto_idx {proto_idx} out of range (len={current_events_len})",
            )))?;

        // All markdown nodes (document + headings) have events
        // TomlCodec is only used to enrich frontmatter parsing and preserve unknown fields
        let mut current_events = std::mem::take(&mut proto_events.1);

        let frontmatter_changed = proto_events.0.update_from_context(ctx)?;

        // Apply section metadata matching if we extracted it
        let mut sections_metadata_merged = false;
        if let Some(sections_map) = sections_metadata {
            // Try to find a match using priority: BID > Anchor > Title
            if let Some((matched_key, metadata_table)) =
                find_metadata_match(&proto_events.0, &sections_map)
            {
                // Track that we matched this key
                self.matched_sections.insert(matched_key.clone());

                // Merge metadata into the heading node; only mark as merged when at least
                // one new field was actually inserted. A match with no new fields means the
                // section node already had all the metadata (e.g. BID was already in the
                // proto from a prior parse's finalize() write-back), so there is nothing to
                // propagate and triggering text-regen would cause an infinite rewrite loop.
                if merge_metadata_into_node(&mut proto_events.0, metadata_table) {
                    sections_metadata_merged = true;
                }
            }
        }

        // Network-level collision detection and ID injection
        let mut id_changed = false;
        if proto_events.0.heading > 2 {
            // This is a heading node (not document)

            // Find the original ID from the heading event (check current_events, not proto_events.1).
            // Used below to decide whether to mutate the event stream (which forces cmark_resume
            // to write the id into the output rather than copying from the source range).
            let original_event_id = current_events.iter().find_map(|(event, _)| {
                if let MdEvent::Start(MdTag::Heading { id, .. }) = event {
                    id.as_ref().map(|s| s.to_string())
                } else {
                    None
                }
            });

            // Determine the authoritative final id.
            //
            // Priority (highest to lowest):
            //
            // 1. The id stored directly in `proto_events.0.document["id"]` after
            //    `codec.parse()` has completed — including intra-document collision
            //    resolution (Case A: explicit anchor removed; Case B: slug-N assigned).
            //    This is the ground truth from the current on-disk source.
            //
            //    NOTE: we read `document["id"]` directly rather than calling
            //    `proto_events.0.id()`, which would fall through to `to_anchor(title)`.
            //    The title-slug fallback is deliberately excluded here: if the proto has
            //    no explicit id in its document, `push()` already handled the title-slug
            //    path via `ctx.node.id()` below, and we must not pick up an unrelated
            //    title slug as a "source-local" id.
            //
            // 2. `ctx.node.id()` from `push()` — carries the collision-corrected value
            //    from the network-level first-one-wins check in builder.rs.  Used when
            //    the proto has no explicit id in its document (heading has no anchor yet
            //    and needs one assigned, or the anchor was cleared by a network collision).
            //
            // Why NOT `original_event_id` from the heading event stream:
            //    At `Start(Heading)`, the event id is set from the raw `{#anchor}` before
            //    intra-document collision resolution runs (which happens at `End(Heading)`
            //    and modifies `proto.document` but NOT the already-queued event).  Using
            //    the event id directly would expose the pre-collision anchor, which may
            //    have been cleared (Case A) or is wrong (the heading event retains the
            //    original explicit id even after Case B replaces it with slug-N).
            //
            // Why NOT `ctx.node.id()` unconditionally:
            //    global_bb's cached id may be stale.  Specifically, the remainder reparse
            //    on parse 1 reads the pre-rewrite file (before apply_rewrite has written
            //    the injected anchor back to disk), assigns a bref as the section's id,
            //    and commits that bref to global_bb.  On parse 2, the on-disk file has the
            //    explicit anchor, but ctx.node.id() still returns the stale bref —
            //    overwriting the source-parsed anchor with the bref produces a
            //    [sections."id://bref"] key that diverges from the on-disk
            //    [sections."id://anchor"] key, triggering an infinite rewrite loop.
            let proto_doc_id: Option<String> = proto_events
                .0
                .document
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            let final_id = if let Some(ref src_id) = proto_doc_id {
                // Source has an explicit, non-empty id (possibly collision-resolved).
                // Trust it unconditionally over the potentially stale cached id.
                src_id.clone()
            } else {
                // No explicit id in the proto document — use the cached/collision-corrected
                // value from push().  This is the normal id-injection path for headings that
                // have no anchor yet and need one assigned.
                ctx.node.id()
            };

            // Store the final ID in the proto
            if proto_events.0.id().as_ref() != Some(&final_id) {
                id_changed = true;
            }

            if Some(&final_id) != original_event_id.as_ref() {
                // Mutate heading event to inject final ID and clear range
                // Clearing the range forces cmark_resume to use event data instead of source
                // IMPORTANT: Modify current_events, not proto_events.1 (which was taken via mem::take)
                for (event, range) in current_events.iter_mut() {
                    if let MdEvent::Start(MdTag::Heading { id, .. }) = event {
                        *id = Some(CowStr::from(final_id.clone()));
                        *range = None; // Clear range to force writing modified ID
                        break;
                    }
                }
                // Set id_changed after injection to trigger text regeneration
                id_changed = true;
            }

            if id_changed {
                proto_events
                    .0
                    .document
                    .insert("id", value(final_id.clone()));
            }
        }

        // Only update frontmatter for document nodes (heading == 2), never for section nodes (heading > 2)
        // Section metadata stays in document-level "sections" table (Issue 02)
        if (frontmatter_changed.is_some() || sections_metadata_merged || id_changed)
            && proto_events.0.heading <= 2
        {
            let metadata_string = proto_events.0.as_frontmatter();
            update_or_insert_frontmatter(&mut current_events, &metadata_string)?;
        }

        let link_changed = check_for_link_and_push(
            &mut current_events,
            ctx,
            &node.path,
            &self.content,
            &mut proto_events.1,
            None,
            diagnostics,
        );
        let maybe_text = if frontmatter_changed.is_some()
            || sections_metadata_merged
            || link_changed
            || id_changed
        {
            if let Some(start_idx) = find_frontmatter_end(&proto_events.1) {
                Self::events_to_text(
                    &self.content,
                    proto_events.1.iter().skip(start_idx).cloned(),
                )
            } else {
                Self::events_to_text(&self.content, proto_events.1.iter().cloned())
            }
        } else {
            None
        };

        // Helper: carry forward runtime metadata from ctx.node into any newly-constructed
        // BeliefNode.  Metadata (git status, source backlinks) is never stored in source
        // files and is not part of the IRNode/TOML round-trip, so BeliefNode::try_from
        // always produces an empty metadata table.  Any metadata injected by push() via
        // metadata_override must survive the inject_context rewrite unchanged.
        let propagate_metadata = |mut node: BeliefNode| -> BeliefNode {
            if node.metadata.is_empty() && !ctx.node.metadata.is_empty() {
                node.metadata = ctx.node.metadata.clone();
            }
            node
        };

        let result: Result<Option<BeliefNode>, BuildonomyError> = if let Some(text) = maybe_text {
            proto_events.0.document.insert("text", value(text.clone()));
            // If sections metadata was merged OR frontmatter changed, create new node from proto
            // This ensures we capture both context updates AND sections metadata
            let new_node = if sections_metadata_merged || frontmatter_changed.is_some() {
                match BeliefNode::try_from(&proto_events.0) {
                    Ok(node) => propagate_metadata(node),
                    Err(e) => {
                        tracing::warn!("Failed to convert updated proto to BeliefNode: {:?}", e);
                        propagate_metadata(frontmatter_changed.unwrap_or(ctx.node.clone()))
                    }
                }
            } else {
                propagate_metadata(frontmatter_changed.unwrap_or(ctx.node.clone()))
            };
            let mut new_node_with_text = new_node;
            new_node_with_text
                .payload
                .insert("text".to_string(), toml::Value::String(text));
            Ok(Some(new_node_with_text))
        } else if sections_metadata_merged || frontmatter_changed.is_some() {
            // No text regeneration needed, but metadata was merged or context changed
            // Create new BeliefNode from the updated IRNode
            match BeliefNode::try_from(&proto_events.0) {
                Ok(new_node) => Ok(Some(propagate_metadata(new_node))),
                Err(e) => {
                    tracing::warn!(
                        "Failed to convert proto with merged metadata to BeliefNode: {:?}",
                        e
                    );
                    Ok(frontmatter_changed.map(propagate_metadata))
                }
            }
        } else {
            Ok(None)
        };

        // Inject source_url, {query} metadata, and {maps_to} specs into the result node.
        // All run regardless of whether other inject_context logic produced a change —
        // if there is no other change, we promote the result from None to Some so the
        // metadata is propagated through the event system.
        let source_url = compute_source_url(node, ctx);
        let has_query_metadata = !self.query_blocks.is_empty() && node.heading <= 2;
        // Section nodes (heading > 2) with {maps_to} directives carry per-directive
        // source/sink NodeKey strings so the compiler can filter owned edges per-directive.
        let has_mapping_specs = node.heading > 2 && !node.mappings.is_empty();

        if source_url.is_some() || has_query_metadata || has_mapping_specs {
            let mut result_node = match result? {
                Some(n) => n,
                None => ctx.node.clone(),
            };

            if let Some(url) = source_url {
                result_node
                    .metadata
                    .insert("source_url".to_string(), toml::Value::String(url));
            }

            // Persist {query} directive data as internal metadata keys.
            //
            // These are consumed by `generate_html_for_path` in compiler.rs
            // during Phase 3 (deferred HTML rendering). They are NOT user-facing
            // and should not appear in exported BeliefGraph or source files.
            //
            // `_query_specs`: TOML array of JSON strings. Each element is either:
            //   - A JSON-serialized `QuerySpec` (from `query_parser::parse`)
            //   - A JSON object `{"error": "message"}` for parse failures
            //   Array index N corresponds to sentinel `<!--@@noet-query:N@@-->`.
            //
            // `_query_options`: Parallel TOML array of JSON strings. Each element
            //   is a JSON-serialized `toml::Table` of directive options (`:view:`,
            //   `:caption:`, `:sort:`, etc.) mapped to params keys.
            if has_query_metadata {
                let specs_array: Vec<toml::Value> = self
                    .query_blocks
                    .iter()
                    .map(|qb| toml::Value::String(qb.spec_json.clone()))
                    .collect();
                let options_array: Vec<toml::Value> = self
                    .query_blocks
                    .iter()
                    .map(|qb| toml::Value::String(qb.options_json.clone()))
                    .collect();
                let texts_array: Vec<toml::Value> = self
                    .query_blocks
                    .iter()
                    .map(|qb| toml::Value::String(qb.query_text.clone()))
                    .collect();
                result_node
                    .metadata
                    .insert("_query_specs".to_string(), toml::Value::Array(specs_array));
                result_node.metadata.insert(
                    "_query_options".to_string(),
                    toml::Value::Array(options_array),
                );
                result_node
                    .metadata
                    .insert("_query_texts".to_string(), toml::Value::Array(texts_array));
            }

            // Persist {maps_to} directive specs as internal metadata on section nodes.
            // Each element is a JSON string encoding {"sources": [...], "sinks": [...]}
            // using the NodeKey Display form (e.g. "id:impl-one", "bid:...").
            // Array index N corresponds to sentinel `<!--@@noet-mapping-table:ANCHOR:N@@-->`.
            // Consumed by `generate_html_for_path` to filter owned edges per-directive.
            if has_mapping_specs {
                let specs_array: Vec<toml::Value> = node
                    .mappings
                    .iter()
                    .map(|m| {
                        let sources: Vec<String> =
                            m.sources.iter().map(|k| k.to_string()).collect();
                        let sinks: Vec<String> = m.sinks.iter().map(|k| k.to_string()).collect();
                        let json = serde_json::json!({"sources": sources, "sinks": sinks});
                        toml::Value::String(json.to_string())
                    })
                    .collect();
                result_node.metadata.insert(
                    "_maps_to_specs".to_string(),
                    toml::Value::Array(specs_array),
                );
            }

            Ok(Some(result_node))
        } else {
            result
        }
    }

    fn should_defer(&self) -> bool {
        self.has_deferred_render
    }

    fn generate_source(&self) -> Option<String> {
        let events = self
            .current_events
            .iter()
            .flat_map(|(_p, events)| events.iter().cloned());
        Self::events_to_text(&self.content, events)
    }

    fn generate_html(&self) -> Result<crate::codec::HtmlFragmentPairs, BuildonomyError> {
        let doc_abs_path = self
            .current_events
            .first()
            .map(|(proto, _)| proto.path.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or("document.md".to_string());
        let doc_abs_ap = AnchorPath::from(&doc_abs_path);

        // Extract filename and convert extension to .html.
        // Handle empty path (tests) by defaulting to "document.html".
        if doc_abs_ap.filestem().is_empty() {
            return Err(BuildonomyError::Codec(format!(
                "Markdown file has no filename! {doc_abs_path}",
            )));
        }
        let output_filename = format!("{}.html", doc_abs_ap.filestem());

        let body = self.render_html_body();

        Ok(vec![(
            output_filename,
            vec![("{{BODY}}".to_string(), body)],
            Some(crate::codec::assets::Layout::Simple),
        )])
    }

    fn finalize(
        &mut self,
        diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<std::collections::HashMap<crate::properties::Bid, IRNode>, BuildonomyError> {
        let mut modified_nodes = std::collections::HashMap::new();

        // Step 1: Build sections table from all section nodes (heading > 2)
        // This happens AFTER all inject_context() calls, so sections have BIDs
        let mut sections_table = toml_edit::Table::new();

        for (section_proto, _) in self.current_events.iter().skip(1) {
            // Skip document node (index 0), collect section nodes (heading > 2)
            if section_proto.heading > 2 {
                // Get or generate section ID
                // Sections without IDs are collision cases where Bref should be used
                let section_id = if let Some(id) = section_proto.id() {
                    id.clone()
                } else {
                    // Generate Bref from BID for sections without IDs (collision cases)
                    if let Some(bid_value) = section_proto.document.get("bid") {
                        if let Some(bid_str) = bid_value.as_str() {
                            if let Ok(bid) = crate::properties::Bid::try_from(bid_str) {
                                let bref = bid.bref().to_string();
                                tracing::debug!(
                                    "finalize() - Generated Bref '{}' for section without ID: title={:?}",
                                    bref,
                                    section_proto.document.get("title").and_then(|v| v.as_str())
                                );
                                bref
                            } else {
                                diagnostics.push(ParseDiagnostic::warning(format!(
                                    "Section has invalid BID, skipping: title={:?}",
                                    section_proto.document.get("title").and_then(|v| v.as_str())
                                )));
                                continue;
                            }
                        } else {
                            diagnostics.push(ParseDiagnostic::warning(format!(
                                "Section BID is not a string, skipping: title={:?}",
                                section_proto.document.get("title").and_then(|v| v.as_str())
                            )));
                            continue;
                        }
                    } else {
                        diagnostics.push(ParseDiagnostic::warning(format!(
                            "Section has no BID, skipping: title={:?}",
                            section_proto.document.get("title").and_then(|v| v.as_str())
                        )));
                        continue;
                    }
                };

                let mut section_metadata = toml_edit::Table::new();

                // Always include BID (required)
                if let Some(bid) = section_proto.document.get("bid") {
                    section_metadata.insert("bid", bid.clone());
                }

                // Include ID (for lookup)
                section_metadata.insert("id", value(section_id.clone()));

                // Include schema if present
                if let Some(schema) = section_proto.document.get("schema") {
                    section_metadata.insert("schema", schema.clone());
                }

                // Include any other metadata fields (excluding internal fields)
                for (key, val) in section_proto.document.iter() {
                    if !matches!(key, "bid" | "id" | "title" | "text" | "schema" | "heading") {
                        section_metadata.insert(key, val.clone());
                    }
                }

                // Use NodeKey format for section table key (e.g., "id://background")
                let section_key = format!("id://{}", section_id);
                sections_table.insert(&section_key, toml_edit::Item::Table(section_metadata));
            }
        }

        // Step 2: Update document's sections field and handle garbage collection
        // Access document node (always at index 0) to check for unmatched sections
        if let Some(doc_proto) = self.current_events.first_mut() {
            // Compare built sections table with existing sections in frontmatter
            let existing_sections = doc_proto.0.document.get("sections");

            let needs_update = if !sections_table.is_empty() {
                match existing_sections {
                    Some(existing) => {
                        // Compare table contents directly
                        if let Some(existing_table) = existing.as_table() {
                            // Check if keys match
                            let existing_keys: std::collections::HashSet<&str> =
                                existing_table.iter().map(|(k, _)| k).collect();
                            let new_keys: std::collections::HashSet<&str> =
                                sections_table.iter().map(|(k, _)| k).collect();

                            existing_keys != new_keys
                        } else {
                            // existing is not a table, need to replace
                            true
                        }
                    }
                    None => true, // No existing sections, need to add
                }
            } else {
                // sections_table is empty — no heading>2 nodes in markdown.
                // Only update if stale sections exist on-disk that need to be removed.
                existing_sections.is_some()
            };

            if needs_update {
                // Update or remove sections field
                if !sections_table.is_empty() {
                    doc_proto
                        .0
                        .document
                        .insert("sections", toml_edit::Item::Table(sections_table));
                } else {
                    // No sections in markdown, remove sections field
                    doc_proto.0.document.remove("sections");
                }

                // Update the frontmatter events with the modified document
                let metadata_string = doc_proto.0.as_frontmatter();
                update_or_insert_frontmatter(&mut doc_proto.1, &metadata_string)?;

                // Document was modified — return the (Bid, IRNode) pair so the caller can
                // apply source-file-derived field changes via BeliefNode::apply_source_update,
                // which preserves runtime-only fields (bid, metadata) that IRNode lacks.
                // Extract the BID from the document table (inject_context wrote it there).
                // If the BID is absent or malformed, skip rather than inserting under a nil key.
                let bid_opt = doc_proto
                    .0
                    .document
                    .get("bid")
                    .and_then(|v| v.as_str())
                    .and_then(|s| crate::properties::Bid::try_from(s).ok());
                match bid_opt {
                    Some(bid) => {
                        modified_nodes.insert(bid, doc_proto.0.clone());
                    }
                    None => {
                        tracing::warn!(
                            "[finalize] document IRNode has no valid BID in its document table; \
                             skipping apply_source_update for title={:?}",
                            doc_proto.0.document.get("title").and_then(|v| v.as_str())
                        );
                    }
                }
            }
        }

        Ok(modified_nodes)
    }

    fn parse(
        &mut self,
        content: &str,
        mut current: IRNode,
        diagnostics: &mut Vec<ParseDiagnostic>,
        proto_index: &crate::codec::proto_index::ProtoIndex,
    ) -> Result<(), BuildonomyError> {
        // Initial parse and format to try and make pulldown_cmark <-> pulldown_cmark_to_cmark idempotent
        self.content = content.to_string();
        self.current_events = Vec::default();
        self.matched_sections.clear();
        self.seen_ids.clear();
        self.heading_start_offset = None;
        self.session_verb_registry.clear();
        self.relation_context_stack.clear();
        self.in_maps_to_block = false;
        self.maps_to_body_accum = String::new();
        self.maps_to_weight_kind_override = None;
        self.past_first_heading = false;
        self.has_deferred_render = false;
        self.has_network_children = false;
        self.in_query_block = false;
        self.query_body_accum = String::new();
        self.query_blocks.clear();
        // Document root node always starts at line 1
        current.source_line = Some(1);
        let mut proto_events = VecDeque::new();
        let mut link_stack: Vec<LinkAccumulator> = Vec::new();
        // Inline anchor detection state (local to parse, not struct fields).
        // `enclosing_heading_depth` tracks the heading depth of the most recent
        // heading-created node so consecutive inline anchors are siblings.
        let mut enclosing_heading_depth: usize = current.heading;
        // Index into proto_events where the current scannable block started.
        // Set on Start(Paragraph) / Start(Item), consumed on End(Paragraph) / End(Item).
        let mut inline_anchor_block_start: Option<usize> = None;
        for (event, offset) in MdParser::new_with_broken_link_callback(
            &self.content,
            buildonomy_md_options(),
            Some(|link: BrokenLink<'_>| {
                let reference = link.reference.into_static();
                Some((reference.clone(), reference))
            }),
        )
        .into_offset_iter()
        {
            if let Some(link_data) = LinkAccumulator::new(event.borrow(), &Some(offset.clone())) {
                link_stack.push(link_data);
            }
            let mut push_relation = false;
            if let Some(link_data) = link_stack.last_mut() {
                push_relation = link_data.push(event.borrow(), &Some(offset.clone()));
            }
            if push_relation {
                let link_data = link_stack.pop().expect(
                    "Push relation is only true if link_data is some and the link end tag is found",
                );
                let mut node_keys = link_to_relation(
                    &link_data.link_type,
                    &link_data.rel_url,
                    &CowStr::from(link_data.title_string()),
                    &link_data.id.clone(),
                );
                if let Some(primary_key) = node_keys.first().cloned() {
                    let node_key = primary_key.resolve_against(&current.path);
                    let title = link_data.title_string();
                    let payload = if !title.is_empty()
                        && title != link_data.rel_url.as_ref()
                        && title != link_data.id.as_ref()
                    {
                        let mut weight = Weight::default();
                        weight.set::<String>("title", title).ok();
                        Some(weight)
                    } else {
                        None
                    };
                    // Route the relation per the top of the relation context stack.
                    // ReferenceRole::Source → referent is source, subject is sink → upstream.
                    // ReferenceRole::Sink   → referent is sink, subject is source → downstream.
                    let (relation_kind, ref_role) = self
                        .relation_context_stack
                        .last()
                        .map(|(k, r, _)| (*k, *r))
                        .unwrap_or((WeightKind::Epistemic, ReferenceRole::Source));
                    let fallback_keys: Vec<NodeKey> = node_keys.drain(1..).collect();
                    let mut relation = IntermediateRelation::new(node_key, relation_kind, payload)
                        .with_fallback_keys(fallback_keys);
                    if let Some(byte_offset) = link_data.range.as_ref().map(|r| r.start) {
                        relation = relation.with_location(byte_offset);
                    }
                    match ref_role {
                        // Referent is source → subject (IR node) is sink → upstream
                        ReferenceRole::Source => current.upstream.push(relation),
                        // Referent is sink → subject (IR node) is source → downstream
                        ReferenceRole::Sink => current.downstream.push(relation),
                    }
                }
            }

            // log::debug!("[codec::md]: {:?}", event);
            match event.borrow() {
                MdEvent::Start(MdTag::MetadataBlock(_)) if !self.past_first_heading => {
                    debug_assert!(current.accumulator.is_none());
                    current.accumulator = Some(String::new());
                }
                MdEvent::End(MdTagEnd::MetadataBlock(_)) if !self.past_first_heading => {
                    let toml_string = current.accumulator.take().expect(
                        "to never encounter an end tag before a start tag and always initialize \
                         accum to Some in the start tag",
                    );

                    match IRNode::from_str(&toml_string) {
                        Ok(mut proto) => {
                            current.merge(&mut proto);
                        }
                        Err(e) => {
                            // Fallback to simple deserialization if TomlCodec fails
                            tracing::warn!("IRNode toml parse failed: {:?}", e);
                            current.errors.push(e);
                        }
                    };
                }
                // After the first heading, `---` is a horizontal rule, not a YAML
                // frontmatter delimiter. pulldown-cmark may still emit MetadataBlock
                // events for it, so we silently ignore them here.
                MdEvent::Start(MdTag::MetadataBlock(_)) | MdEvent::End(MdTagEnd::MetadataBlock(_)) => {}
                MdEvent::Text(cow_str) | MdEvent::InlineHtml(cow_str) => {
                    if self.in_maps_to_block {
                        // Accumulate body text for the {maps_to} directive.
                        self.maps_to_body_accum.push_str(cow_str);
                    } else if self.in_query_block {
                        self.query_body_accum.push_str(cow_str);
                    } else if !current.document.contains_key("title") || current.content.is_empty()
                    {
                        if let Some(accum_string_ref) = current.accumulator.as_mut() {
                            *accum_string_ref += " ";
                            *accum_string_ref += cow_str;
                        } else {
                            current.accumulator = Some(cow_str.to_string());
                        }
                    }
                }
                MdEvent::Code(cow_str) => {
                    // Own strings early to avoid borrow conflict with MdParser's borrow on self.content.
                    let trimmed_owned = cow_str.trim().to_string();
                    let cow_owned = cow_str.to_string();
                    // Gate: only treat as a directive if the name is recognized.
                    // Unrecognized `{...}` code spans (e.g. `{variable}`) pass through silently.
                    let directive_parsed = crate::codec::myst::parse_directive_info(&trimmed_owned)
                        .map(|(n, a)| (n.to_string(), a.to_string()));
                    if let Some((name_owned, args_owned)) = directive_parsed {
                        // Dispatch is registry-driven via directive_def(). Three cases:
                        //   1. Known relation verb (weight_kind + ref_role set) → relation stack
                        //   2. Pipeline directive (builder set) → deferred render flags
                        //   3. Control keyword (end, relation) → dispatch_relation_directive
                        //   4. Session-registered custom verb → relation stack
                        // Unrecognized names pass through silently (accumulate as content).
                        //
                        // Bare codespans (no arguments) of non-verb directives are prose
                        // mentions, not invocations — e.g. `{query}`, `{network_children}`.
                        // Only relation verbs ({implements}, {uses}, ...), {end}, {relation},
                        // and session-registered verbs are meaningful as bare codespans.
                        let def = crate::codec::myst::directive_def(&name_owned);
                        let is_verb_or_control = name_owned == "relation"
                            || name_owned == "end"
                            || self.session_verb_registry.contains_key(name_owned.as_str())
                            || def.is_some_and(|d| d.weight_kind.is_some());
                        let is_known = if args_owned.is_empty() {
                            // Bare codespan: only verbs and control keywords are directives.
                            is_verb_or_control
                        } else {
                            // Codespan with arguments: any known directive is valid.
                            def.is_some() || is_verb_or_control
                        };
                        if is_known {
                            // Relation verbs and control keywords → relation context stack.
                            // dispatch_relation_directive silently skips pipeline directives.
                            dispatch_relation_directive(
                                &name_owned,
                                &args_owned,
                                &trimmed_owned,
                                &mut self.relation_context_stack,
                                &mut self.session_verb_registry,
                                diagnostics,
                            );
                            // Pipeline directives (builder: Some) → deferred render flags.
                            // Driven by the registry; no explicit name enumeration needed.
                            if let Some(d) = def {
                                if d.builder.is_some() || !d.queries.is_empty() {
                                    self.has_deferred_render = true;
                                    if d.name == "network_children" {
                                        self.has_network_children = true;
                                    }
                                    // maps_to requires body accumulation — registry-driven
                                    // flag plus per-directive args parsing.
                                    if d.name == "maps_to" {
                                        self.in_maps_to_block = true;
                                        self.maps_to_body_accum = String::new();
                                        self.maps_to_weight_kind_override =
                                            WeightKind::try_from(args_owned.as_str()).ok();
                                    }
                                }
                            }
                            // Do not accumulate directive codespans into title/content.
                            // Fall through to proto_events push below (round-trip fidelity).
                        } else {
                            // Not a directive — accumulate for title/content as normal.
                            if !current.document.contains_key("title")
                                || current.content.is_empty()
                            {
                                if let Some(accum_string_ref) = current.accumulator.as_mut() {
                                    *accum_string_ref += " ";
                                    *accum_string_ref += &cow_owned;
                                } else {
                                    current.accumulator = Some(cow_owned);
                                }
                            }
                        }
                    } else {
                        // No `{...}` prefix — plain code span, accumulate normally.
                        if !current.document.contains_key("title") || current.content.is_empty() {
                            if let Some(accum_string_ref) = current.accumulator.as_mut() {
                                *accum_string_ref += " ";
                                *accum_string_ref += &cow_owned;
                            } else {
                                current.accumulator = Some(cow_owned);
                            }
                        }
                    }
                }
                MdEvent::End(MdTagEnd::CodeBlock) if self.in_query_block => {
                    let body = std::mem::take(&mut self.query_body_accum);
                    self.in_query_block = false;

                    // Parse directive options from the body
                    let (options, query_string) =
                        crate::codec::myst::parse_directive_options(&body);

                    // Build the options toml::Table
                    let mut params = toml::Table::new();
                    for (k, v) in &options {
                        // Map directive option keys to params keys
                        let param_key = match k.as_str() {
                            "view" => "display",
                            "sort" => "sort",
                            "max-rows" => "max_rows",
                            "caption" => "caption",
                            "columns" => "columns",
                            other => other,
                        };
                        params.insert(
                            param_key.to_string(),
                            toml::Value::String(v.clone()),
                        );
                    }

                    // Try to parse the query string
                    let spec_json = match crate::query::parser::parse(&query_string) {
                        Ok(spec) => serde_json::to_string(&spec).unwrap_or_else(|e| {
                            format!("{{\"error\":\"spec serialization failed: {e}\"}}")
                        }),
                        Err(e) => {
                            serde_json::json!({"error": e.to_string()}).to_string()
                        }
                    };

                    let options_json =
                        serde_json::to_string(&params).unwrap_or_else(|e| {
                            format!("{{\"error\":\"options serialization failed: {e}\"}}")
                        });

                    self.query_blocks.push(QueryBlockData {
                        spec_json,
                        options_json,
                        query_text: query_string,
                    });
                }
                MdEvent::End(MdTagEnd::CodeBlock) if self.in_maps_to_block => {
                    // Parse the accumulated body and populate current.mappings.
                    let body = std::mem::take(&mut self.maps_to_body_accum);
                    self.in_maps_to_block = false;
                    let kind_override = self.maps_to_weight_kind_override.take();
                    match crate::codec::belief_ir::parse_with_fallback(
                        &body,
                        crate::codec::belief_ir::MetadataFormat::Toml,
                    ) {
                        Ok(doc) => {
                            // Resolve weight_kind: info-string arg takes precedence over body field.
                            let kind = kind_override
                                .or_else(|| {
                                    doc.get("weight_kind")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| WeightKind::try_from(s).ok())
                                })
                                .unwrap_or(WeightKind::Epistemic);

                            /// Parse a directive body field ("source" or "sink") that accepts
                            /// either a single node key string or an array of node key strings.
                            fn parse_node_keys(
                                doc: &toml_edit::DocumentMut,
                                field: &str,
                                base_path: &str,
                            ) -> Vec<NodeKey> {
                                if let Some(arr) = doc.get(field).and_then(|v| v.as_array()) {
                                    arr.iter()
                                        .filter_map(|item| item.as_str())
                                        .filter_map(|s| {
                                            NodeKey::from_str(s)
                                                .ok()
                                                .map(|k| k.resolve_against(base_path))
                                        })
                                        .collect()
                                } else if let Some(s) = doc.get(field).and_then(|v| v.as_str()) {
                                    NodeKey::from_str(s)
                                        .ok()
                                        .map(|k| k.resolve_against(base_path))
                                        .into_iter()
                                        .collect()
                                } else {
                                    vec![]
                                }
                            }

                            // Parse source and sink: each accepts a single string or an array.
                            let source_keys = parse_node_keys(&doc, "source", &current.path);
                            let sink_keys = parse_node_keys(&doc, "sink", &current.path);

                            // Collect extra payload fields (all keys except source, sink, weight_kind).
                            let extra_weight = {
                                use crate::properties::Weight;
                                let mut w = Weight::default();
                                let skip = ["source", "sink", "weight_kind"];
                                let mut has_extra = false;
                                for (k, v) in doc.iter() {
                                    if skip.contains(&k) {
                                        continue;
                                    }
                                    if let Some(s) = v.as_str() {
                                        w.set::<String>(k, s.to_string()).ok();
                                        has_extra = true;
                                    } else if let Some(i) = v.as_integer() {
                                        w.set::<i64>(k, i).ok();
                                        has_extra = true;
                                    } else if let Some(f) = v.as_float() {
                                        w.set::<f64>(k, f).ok();
                                        has_extra = true;
                                    } else if let Some(b) = v.as_bool() {
                                        w.set::<bool>(k, b).ok();
                                        has_extra = true;
                                    }
                                }
                                if has_extra {
                                    Some(w)
                                } else {
                                    None
                                }
                            };

                            if source_keys.is_empty() {
                                diagnostics.push(ParseDiagnostic::warning(
                                    "{maps_to} directive missing or unresolvable `source` \
                                     field; skipping"
                                        .to_string(),
                                ));
                            } else if sink_keys.is_empty() {
                                diagnostics.push(ParseDiagnostic::warning(
                                    "{maps_to} directive has no resolvable `sink` entries; \
                                     skipping"
                                        .to_string(),
                                ));
                            } else {
                                current.mappings.push(
                                    crate::codec::belief_ir::IntermediateMappingRelation {
                                        sources: source_keys,
                                        sinks: sink_keys,
                                        kind,
                                        weight: extra_weight,
                                        location: None,
                                    },
                                );
                            }
                        }
                        Err(e) => {
                            diagnostics.push(ParseDiagnostic::warning(format!(
                                "{{maps_to}} directive body failed to parse: {e}"
                            )));
                        }
                    }
                }
                MdEvent::Start(MdTag::Heading {
                    level,
                    id,
                    classes: _,
                    attrs: _,
                }) => {
                    // Auto-close any open relation context stack entries on heading boundaries.
                    // Emit one warning per unclosed entry so authors know to add `{end}`.
                    for (_, _, label) in self.relation_context_stack.drain(..) {
                        diagnostics.push(ParseDiagnostic::warning(format!(
                            "implicit close of `{{{label}}}` at node boundary (missing `{{end}}`)"
                        )));
                    }
                    self.in_maps_to_block = false;
                    self.maps_to_body_accum = String::new();
                    self.maps_to_weight_kind_override = None;
                    self.in_query_block = false;
                    self.query_body_accum = String::new();
                    self.past_first_heading = true;
                    self.heading_start_offset = Some(offset.start);
                    let heading_line = byte_offset_to_location(&self.content, offset.start).0;
                    let heading = match level {
                        // 0: UUID_NAMESPACE_BUILDONOMY
                        // 1: Network Node
                        // 2: Doc node (file)
                        HeadingLevel::H1 => 3,
                        HeadingLevel::H2 => 4,
                        HeadingLevel::H3 => 5,
                        HeadingLevel::H4 => 6,
                        HeadingLevel::H5 => 7,
                        HeadingLevel::H6 => 8,
                    };
                    // Capture and normalize explicit ID from {#anchor} syntax
                    let maybe_normalized_id = id.as_ref().map(|id_str| to_anchor(id_str));
                    let mut new_current = IRNode {
                        path: current.path.clone(),
                        heading,
                        source_line: Some(heading_line),
                        ..Default::default()
                    };
                    if let Some(normalized_id) = maybe_normalized_id {
                        // Store explicit {#anchor} id into the document. seen_ids tracking
                        // happens at End(Heading) where the title is also available, so we
                        // can compute the full effective id the same way BeliefNode::id() would.
                        new_current.document.insert("id", value(normalized_id));
                    }
                    // Inherit the schema type from the prior parse. If the node has an explicit
                    // schema, it will overwrite this when merging the node's toml.
                    let mut proto_to_push = replace(&mut current, new_current);
                    proto_to_push.traverse_schema()?;

                    // Do NOT eagerly insert id from title here. Title→id derivation is handled
                    // lazily by BeliefNode::id() (properties.rs) and builder.rs push().
                    let proto_to_push_events = std::mem::take(&mut proto_events);
                    self.current_events
                        .push((proto_to_push, proto_to_push_events));
                    // Track the heading depth for inline-anchor sibling depth.
                    enclosing_heading_depth = heading;
                    // Clear any pending block tracking — heading boundary supersedes.
                    inline_anchor_block_start = None;
                }
                MdEvent::End(MdTagEnd::Heading(_)) => {
                    // We should never encounter a heading end tag before a heading start tag, and
                    // we initialize title_accum to Some(String::new) in the start tag.
                    let accum_title = current.accumulator.take().unwrap_or_default();
                    let current_title = current.title().unwrap_or_default();
                    // Check whether the heading carries the magic `{#__continue}` id.
                    let is_continue = current
                        .document
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s == MAGIC_CONTINUE_ID)
                        .unwrap_or(false);
                    // Heading 3 is an h1. heading 1 == network, heading 2 == document
                    if is_continue || accum_title.is_empty() || current_title == accum_title {
                        // Don't count this as a new section --- glue it back onto the last proto for these cases:
                        // 1. the heading has the {#__continue} magic id (explicit continuation),
                        // 2. the new title is empty,
                        // 3. it's the same as the last section title, or
                        // 4. BEHAVIOR REMOVED ~~its an h1 at the start of the document with no
                        //    prior headings~~
                        if let Some((last_proto, mut last_event_vec)) = self.current_events.pop() {
                            current = last_proto;
                            last_event_vec.append(&mut proto_events);
                            proto_events = last_event_vec;
                        }
                    } else {
                        current.document.insert("title", value(accum_title.clone()));
                    }
                    // Check for intra-document anchor collision for all section nodes (heading > 2).
                    // Done here at End(Heading) because this is the first point where both the
                    // explicit id (set at Start) and the accumulated title are available, so we
                    // can mirror the full BeliefNode::id() fallback: explicit id > to_anchor(title).
                    // Skip entirely for {#__continue} headings — they produced no new node, so
                    // there is nothing to register in seen_ids and no collision to detect.
                    if !is_continue && current.heading > 2 && !accum_title.is_empty() {
                        let candidate_id = current
                            .document
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                let slug = to_anchor(&accum_title);
                                if slug.is_empty() {
                                    None
                                } else {
                                    Some(slug)
                                }
                            });
                        if let Some(candidate) = candidate_id {
                            if self.seen_ids.contains(&candidate) {
                                // Collision: a prior section already owns this anchor slug.
                                let (line, col) = self
                                    .heading_start_offset
                                    .map(|offset| byte_offset_to_location(&self.content, offset))
                                    .unwrap_or((0, 0));

                                // Distinguish two cases:
                                //
                                // A) The explicit {#anchor} collides but the title-derived slug
                                //    is free. Drop only the explicit id; let the title fallback
                                //    give the node a stable, addressable anchor. The warning
                                //    tells the author their explicit tag was ignored.
                                //
                                // B) The title-derived slug itself collides (no explicit id, or
                                //    both collide). The node truly cannot have an addressable
                                //    anchor — insert the empty-string sentinel so IRNode::id()
                                //    suppresses the title fallback and builder.rs falls back to
                                //    a bref-based id.
                                let has_explicit_id = current
                                    .document
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| !s.is_empty())
                                    .unwrap_or(false);
                                let title_slug = to_anchor(accum_title.as_str());
                                let title_slug_free =
                                    !title_slug.is_empty() && !self.seen_ids.contains(&title_slug);

                                if has_explicit_id && title_slug_free {
                                    // Case A: explicit anchor collides, title is fine.
                                    // Remove the explicit id so IRNode::id() falls through to
                                    // the title slug, giving the node a stable identity.
                                    current.document.remove("id");
                                    // Register the title slug as claimed so later headings
                                    // with the same title still collide correctly.
                                    self.seen_ids.insert(title_slug);
                                    diagnostics.push(
                                        ParseDiagnostic::info(format!(
                                            "Intra-document anchor collision: explicit anchor \
                                             '{}' is already used in this document. The \
                                             explicit anchor has been removed; the heading will \
                                             use its title-derived anchor instead.",
                                            candidate
                                        ))
                                        .with_location(line, col),
                                    );
                                } else {
                                    // Case B: title slug also collides (or there is no title
                                    // slug). Assign a human-readable slug-N id (e.g.
                                    // "details-2", "details-3") so the node has a stable,
                                    // addressable anchor without falling back to a bref.
                                    // Use seen_ids to find the next free suffix.
                                    let base = match title_slug.rsplit_once('-') {
                                        Some((prefix, suffix)) if suffix.parse::<u32>().is_ok() => {
                                            prefix.to_string()
                                        }
                                        _ => title_slug.clone(),
                                    };
                                    let mut counter: u32 = 2;
                                    let slug_n = loop {
                                        let candidate = format!("{}-{}", base, counter);
                                        if !self.seen_ids.contains(&candidate) {
                                            break candidate;
                                        }
                                        counter += 1;
                                        if counter > 9999 {
                                            tracing::warn!(
                                                "Pathological title anchor: fall back to original \
                                                candidate. (bref-based node id will be assigned \
                                                downstream)."
                                            );
                                            break candidate;
                                        }
                                    };
                                    self.seen_ids.insert(slug_n.clone());
                                    current.document.insert("id", value(slug_n.clone()));
                                    diagnostics.push(
                                        ParseDiagnostic::warning(format!(
                                            "Intra-document heading anchor collision: '{}' \
                                             appears more than once in this document. The \
                                             duplicate heading has been assigned the anchor \
                                             '{}'.",
                                            candidate, slug_n
                                        ))
                                        .with_location(line, col),
                                    );
                                }
                            } else {
                                self.seen_ids.insert(candidate);
                            }
                        }
                        self.heading_start_offset = None;
                    }
                }
                MdEvent::Start(MdTag::CodeBlock(MdCodeBlockKind::Fenced(info))) => {
                    // Detect MyST backtick-fence directives: info strings of the form `{name}`
                    // or `{name} args`.  Plain language tags (e.g. "rust") are unaffected.
                    // Directives are syntax-form-agnostic: fenced-block and codespan forms
                    // both dispatch through the same logic. Own strings before any &mut self
                    // calls to avoid borrow conflict with MdParser's borrow on self.content.
                    let directive_parsed =
                        crate::codec::myst::parse_directive_info(info.as_ref())
                            .map(|(n, a)| (n.to_string(), a.to_string()));
                    if let Some((name_owned, args_owned)) = directive_parsed {
                        let name = name_owned.as_str();
                        let args = args_owned.as_str();
                        // Dispatch is registry-driven via directive_def(). Same logic as
                        // the codespan arm — syntax form (fenced vs codespan) is irrelevant.
                        let def = crate::codec::myst::directive_def(name);
                        match def {
                            None if !name.is_empty() && name != "relation" && name != "end"
                                && !self.session_verb_registry.contains_key(name) =>
                            {
                                diagnostics.push(ParseDiagnostic::warning(format!(
                                    "unknown noet directive: {{{name}}}"
                                )));
                            }
                            _ => {
                                // Relation verbs, {end}, and {relation} → relation context stack.
                                // dispatch_relation_directive silently skips pipeline directives.
                                dispatch_relation_directive(
                                    name,
                                    args,
                                    name,
                                    &mut self.relation_context_stack,
                                    &mut self.session_verb_registry,
                                    diagnostics,
                                );
                                // Pipeline directives (builder: Some) → deferred render flags.
                                // Driven by the registry; no explicit name enumeration needed.
                                if let Some(d) = def {
                                    if d.builder.is_some() || !d.queries.is_empty() {
                                        self.has_deferred_render = true;
                                        if d.name == "network_children" {
                                            self.has_network_children = true;
                                        }
                                        // maps_to requires body accumulation — registry-driven
                                        // flag plus per-directive args parsing.
                                        if d.name == "maps_to" {
                                            self.in_maps_to_block = true;
                                            self.maps_to_body_accum = String::new();
                                            // Parse optional weight_kind from info-string args
                                            // e.g. `{maps_to} Pragmatic` → WeightKind::Pragmatic
                                            self.maps_to_weight_kind_override =
                                                WeightKind::try_from(args).ok();
                                        }
                                    }
                                }
                            }
                        }
                        // {query} uses per-instance sentinels and body accumulation
                        // (not the standard pipeline, which requires builder or queries).
                        if name == "query" {
                            self.has_deferred_render = true;
                            self.in_query_block = true;
                            self.query_body_accum = String::new();
                        }
                        // Keep original events in proto_events unchanged (write-back fidelity).
                    }
                }
                MdEvent::Html(cow_str)
                    // Detect the legacy <!-- network-children --> HTML comment marker so
                    // NetworkCodec::should_defer() knows to run the deferred child listing pass.
                    if cow_str.contains("<!-- network-children -->") => {
                    self.has_network_children = true;
                }
                // -- Inline anchor detection: track block boundaries --
                //
                // Set the splice point on Start(Paragraph) and Start(Item). The inner
                // block (Paragraph inside Item) overrides the outer, so loose list items
                // detect at the paragraph level. Tight list items (no Paragraph wrapper)
                // detect at the Item level.
                MdEvent::Start(MdTag::Paragraph) => {
                    inline_anchor_block_start = Some(proto_events.len());
                }
                MdEvent::Start(MdTag::Item) => {
                    inline_anchor_block_start = Some(proto_events.len());
                }
                // At block end, scan accumulated text for {#id}. If found, splice the
                // block events into a new child node at enclosing_heading_depth + 1.
                MdEvent::End(MdTagEnd::Paragraph) | MdEvent::End(MdTagEnd::Item) => {
                    if let Some(block_start) = inline_anchor_block_start.take() {
                        if let Some(anchor_id) =
                            scan_for_inline_anchor(&proto_events, block_start)
                        {
                            // Full boundary reset (mirrors heading boundary at Start(Heading)).
                            for (_, _, label) in self.relation_context_stack.drain(..) {
                                diagnostics.push(ParseDiagnostic::warning(format!(
                                    "implicit close of `{{{label}}}` at node boundary \
                                     (missing `{{end}}`)"
                                )));
                            }
                            self.in_maps_to_block = false;
                            self.maps_to_body_accum = String::new();
                            self.maps_to_weight_kind_override = None;
                            self.in_query_block = false;
                            self.query_body_accum = String::new();

                            // Split proto_events: [0..block_start) stays with old node,
                            // [block_start..] becomes the new node's events.
                            let block_events = proto_events.split_off(block_start);
                            let pre_block_events =
                                std::mem::replace(&mut proto_events, block_events);

                            // Source line from the block's opening event.
                            let source_line = proto_events
                                .front()
                                .and_then(|(_, range)| range.as_ref())
                                .map(|r| byte_offset_to_location(&self.content, r.start).0);

                            // Create the inline-anchor node.
                            let normalized_id = to_anchor(&anchor_id);
                            let mut new_current = IRNode {
                                path: current.path.clone(),
                                heading: enclosing_heading_depth + 1,
                                source_line,
                                ..Default::default()
                            };
                            new_current
                                .document
                                .insert("id", value(normalized_id.clone()));
                            new_current
                                .document
                                .insert("title", value(anchor_id));

                            // Push the old node with its pre-block events.
                            let mut proto_to_push = replace(&mut current, new_current);
                            proto_to_push.traverse_schema()?;
                            self.current_events
                                .push((proto_to_push, pre_block_events));

                            // Collision detection for the inline anchor ID
                            // (mirrors End(Heading) Case B — inline anchors have no
                            // title fallback, so always assign slug-N on collision).
                            if self.seen_ids.contains(&normalized_id) {
                                let (line, col) = source_line
                                    .map(|l| (l, 0))
                                    .unwrap_or((0, 0));
                                // Find next free slug-N suffix.  Unlike heading
                                // collisions, do NOT strip a trailing numeric
                                // component — the entire anchor ID is author-chosen
                                // (e.g. `swdp-63`), so the suffix should be
                                // `swdp-63-2`, not `swdp-7`.
                                let base = normalized_id.clone();
                                let mut counter: u32 = 2;
                                let slug_n = loop {
                                    let candidate = format!("{}-{}", base, counter);
                                    if !self.seen_ids.contains(&candidate) {
                                        break candidate;
                                    }
                                    counter += 1;
                                    if counter > 9999 {
                                        break candidate;
                                    }
                                };
                                self.seen_ids.insert(slug_n.clone());
                                current.document.insert("id", value(slug_n.clone()));
                                current
                                    .document
                                    .insert("title", value(slug_n.clone()));
                                diagnostics.push(
                                    ParseDiagnostic::warning(format!(
                                        "Intra-document anchor collision: inline anchor \
                                         '{}' is already used in this document. The \
                                         duplicate has been assigned the anchor '{}'.",
                                        normalized_id, slug_n
                                    ))
                                    .with_location(line, col),
                                );
                            } else {
                                self.seen_ids.insert(normalized_id);
                            }
                        }
                    }
                }
                _ => {}
            }
            proto_events.push_back((event.into_static(), Some(offset)));
        }
        current.traverse_schema()?;
        // Do NOT eagerly insert id from title for the final node either.
        // Title→id derivation is handled lazily by BeliefNode::id() (properties.rs).
        self.current_events.push((current, proto_events));
        // tracing::debug!("Parsed a total of {} nodes", self.current_events.len());

        // panic!(
        //     "events:\n{}",
        //     self.current_events
        //         .iter()
        //         .map(|(_p, events)| events)
        //         .flatten()
        //         .map(|e| format!("{:?}", e))
        //         .collect::<Vec<String>>()
        //         .join(",\n")
        // );

        // Evaluate alias-template from ancestor network config.
        //
        // Which nodes receive an alias is governed by the network's `alias-scope`
        // (default `submap` = every node, the original behaviour) and overridden
        // per node by an `alias = true|false` field.  Under `submap`, a template
        // like `{{ jira_base_url }}/browse/{{ id | upper }}` correctly derives a
        // Jira URL for each hand-authored section (`{#ticket-1101}` → `TICKET-1101`).
        // Under `explicit`, only nodes that opt in are aliased — for corpora whose
        // descendants are machine-generated headings that should not be addressable.
        // See `AliasScope` for the measurements that motivated the distinction.
        if !self.current_events.is_empty() {
            // Resolve the config once using the document root's path (all nodes
            // in this file share the same ancestor network).
            let doc_path = std::path::PathBuf::from(&self.current_events[0].0.path);
            if let Some((_ancestor_dir, alias_config)) = proto_index
                .ancestor_meta_as::<crate::codec::network::AliasTemplateConfig>(
                &doc_path,
                "url_alias",
            ) {
                // A heading's `alias` opt-in/out lives in the document root's
                // `[sections."#anchor"]` table.  `inject_context` merges those tables
                // into their nodes, but that runs in Phase 4 — after `push()` has
                // already consumed `namespace_paths` and emitted the href edge — so
                // the merged value is not available here.  Read the raw table
                // instead, reusing the same matching logic `inject_context` uses so
                // the two cannot disagree about which table belongs to which node.
                let sections_metadata = self.current_events[0]
                    .0
                    .document
                    .get("sections")
                    .map(parse_sections_metadata);

                for (node, _events) in self.current_events.iter_mut() {
                    let node_opt = node
                        .document
                        .get("alias")
                        .and_then(|v| v.as_bool())
                        .or_else(|| {
                            sections_metadata.as_ref().and_then(|meta| {
                                find_metadata_match(node, meta)
                                    .and_then(|(_key, table)| table.get("alias"))
                                    .and_then(|v| v.as_bool())
                            })
                        });
                    if !alias_config.scope.applies(node_opt) {
                        continue;
                    }
                    if let Some(alias) = crate::codec::network::evaluate_alias_template(
                        &alias_config.template,
                        &node.document,
                    ) {
                        node.namespace_paths
                            .push((crate::properties::href_namespace(), alias));
                    } else {
                        tracing::debug!(
                            "[MdCodec::parse] alias-template '{}' could not be evaluated \
                             for node '{:?}' in {:?} (missing or non-scalar field)",
                            alias_config.template,
                            node.id(),
                            node.path,
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::init_logging;
    use crate::{nodekey::NodeKey, paths::to_anchor, properties::Bid};
    use std::collections::HashMap;
    use toml_edit::{DocumentMut, Table as TomlTable};

    // ── parse_markdown_relations ─────────────────────────────────────────────

    #[test]
    fn test_parse_markdown_relations_empty_string() {
        let mut diag = Vec::new();
        let rels = parse_markdown_relations("", "/repo/doc.md", &mut diag);
        assert!(rels.is_empty(), "empty string should produce no relations");
        assert!(diag.is_empty());
    }

    #[test]
    fn test_parse_markdown_relations_plain_text_no_links() {
        let mut diag = Vec::new();
        let rels = parse_markdown_relations(
            "The system shall initialise all subsystems at power-on.",
            "/repo/doc.md",
            &mut diag,
        );
        assert!(
            rels.is_empty(),
            "plain text with no links should produce no relations"
        );
    }

    #[test]
    fn test_parse_markdown_relations_inline_link() {
        let mut diag = Vec::new();
        let rels = parse_markdown_relations(
            "See [Widget Init](id:widget-init) for details.",
            "/repo/doc.md",
            &mut diag,
        );
        assert_eq!(rels.len(), 1, "one inline link should produce one relation");
        // All links from parse_markdown_relations are Epistemic by default.
        assert_eq!(rels[0].kind, WeightKind::Epistemic);
    }

    #[test]
    fn test_parse_markdown_relations_multiple_links() {
        let mut diag = Vec::new();
        let content = "References [Alpha](id:alpha) and [Beta](id:beta).";
        let rels = parse_markdown_relations(content, "/repo/doc.md", &mut diag);
        assert_eq!(rels.len(), 2, "two links should produce two relations");
    }

    #[test]
    fn test_parse_markdown_relations_wikilink() {
        let mut diag = Vec::new();
        let rels = parse_markdown_relations(
            "See [[widget-init]] for context.",
            "/repo/doc.md",
            &mut diag,
        );
        // WikiLinks resolve to NodeKey::Id — should produce one relation.
        assert_eq!(rels.len(), 1);
        assert!(
            matches!(&rels[0].key, NodeKey::Id { id, .. } if id.contains("widget-init") || !id.is_empty()),
            "wikilink should produce an Id-keyed relation, got {:?}",
            rels[0].key
        );
    }

    #[test]
    fn test_parse_markdown_relations_link_with_title_payload() {
        let mut diag = Vec::new();
        let rels =
            parse_markdown_relations("[Widget Init](id:widget-init)", "/repo/doc.md", &mut diag);
        assert_eq!(rels.len(), 1);
        // The link text "Widget Init" should be stored as a title payload on the weight.
        let has_title = rels[0]
            .weight
            .as_ref()
            .and_then(|w| w.get::<String>("title"))
            .is_some();
        assert!(
            has_title,
            "link with distinct text should carry title payload"
        );
    }

    #[test]
    fn test_parse_markdown_relations_bare_bref_link() {
        let mut diag = Vec::new();
        // A link whose text equals the URL (bare bref) should produce no title payload.
        let rels = parse_markdown_relations("[abc123de](abc123de)", "/repo/doc.md", &mut diag);
        assert_eq!(rels.len(), 1);
        // title == dest_url → no payload
        let has_title = rels[0]
            .weight
            .as_ref()
            .and_then(|w| w.get::<String>("title"))
            .is_some();
        assert!(
            !has_title,
            "link where text == url should have no title payload"
        );
    }

    #[test]
    fn test_parse_markdown_relations_byte_offset_populated() {
        let mut diag = Vec::new();
        let content = "Prefix text. [Link](id:target) suffix.";
        let rels = parse_markdown_relations(content, "/repo/doc.md", &mut diag);
        assert_eq!(rels.len(), 1);
        assert!(
            rels[0].location.is_some(),
            "byte offset should be populated for links with a known range"
        );
        // The link starts after "Prefix text. " (13 bytes).
        assert!(
            rels[0].location.unwrap() >= 13,
            "byte offset should point into the link, not the start of the string"
        );
    }

    #[test]
    fn test_parse_markdown_relations_no_diagnostics_emitted() {
        // Unresolvable links are silently dropped — no diagnostic.
        let mut diag = Vec::new();
        let rels = parse_markdown_relations(
            "Text with [broken ref][missing] and plain words.",
            "/repo/doc.md",
            &mut diag,
        );
        // Whether or not a relation is produced, no diagnostic should be emitted.
        assert!(
            diag.is_empty(),
            "parse_markdown_relations should never emit diagnostics"
        );
        let _ = rels; // result may be empty or contain a relation; either is acceptable
    }

    /// Parse sections field from frontmatter into flat metadata map.
    /// Returns HashMap<NodeKey, TomlTable> for matching against heading nodes.
    fn parse_sections_metadata(sections: &toml_edit::Item) -> HashMap<NodeKey, TomlTable> {
        let mut metadata = HashMap::new();

        if let Some(table) = sections.as_table() {
            for (key_str, value) in table.iter() {
                // Parse key as NodeKey
                if let Ok(node_key) = NodeKey::from_str(key_str) {
                    // Extract value as TomlTable
                    if let Some(value_table) = value.as_table() {
                        metadata.insert(node_key, value_table.clone());
                    }
                }
            }
        }

        metadata
    }

    /// Find metadata match for a IRNode with priority: BID > Anchor > Title.
    fn find_metadata_match<'a>(
        node: &IRNode,
        metadata: &'a HashMap<NodeKey, TomlTable>,
    ) -> Option<&'a TomlTable> {
        // Priority 1: Match by BID (most explicit)
        if let Some(bid_value) = node.document.get("bid") {
            if let Some(bid_str) = bid_value.as_str() {
                if let Ok(bid) = Bid::try_from(bid_str) {
                    let bid_key = NodeKey::Bid { bid };
                    if let Some(meta) = metadata.get(&bid_key) {
                        return Some(meta);
                    }
                }
            }
        }

        // Priority 2: Match by anchor (medium specificity)
        if let Some(anchor) = node.id() {
            // Try as Id variant (anchors are IDs within a document)
            let anchor_key = NodeKey::Id {
                net: Bref::default(),
                id: anchor,
            };
            if let Some(meta) = metadata.get(&anchor_key) {
                return Some(meta);
            }
        }

        // Priority 3: Match by title anchor (least specific)
        // Use Id variant since titles are only guaranteed unique for documents
        if let Some(title_value) = node.document.get("title") {
            if let Some(title) = title_value.as_str() {
                let anchor = to_anchor(title);
                let id_key = NodeKey::Id {
                    net: Bref::default(),
                    id: anchor,
                };
                if let Some(meta) = metadata.get(&id_key) {
                    return Some(meta);
                }
            }
        }

        None
    }

    // ========== UNIT TESTS ==========

    #[test]
    fn test_parse_sections_metadata_with_bid_keys() {
        init_logging();
        let toml_str = r#"
bid = "00000000-0000-0000-0000-000000000001"
schema = "Document"

[sections."bid://00000000-0000-0000-0000-000000000002"]
schema = "Section"
complexity = "high"

[sections."bid://00000000-0000-0000-0000-000000000003"]
schema = "Section"
complexity = "medium"
"#;
        let doc: DocumentMut = toml_str.parse().unwrap();
        let sections = doc.get("sections").unwrap();

        let metadata = parse_sections_metadata(sections);

        assert_eq!(
            metadata.len(),
            2,
            "sections toml:\n{:?}\nmetadata: {:?}",
            sections,
            metadata
        );

        let bid2 = Bid::try_from("00000000-0000-0000-0000-000000000002").unwrap();
        let key2 = NodeKey::Bid { bid: bid2 };
        assert!(metadata.contains_key(&key2));
        assert_eq!(
            metadata
                .get(&key2)
                .unwrap()
                .get("complexity")
                .unwrap()
                .as_str()
                .unwrap(),
            "high"
        );
    }

    #[test]
    fn test_parse_sections_metadata_with_anchor_keys() {
        let toml_str = r#"
bid = "00000000-0000-0000-0000-000000000001"

[sections."id://introduction"]
schema = "Section"
complexity = "high"

[sections."id://background"]
schema = "Section"
complexity = "low"
"#;
        let doc: DocumentMut = toml_str.parse().unwrap();
        let sections = doc.get("sections").unwrap();

        let metadata = parse_sections_metadata(sections);

        // Note: Plain strings like "introduction" (no whitespace) are parsed as Id variant
        // Strings with whitespace become Title variant (normalized via to_anchor)
        assert_eq!(metadata.len(), 2);

        // Verify that plain string keys become NodeKey::Id
        let intro_key = NodeKey::Id {
            net: Bref::default(),
            id: "introduction".to_string(),
        };
        assert!(metadata.contains_key(&intro_key), "{:?}", metadata);
    }

    #[test]
    fn test_parse_sections_metadata_empty_sections() {
        let toml_str = r#"
bid = "00000000-0000-0000-0000-000000000001"
schema = "Document"
"#;
        let doc: DocumentMut = toml_str.parse().unwrap();
        let sections = doc.get("sections");

        if let Some(sections_item) = sections {
            let metadata = parse_sections_metadata(sections_item);
            assert_eq!(metadata.len(), 0);
        }
    }

    #[test]
    fn test_to_anchor_usage() {
        // to_anchor lowercases, maps whitespace to hyphens, collapses hyphens,
        // and preserves semantically meaningful punctuation (.  _  ()  []  @).
        assert_eq!(to_anchor("Introduction"), "introduction");
        assert_eq!(to_anchor("My Section Title"), "my-section-title");
        // ':' is stripped; surrounding spaces collapse to a single hyphen
        assert_eq!(to_anchor("Section 2.1: Overview"), "section-2.1-overview");
        // '&' is stripped; surrounding spaces collapse to a single hyphen
        assert_eq!(to_anchor("API & Reference"), "api-reference");
    }

    #[test]
    fn test_find_metadata_match_by_bid() {
        let mut metadata = HashMap::new();
        let bid = Bid::try_from("00000000-0000-0000-0000-000000000002").unwrap();
        let key = NodeKey::Bid { bid };

        let mut table = TomlTable::new();
        table.insert("complexity", value("high"));
        metadata.insert(key, table);

        // Create a node with matching BID
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("00000000-0000-0000-0000-000000000002"));
        doc.insert("title", value("Introduction"));

        let node = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 4,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().get("complexity").unwrap().as_str().unwrap(),
            "high"
        );
    }

    #[test]
    fn test_find_metadata_match_by_anchor() {
        let mut metadata = HashMap::new();
        let key = NodeKey::Id {
            net: Bref::default(),
            id: "intro".to_string(),
        };

        let mut table = TomlTable::new();
        table.insert("complexity", value("medium"));
        metadata.insert(key, table);

        // Create a node with matching anchor
        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));
        doc.insert("anchor", value("intro"));
        doc.insert("id", value("intro"));
        let node = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 4,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().get("complexity").unwrap().as_str().unwrap(),
            "medium"
        );
    }

    #[test]
    fn test_find_metadata_match_by_title_anchor() {
        let mut metadata = HashMap::new();
        // Use Id variant for title-based matching (not Title)
        let key = NodeKey::Id {
            net: Bref::default(),
            id: "introduction".to_string(),
        };

        let mut table = TomlTable::new();
        table.insert("complexity", value("low"));
        metadata.insert(key, table);

        // Create a node with matching title (no BID, no anchor)
        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));

        let node = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 4,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().get("complexity").unwrap().as_str().unwrap(),
            "low"
        );
    }

    #[test]
    fn test_find_metadata_match_priority_bid_over_anchor() {
        let mut metadata = HashMap::new();

        // Add BID match
        let bid = Bid::try_from("00000000-0000-0000-0000-000000000002").unwrap();
        let bid_key = NodeKey::Bid { bid };
        let mut bid_table = TomlTable::new();
        bid_table.insert("source", value("bid"));
        metadata.insert(bid_key, bid_table);

        // Add anchor match
        let anchor_key = NodeKey::Id {
            net: Bref::default(),
            id: "intro".to_string(),
        };
        let mut anchor_table = TomlTable::new();
        anchor_table.insert("source", value("anchor"));
        metadata.insert(anchor_key, anchor_table);

        // Create node with BOTH BID and anchor
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("00000000-0000-0000-0000-000000000002"));
        doc.insert("anchor", value("intro"));
        doc.insert("title", value("Introduction"));

        let node = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 4,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_some());
        // Should match by BID (highest priority)
        assert_eq!(
            result.unwrap().get("source").unwrap().as_str().unwrap(),
            "bid"
        );
    }

    #[test]
    fn test_find_metadata_match_priority_anchor_over_title() {
        let mut metadata = HashMap::new();

        // Add anchor match
        let anchor_key = NodeKey::Id {
            net: Bref::default(),
            id: "intro".to_string(),
        };
        let mut anchor_table = TomlTable::new();
        anchor_table.insert("source", value("anchor"));
        metadata.insert(anchor_key, anchor_table);

        // Add title match (using Id variant)
        let title_key = NodeKey::Id {
            net: Bref::default(),
            id: "introduction".to_string(),
        };
        let mut title_table = TomlTable::new();
        title_table.insert("source", value("title"));
        metadata.insert(title_key, title_table);

        // Create node with anchor and title (no BID)
        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));
        doc.insert("id", value("intro"));
        let node = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 4,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_some());
        // Should match by anchor (higher priority than title)
        assert_eq!(
            result.unwrap().get("source").unwrap().as_str().unwrap(),
            "anchor"
        );
    }

    #[test]
    fn test_find_metadata_match_no_match() {
        let metadata = HashMap::new(); // Empty metadata

        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));

        let node = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 4,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_none());
    }

    // ========================================================================

    #[test]
    fn test_pulldown_cmark_to_cmark_writes_heading_ids() {
        // Verify that pulldown_cmark_to_cmark writes the `id` field from heading events
        use pulldown_cmark::{Event as MdEvent, HeadingLevel, Tag as MdTag, TagEnd as MdTagEnd};

        // Test 1: Parse heading with ID
        let markdown = "## My Heading {#my-id}";
        let parser = MdParser::new_ext(markdown, buildonomy_md_options());
        let events: Vec<MdEvent> = parser.collect();

        // Verify ID was parsed
        let has_id = events.iter().any(|e| {
            if let MdEvent::Start(MdTag::Heading { id, .. }) = e {
                id.as_ref().map(|s| s.as_ref()) == Some("my-id")
            } else {
                false
            }
        });
        assert!(has_id, "Should parse heading ID");

        // Test 2: Write back with cmark
        let mut buf = String::new();
        pulldown_cmark_to_cmark::cmark(events.iter(), &mut buf).unwrap();
        assert!(
            buf.contains("{ #my-id }") || buf.contains("{#my-id}"),
            "Should write heading ID back. Got: {buf}"
        );

        // Test 3: Modify ID and write
        let modified_events = [
            MdEvent::Start(MdTag::Heading {
                level: HeadingLevel::H2,
                id: Some(CowStr::from("new-id")),
                classes: Vec::new(),
                attrs: Vec::new(),
            }),
            MdEvent::Text(CowStr::from("My Heading")),
            MdEvent::End(MdTagEnd::Heading(HeadingLevel::H2)),
        ];

        let mut buf2 = String::new();
        pulldown_cmark_to_cmark::cmark(modified_events.iter(), &mut buf2).unwrap();
        assert!(
            buf2.contains("{ #new-id }") || buf2.contains("{#new-id}"),
            "Should write modified heading ID. Got: {buf2}"
        );

        // Test 4: Normalized ID (lowercase, no punctuation)
        let normalized_events = [
            MdEvent::Start(MdTag::Heading {
                level: HeadingLevel::H2,
                id: Some(CowStr::from("my-heading")), // normalized
                classes: Vec::new(),
                attrs: Vec::new(),
            }),
            MdEvent::Text(CowStr::from("My Heading")),
            MdEvent::End(MdTagEnd::Heading(HeadingLevel::H2)),
        ];

        let mut buf3 = String::new();
        pulldown_cmark_to_cmark::cmark(normalized_events.iter(), &mut buf3).unwrap();
        assert!(
            buf3.contains("{ #my-heading }") || buf3.contains("{#my-heading}"),
            "Should write normalized ID. Got: {buf3}"
        );
    }

    #[test]
    fn test_id_normalization_during_parse() {
        // Test that parse() sets the title correctly and that IRNode::id() derives a slug from the
        // title when no explicit {#anchor} is present. IRNode::id() mirrors BeliefNode::id():
        // explicit document["id"] takes priority, then to_anchor(title), then None.
        use toml_edit::DocumentMut;

        let markdown = "## My-Section!";
        let mut codec = MdCodec::new();

        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000001"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("My Document"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let heading_node = codec.current_events.iter().find(|(p, _)| p.heading > 2);
        assert!(
            heading_node.is_some(),
            "Should have heading node. codec events:\n{}",
            codec
                .current_events
                .iter()
                .map(|(proto, events)| format!("{}\n{events:?}", proto.document))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let (proto, _) = heading_node.unwrap();

        // Title is set from the heading text
        assert_eq!(
            proto.title().as_deref(),
            Some("My-Section!"),
            "Title should be set from heading text"
        );

        // No explicit {#anchor} was present — IRNode::id() falls back to to_anchor(title).
        // "My-Section!" → to_anchor → "my-section"
        assert_eq!(
            proto.id().as_deref(),
            Some("my-section"),
            "IRNode::id() should return title-derived slug when no explicit anchor is set"
        );
    }

    #[test]
    fn test_intra_document_heading_anchor_collision_title_derived() {
        // Two headings with the same title: the first claims the title-derived slug via id();
        // the second collides and is assigned a slug-N id (e.g. "introduction-2") so it has
        // a stable, human-readable anchor without falling back to a bref.
        // Both survive as section nodes with their titles intact.
        use crate::codec::DocCodec;
        use toml_edit::DocumentMut;

        let markdown = "## Introduction\n\nFirst.\n\n## Introduction\n\nSecond.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000001"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let sections: Vec<_> = nodes.iter().filter(|n| n.heading > 2).collect();
        assert_eq!(sections.len(), 2, "Both section nodes must survive");

        // First Introduction: claims the title-derived slug "introduction"
        assert_eq!(sections[0].title().as_deref(), Some("Introduction"));
        assert_eq!(
            sections[0].id().as_deref(),
            Some("introduction"),
            "First section gets the title-derived slug"
        );

        // Second Introduction: collision detected → Case B assigns slug-N → id() returns "introduction-2"
        assert_eq!(sections[1].title().as_deref(), Some("Introduction"));
        assert_eq!(
            sections[1].id().as_deref(),
            Some("introduction-2"),
            "Second section must get a slug-N id — collision triggers stable slug-N assignment"
        );
    }

    #[test]
    fn test_intra_document_heading_anchor_collision_explicit_anchor() {
        // Two headings with explicit {#shared} anchor: first keeps the anchor; second's explicit
        // anchor is stripped but its title slug "beta" is free, so id() returns Some("beta").
        // This is Case A: explicit collision but title fallback is available and stable.
        use crate::codec::DocCodec;
        use toml_edit::DocumentMut;

        let markdown = "## Alpha {#shared}\n\nFirst.\n\n## Beta {#shared}\n\nSecond.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000002"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let sections: Vec<_> = nodes.iter().filter(|n| n.heading > 2).collect();
        assert_eq!(sections.len(), 2, "Both section nodes must survive");

        let alpha = sections
            .iter()
            .find(|s| s.title().as_deref() == Some("Alpha"))
            .unwrap();
        let beta = sections
            .iter()
            .find(|s| s.title().as_deref() == Some("Beta"))
            .unwrap();

        // First claimant keeps the anchor
        assert_eq!(
            alpha.id().as_deref(),
            Some("shared"),
            "First explicit anchor must be kept"
        );
        // Second claimant: explicit anchor collided but title slug "beta" is free →
        // Case A: explicit id removed, title fallback gives a stable anchor.
        assert_eq!(
            beta.id().as_deref(),
            Some("beta"),
            "When explicit anchor collides but title slug is free, title fallback must apply"
        );
    }

    #[test]
    fn test_intra_document_heading_anchor_collision_explicit_vs_title() {
        // An explicit anchor on the first heading blocks the later title-derived slug from
        // claiming the same anchor. The second heading has no explicit id and its title slug
        // "later" is already taken — Case B assigns "later-2" as a stable slug-N id.
        use crate::codec::DocCodec;
        use crate::paths::path::to_anchor;
        use toml_edit::DocumentMut;

        // "## Later" has title "Later" → to_anchor = "later", same as {#later} on first heading.
        let markdown = "## First {#later}\n\nFirst.\n\n## Later\n\nSecond.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000003"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let sections: Vec<_> = nodes.iter().filter(|n| n.heading > 2).collect();
        assert_eq!(sections.len(), 2);

        let first = sections
            .iter()
            .find(|s| s.title().as_deref() == Some("First"))
            .unwrap();
        let later = sections
            .iter()
            .find(|s| s.title().as_deref() == Some("Later"))
            .unwrap();

        // Explicit anchor on "First" is kept
        assert_eq!(first.id().as_deref(), Some("later"));
        // "Later" has no explicit anchor; title slug "later" is already taken by "First {#later}".
        // Collision detected → Case B assigns slug-N "later-2" as a stable addressable anchor.
        assert_eq!(
            later.id().as_deref(),
            Some("later-2"),
            "Title-derived slug blocked by prior explicit anchor — Case B assigns slug-N fallback"
        );
        assert_eq!(to_anchor(&later.title().unwrap()), "later");
    }

    #[test]
    fn test_intra_document_heading_anchor_no_false_collision() {
        // Distinct titles must not trigger a collision; each gets its title-derived slug via id().
        use crate::codec::DocCodec;
        use toml_edit::DocumentMut;

        let markdown = "## Alpha\n\nFirst.\n\n## Beta\n\nSecond.\n\n## Gamma\n\nThird.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000004"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let sections: Vec<_> = nodes.iter().filter(|n| n.heading > 2).collect();
        assert_eq!(
            sections.len(),
            3,
            "All three distinct sections must be present"
        );

        // No explicit anchors, no collisions — each section gets its title-derived slug
        let expected: std::collections::HashMap<&str, &str> =
            [("Alpha", "alpha"), ("Beta", "beta"), ("Gamma", "gamma")]
                .into_iter()
                .collect();
        for section in &sections {
            let title = section.title().unwrap_or_default();
            let expected_id = expected.get(title.as_str()).copied().unwrap_or("");
            assert_eq!(
                section.id().as_deref(),
                Some(expected_id),
                "Section '{}' should have title-derived id '{}'",
                title,
                expected_id
            );
        }
    }

    // ========================================================================
    // Magic Continue ID Tests
    // ========================================================================

    #[test]
    fn test_magic_continue_id_merges_into_prior_section() {
        // A heading with {#__continue} must fold into the preceding section node,
        // producing one section instead of two.
        use crate::codec::DocCodec;
        use toml_edit::DocumentMut;

        let markdown = "## My Section\n\nFirst paragraph.\n\n## Continued {#__continue}\n\nSecond paragraph.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000010"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        let mut diagnostics = vec![];
        codec
            .parse(
                markdown,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // No diagnostics expected — __continue is a clean merge, not an error.
        assert!(
            diagnostics.is_empty(),
            "Expected no diagnostics, got: {diagnostics:?}"
        );

        let sections: Vec<_> = codec
            .nodes()
            .into_iter()
            .filter(|n| n.heading > 2)
            .collect();
        assert_eq!(
            sections.len(),
            1,
            "Both headings should fold into one section node"
        );
        assert_eq!(
            sections[0].title().as_deref(),
            Some("My Section"),
            "Section title should come from the first heading"
        );
        // __continue must NOT appear as the node id — the node keeps its title-derived anchor.
        assert_ne!(
            sections[0].id().as_deref(),
            Some(MAGIC_CONTINUE_ID),
            "__continue must not leak into the node id"
        );
    }

    #[test]
    fn test_magic_continue_id_mid_document() {
        // __continue on a middle heading merges only with its immediate predecessor;
        // the heading before and after are unaffected.
        use crate::codec::DocCodec;
        use toml_edit::DocumentMut;

        let markdown =
            "## Alpha\n\nFirst.\n\n## Beta {#__continue}\n\nStill Beta.\n\n## Gamma\n\nThird.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000011"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let sections: Vec<_> = codec
            .nodes()
            .into_iter()
            .filter(|n| n.heading > 2)
            .collect();
        assert_eq!(
            sections.len(),
            2,
            "Alpha+Beta merge into one, Gamma is separate — expect 2 sections"
        );

        let titles: Vec<_> = sections.iter().filter_map(|s| s.title()).collect();
        assert!(
            titles.contains(&"Alpha".to_string()),
            "Alpha section must exist"
        );
        assert!(
            titles.contains(&"Gamma".to_string()),
            "Gamma section must exist"
        );
        assert!(
            !titles.iter().any(|t| t == "Beta"),
            "Beta must not appear as its own section title"
        );
    }

    #[test]
    fn test_magic_continue_id_content_folds_into_prior_events() {
        // The event stream of the __continue heading must be appended to the prior
        // node's event stream so that source round-trip and HTML rendering see both
        // paragraphs under the same node.
        use crate::codec::DocCodec;
        use toml_edit::DocumentMut;

        let markdown =
            "## Section A\n\nParagraph one.\n\n## Section B {#__continue}\n\nParagraph two.\n";

        let mut codec = MdCodec::new();
        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000012"));
        doc.insert("schema", value("Document"));
        doc.insert("title", value("Test Doc"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let section_events: Vec<_> = codec
            .current_events
            .iter()
            .filter(|(p, _)| p.heading > 2)
            .collect();
        assert_eq!(section_events.len(), 1);

        let (_, events) = &section_events[0];
        // Reconstruct the text from the merged event stream — both paragraphs must appear.
        let text =
            MdCodec::events_to_text(markdown, events.iter().map(|(e, r)| (e.clone(), r.clone())))
                .unwrap_or_default();

        assert!(
            text.contains("Paragraph one"),
            "Merged event stream must contain first paragraph. Got: {text}"
        );
        assert!(
            text.contains("Paragraph two"),
            "Merged event stream must contain second paragraph. Got: {text}"
        );
        assert!(
            text.contains("__continue"),
            "The __continue heading must be preserved in the event stream. Got: {text}"
        );
    }

    // ========================================================================
    // Link Manipulation Tests
    // ========================================================================

    #[test]
    fn test_parse_title_attribute_bref_only() {
        let parts = parse_title_attribute("bref://abc123456789");
        assert!(parts.bref.is_some());
        assert_eq!(parts.bref.unwrap().to_string(), "abc123456789");
        assert!(!parts.auto_title);
        assert_eq!(parts.user_words, None);
    }

    #[test]
    fn test_parse_title_attribute_with_auto_title() {
        let parts = parse_title_attribute("bref://abc123456789 {\"auto_title\":true}");
        assert!(parts.bref.is_some());
        assert!(parts.auto_title);
        assert_eq!(parts.user_words, None);
    }

    #[test]
    fn test_parse_title_attribute_with_user_words() {
        let parts = parse_title_attribute("bref://abc123456789 My Custom Note");
        assert!(parts.bref.is_some());
        assert!(!parts.auto_title);
        assert_eq!(parts.user_words, Some("My Custom Note".to_string()));
    }

    #[test]
    fn test_parse_title_attribute_full() {
        let parts = parse_title_attribute("bref://abc123456789 {\"auto_title\":true} My Note");
        assert!(parts.bref.is_some());
        assert_eq!(parts.bref.unwrap().to_string(), "abc123456789");
        assert!(parts.auto_title);
        assert_eq!(parts.user_words, Some("My Note".to_string()));
    }

    #[test]
    fn test_parse_title_attribute_bid_format() {
        let bid_str = "00000000-0000-0000-0000-000000000001";
        let parts = parse_title_attribute(&format!("bid://{bid_str}"));
        assert!(parts.bref.is_some());
        // BID should be converted to Bref (namespace)
        // The namespace is derived from the BID using a hash function
        let expected_bref = Bid::try_from(bid_str).unwrap().bref();
        assert_eq!(parts.bref.unwrap().to_string(), expected_bref.to_string());
    }

    #[test]
    fn test_parse_title_attribute_no_bref() {
        let parts = parse_title_attribute("Just some words");
        assert!(parts.bref.is_none());
        assert!(!parts.auto_title);
        assert_eq!(parts.user_words, Some("Just some words".to_string()));
    }

    #[test]
    fn test_parse_title_attribute_empty() {
        let parts = parse_title_attribute("");
        assert!(parts.bref.is_none());
        assert!(!parts.auto_title);
        assert_eq!(parts.user_words, None);
    }

    #[test]
    fn test_build_title_attribute_bref_only() {
        let attr = build_title_attribute("bref://abc123456789", false, None, None);
        assert_eq!(attr, "bref://abc123456789");
    }

    #[test]
    fn test_build_title_attribute_with_auto_title() {
        let attr = build_title_attribute("bref://abc123456789", true, None, None);
        assert_eq!(attr, "bref://abc123456789 {\"auto_title\":true}");
    }

    #[test]
    fn test_build_title_attribute_with_user_words() {
        let attr = build_title_attribute("bref://abc123456789", false, None, Some("My Note"));
        assert_eq!(attr, "bref://abc123456789 My Note");
    }

    #[test]
    fn test_build_title_attribute_full() {
        let attr = build_title_attribute("bref://abc123456789", true, None, Some("My Note"));
        assert_eq!(attr, "bref://abc123456789 {\"auto_title\":true} My Note");
    }

    #[test]
    fn test_build_title_attribute_with_rel_single() {
        let attr =
            build_title_attribute("bref://abc123456789", false, Some("pragmatic:source"), None);
        assert_eq!(attr, "bref://abc123456789 {\"rel\":\"pragmatic:source\"}");
    }

    #[test]
    fn test_build_title_attribute_with_rel_multi() {
        let attr = build_title_attribute(
            "bref://abc123456789",
            false,
            Some("pragmatic:source epistemic:source"),
            None,
        );
        assert_eq!(
            attr,
            "bref://abc123456789 {\"rel\":\"pragmatic:source epistemic:source\"}"
        );
    }

    #[test]
    fn test_build_title_attribute_with_rel_and_auto_title() {
        let attr =
            build_title_attribute("bref://abc123456789", true, Some("pragmatic:source"), None);
        assert_eq!(
            attr,
            "bref://abc123456789 {\"auto_title\":true,\"rel\":\"pragmatic:source\"}"
        );
    }

    #[test]
    fn test_parse_title_attribute_with_rel() {
        let title = "bref://abc123456789 {\"rel\":\"pragmatic:source\"}";
        let parts = parse_title_attribute(title);
        assert_eq!(parts.rel, Some("pragmatic:source".to_string()));
        assert!(!parts.auto_title);
    }

    #[test]
    fn test_parse_title_attribute_with_rel_multi() {
        let title = "bref://abc123456789 {\"rel\":\"pragmatic:source epistemic:source\"}";
        let parts = parse_title_attribute(title);
        assert_eq!(
            parts.rel,
            Some("pragmatic:source epistemic:source".to_string())
        );
    }

    #[test]
    fn test_parse_build_roundtrip_with_rel() {
        let original = "bref://abc123456789 {\"rel\":\"pragmatic:source\"} Custom Text";
        let parts = parse_title_attribute(original);
        let rebuilt = build_title_attribute(
            &format!("bref://{}", parts.bref.unwrap()),
            parts.auto_title,
            parts.rel.as_deref(),
            parts.user_words.as_deref(),
        );
        assert_eq!(original, rebuilt);
    }

    #[test]
    fn test_parse_build_roundtrip_with_rel_and_auto_title() {
        let original =
            "bref://abc123456789 {\"auto_title\":true,\"rel\":\"epistemic:source\"} My Note";
        let parts = parse_title_attribute(original);
        let rebuilt = build_title_attribute(
            &format!("bref://{}", parts.bref.unwrap()),
            parts.auto_title,
            parts.rel.as_deref(),
            parts.user_words.as_deref(),
        );
        assert_eq!(original, rebuilt);
    }

    #[test]
    fn test_make_relative_path_same_dir() {
        let ap = AnchorPath::from("docs/guide.md");
        let rel = ap.join("api.md");
        assert_eq!(rel, "docs/api.md");
    }

    #[test]
    fn test_make_relative_path_nested() {
        let ap = AnchorPath::from("docs/guide.md");
        let rel = ap.path_to("docs/reference/api.md", true);
        assert_eq!(rel, "reference/api.md");
    }

    #[test]
    fn test_make_relative_path_parent() {
        let ap = AnchorPath::from("docs/reference/guide.md");
        let rel = ap.join("../../docs/guide.md");
        assert_eq!(rel, "docs/guide.md");
    }

    #[test]
    fn test_make_relative_path_root_to_nested() {
        let ap = AnchorPath::from("README.md");
        let rel = ap.join("docs/guide.md");
        assert_eq!(rel, "docs/guide.md");
    }

    #[test]
    fn test_make_relative_path_nested_to_root() {
        let ap = AnchorPath::from("docs/guide.md");
        let rel = ap.join("README.md");
        assert_eq!(rel, "docs/README.md");
    }

    #[test]
    fn test_parse_build_roundtrip() {
        let original = "bref://abc123456789 {\"auto_title\":true} Custom Text";
        let parts = parse_title_attribute(original);
        let rebuilt = build_title_attribute(
            &format!("bref://{}", parts.bref.unwrap()),
            parts.auto_title,
            parts.rel.as_deref(),
            parts.user_words.as_deref(),
        );
        assert_eq!(original, rebuilt);
    }

    #[test]
    fn test_inline_elements_in_headings() {
        // Test that all valid inline CommonMark elements in headings are supported
        // without warnings: HTML, code, emphasis, strong, links, images, etc.
        let markdown = r#"---
title: "Test Document"
id: "test"
---

# Regular Heading

### <Method Title> with `code` and **bold**

Some content here.

## Another *emphasis* and [link](url) and ![image](path)

More content.
"#;

        let parser = MdParser::new_ext(markdown, buildonomy_md_options());
        let events: Vec<_> = parser.collect();

        // Verify that various inline events are present in the parsed events
        let has_inline_html = events.iter().any(|e| matches!(e, MdEvent::InlineHtml(_)));
        let has_code = events.iter().any(|e| matches!(e, MdEvent::Code(_)));
        let has_emphasis = events
            .iter()
            .any(|e| matches!(e, MdEvent::Start(MdTag::Emphasis)));
        let has_strong = events
            .iter()
            .any(|e| matches!(e, MdEvent::Start(MdTag::Strong)));

        assert!(has_inline_html, "Expected InlineHtml events");
        assert!(has_code, "Expected Code events");
        assert!(has_emphasis, "Expected Emphasis events");
        assert!(has_strong, "Expected Strong events");

        // The actual test is that update_or_insert_frontmatter doesn't panic or warn
        // This is implicitly tested by the watch service integration tests
    }

    #[test]
    fn test_inline_html_code_in_heading_generates_id() {
        // Tests that titles are captured correctly for headings containing inline HTML and code,
        // and that IRNode::id() returns the title-derived slug for sections without explicit anchors.
        use crate::codec::DocCodec;
        use crate::paths::path::to_anchor;
        use toml_edit::DocumentMut;

        let markdown = r#"### <Method Title>

Content under method title.

### Using `code` in Title

Content under code title.

### Mixed <HTML> and `code` Content

Mixed content.
"#;

        let mut codec = MdCodec::new();

        let mut doc = DocumentMut::new();
        doc.insert("bid", value("01234567-89ab-cdef-0123-456789abcdef"));
        doc.insert("title", value("Test Document"));

        let proto = IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            heading: 2,
        };

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        // Should have 4 nodes: document + 3 sections
        assert_eq!(
            nodes.len(),
            4,
            "Expected 4 nodes (1 doc + 3 sections), got {}",
            nodes.len()
        );

        // Find the section nodes (heading > 2)
        let sections: Vec<_> = nodes.iter().filter(|n| n.heading > 2).collect();
        assert_eq!(sections.len(), 3, "Expected 3 section nodes");

        // Check that InlineHtml heading captured title and has no explicit id
        let method_section = sections
            .iter()
            .find(|s| {
                s.document
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|t| t.contains("Method Title"))
                    .unwrap_or(false)
            })
            .expect("Should find <Method Title> section");

        // No explicit {#anchor} — IRNode::id() falls back to to_anchor(title)
        let method_title = method_section.title().expect("Should have title");
        assert_eq!(
            to_anchor(&method_title),
            "method-title",
            "Title anchor slug should match expected value"
        );
        assert_eq!(
            method_section.id().as_deref(),
            Some("method-title"),
            "IRNode::id() should return title-derived slug for <Method Title> section"
        );

        // Check that Code heading captured title correctly
        let code_section = sections
            .iter()
            .find(|s| {
                s.document
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|t| t.contains("Using") && t.contains("code"))
                    .unwrap_or(false)
            })
            .expect("Should find code section");

        let code_title = code_section.title().expect("Should have title");
        assert_eq!(
            to_anchor(&code_title),
            "using-code-in-title",
            "Code title anchor slug should match expected value"
        );
        assert_eq!(
            code_section.id().as_deref(),
            Some("using-code-in-title"),
            "IRNode::id() should return title-derived slug for code-in-heading section"
        );

        // Check mixed content
        let mixed_section = sections
            .iter()
            .find(|s| {
                s.document
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|t| t.contains("Mixed"))
                    .unwrap_or(false)
            })
            .expect("Should find mixed section");

        let mixed_title = mixed_section.title().expect("Should have title");
        assert_eq!(
            to_anchor(&mixed_title),
            "mixed-html-and-code-content",
            "Mixed content title anchor slug should match expected value"
        );
        assert_eq!(
            mixed_section.id().as_deref(),
            Some("mixed-html-and-code-content"),
            "IRNode::id() should return title-derived slug for mixed-content section"
        );
    }

    #[test]
    fn test_heading_id_round_trip_with_cmark_resume() {
        // Test that heading IDs are written back using cmark_resume_with_source_range_and_options
        use pulldown_cmark::{Event as MdEvent, Tag as MdTag};
        use pulldown_cmark_to_cmark::cmark_resume_with_source_range_and_options;

        let markdown = "## My Heading";
        let parser = MdParser::new_ext(markdown, buildonomy_md_options());
        let events: Vec<(MdEvent, Option<Range<usize>>)> = parser
            .into_offset_iter()
            .map(|(e, r)| (e, Some(r)))
            .collect();

        // Modify the heading to add an ID
        let modified_events: Vec<(MdEvent, Option<Range<usize>>)> = events
            .into_iter()
            .map(|(e, r)| {
                if let MdEvent::Start(MdTag::Heading {
                    level,
                    id: _,
                    classes,
                    attrs,
                }) = e
                {
                    // Clear the range so cmark_resume uses the event data instead of source
                    (
                        MdEvent::Start(MdTag::Heading {
                            level,
                            id: Some(CowStr::from("my-heading")),
                            classes,
                            attrs,
                        }),
                        None, // Clear range to force using modified event
                    )
                } else {
                    (e, r)
                }
            })
            .collect();

        // Write back using cmark_resume
        let mut buf = String::new();
        let options = CmarkToCmarkOptions::default();
        let events_with_refs = modified_events.iter().map(|(e, r)| (e, r.clone()));
        cmark_resume_with_source_range_and_options(
            events_with_refs,
            markdown,
            &mut buf,
            None,
            options,
        )
        .unwrap();

        // Verify ID was written
        assert!(
            buf.contains("{ #my-heading }") || buf.contains("{#my-heading}"),
            "Should write heading ID when range is cleared. Got: {buf}"
        );
    }

    #[test]
    fn test_generate_html_basic() {
        use crate::codec::DocCodec;

        let markdown = r#"---
bid = "01234567-89ab-cdef-0123-456789abcdef"
title = "Test Document"
---

# Getting Started

This is a test document.

## Installation {#a1b2c3d4e5f6}

Install the software.
"#;

        let mut codec = MdCodec::new();
        let mut proto = IRNode::default();
        proto
            .document
            .insert("bid", value("01234567-89ab-cdef-0123-456789abcdef"));
        proto.document.insert("title", value("Test Document"));

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, pairs, _layout) = &fragments[0];
        let (_, html_content) = &pairs[0];

        // Verify HTML body content (fragments don't include DOCTYPE, html, head tags)
        assert!(html_content.contains("<h1"), "Missing h1 heading");
        assert!(
            html_content.contains("Getting Started"),
            "Missing heading content"
        );
        assert!(html_content.contains("<p>"), "Missing paragraph tag");

        // Verify markdown content was converted to HTML
        assert!(html_content.contains("<h1"), "Missing h1 heading");
        assert!(
            html_content.contains("Getting Started"),
            "Missing heading text"
        );
        assert!(html_content.contains("<h2"), "Missing h2 heading");
        assert!(
            html_content.contains("Installation"),
            "Missing subheading text"
        );
        assert!(html_content.contains("<p>"), "Missing paragraph tag");
    }

    #[test]
    fn test_generate_html_minimal_metadata() {
        use crate::codec::DocCodec;

        let markdown = r#"---
bid = "12345678-1234-5678-1234-567812345678"
title = "Minimal Doc"
---

# Simple Heading

Content here.

## Section Without BID

This section has no explicit BID.
"#;

        let mut codec = MdCodec::new();
        let mut proto = IRNode::default();
        proto
            .document
            .insert("bid", value("12345678-1234-5678-1234-567812345678"));
        proto.document.insert("title", value("Minimal Doc"));

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, pairs, _layout) = &fragments[0];
        let (_, html_content) = &pairs[0];

        // Verify HTML body content (fragments don't include metadata)
        assert!(html_content.contains("<h1"), "Missing h1 heading");
        assert!(
            html_content.contains("Content here"),
            "Missing body content"
        );
        assert!(html_content.contains("<p>"), "Missing paragraph tag");
    }

    #[test]
    fn test_generate_html_link_rewriting() {
        init_logging();
        use crate::codec::DocCodec;

        let markdown = r#"---
bid = "12345678-1234-5678-1234-567812345678"
title = "Link Test"
---

# Links Test

Link to [another doc](./other.md "bref://doc123 auto title").
Link with anchor [section link](docs/page.md#section-1 "bref://doc456").
External .md link without bref [external](https://example.com/doc.md).
Already HTML [html link](./page.html "bref://doc789").
"#;

        let mut codec = MdCodec::new();
        let mut proto = IRNode::default();
        proto
            .document
            .insert("bid", value("12345678-1234-5678-1234-567812345678"));
        proto.document.insert("title", value("Link Test"));

        codec
            .parse(
                markdown,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, pairs, _layout) = &fragments[0];
        let (_, html_content) = &pairs[0];

        // Verify .md links WITH bref:// are rewritten to .html
        assert!(
            html_content.contains("href=\"other.html\""),
            "path: {_path}\nExpected href=\"other.html\" but got:\n{html_content}"
        );
        assert!(
            html_content.contains("href=\"docs/page.html#section-1\""),
            "path: {}\n{}",
            _path,
            html_content
        );

        // Verify .md links WITHOUT bref:// are NOT rewritten (we didn't parse them)
        assert!(
            html_content.contains("href=\"https://example.com/doc.md\""),
            "Expected href=\"https://example.com/doc.md\" but got\n{html_content}"
        );

        // Verify already-.html links with bref:// are normalized
        assert!(
            html_content.contains("href=\"./page.html\""),
            "Expected href=\"./page.html\", received\n{html_content}"
        );

        // Verify only links with bref:// were rewritten
        assert!(
            !html_content.contains("href=\"./other.md\""),
            "Expected not to have href=\"./other.html\", received\n{html_content}"
        );
        assert!(
            !html_content.contains("href=\"docs/page.md#"),
            "Expected not to have href=\"docs/page.md#\", received\n{html_content}"
        );
    }

    // Note: Integration test for static asset tracking needed with full GraphBuilder flow
    // MdCodec::parse only creates IRNodes; relations are created by GraphBuilder

    // ── {implements} / {end} block directive ─────────────────────────────────

    /// Parse `content` through a fresh MdCodec and return the flattened upstream relations
    /// from all proto nodes, plus any diagnostics.
    fn parse_upstream(content: &str) -> (Vec<IntermediateRelation>, Vec<ParseDiagnostic>) {
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let relations = codec
            .current_events
            .iter()
            .flat_map(|(node, _)| node.upstream.iter().cloned())
            .collect();
        (relations, diagnostics)
    }

    #[test]
    fn test_implements_block_links_are_pragmatic() {
        let content = "\
---
id = \"doc\"
---

# Doc

````{implements}
````

[some target](target.md)

````{end}
````
";
        let (relations, diagnostics) = parse_upstream(content);
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "expected exactly one Pragmatic relation; got: {relations:?}"
        );
        let epistemic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Epistemic)
            .collect();
        assert!(
            epistemic.is_empty(),
            "expected no Epistemic relations; got: {relations:?}"
        );
    }

    #[test]
    fn test_links_outside_implements_block_are_epistemic() {
        let content = "\
---
id = \"doc\"
---

# Doc

[before](before.md)

````{implements}
````

[inside](inside.md)

````{end}
````

[after](after.md)
";
        let (relations, _) = parse_upstream(content);
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        let epistemic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Epistemic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "only 'inside' should be Pragmatic; got: {pragmatic:?}"
        );
        assert_eq!(
            epistemic.len(),
            2,
            "before and after should be Epistemic; got: {epistemic:?}"
        );
    }

    #[test]
    fn test_implements_block_auto_closes_on_heading() {
        let content = "\
---
id = \"doc\"
---

# Doc

````{implements}
````

[inside](inside.md)

## Next Section

[after heading](after.md)
";
        let (relations, diagnostics) = parse_upstream(content);
        // Heading auto-close now emits a warning about the unclosed context.
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| matches!(d, ParseDiagnostic::Warning { .. }))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one implicit-close warning at heading; got: {diagnostics:?}"
        );
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        let epistemic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Epistemic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "only 'inside' should be Pragmatic; got: {pragmatic:?}"
        );
        assert_eq!(
            epistemic.len(),
            1,
            "'after heading' should be Epistemic; got: {epistemic:?}"
        );
    }

    #[test]
    fn test_stray_end_directive_emits_warning() {
        let content = "\
---
id = \"doc\"
---

# Doc

````{end}
````
";
        let (_, diagnostics) = parse_upstream(content);
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| matches!(d, ParseDiagnostic::Warning { .. }))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one warning for stray {{end}}; got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_nested_implements_stacks_and_both_are_pragmatic() {
        // With the stack model, nested {implements} pushes a second context.
        // {end} pops the inner one, restoring the outer. Both links are Pragmatic.
        let content = "\
---
id = \"doc\"
---

# Doc

````{implements}
````

[first](first.md)

````{implements}
````

[second](second.md)

````{end}
````

[third](third.md)

````{end}
````
";
        let (relations, diagnostics) = parse_upstream(content);
        // No warnings — nested opens are valid with the stack model.
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings with stack model; got: {diagnostics:?}"
        );
        // All three links should be Pragmatic — outer context restored after inner {end}.
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            3,
            "all three links should be Pragmatic; got: {relations:?}"
        );
    }

    #[test]
    fn test_implements_directives_suppressed_from_html() {
        init_logging();
        let content = "\
---
id = \"doc\"
---

# Doc

````{implements}
````

[target](target.md)

````{end}
````
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let html = codec.render_html_body();
        assert!(
            !html.contains("implements"),
            "directive name must not appear in HTML; html:\n{html}"
        );
        assert!(
            !html.contains("<pre>"),
            "no <pre> block should be emitted for directives; html:\n{html}"
        );
    }

    #[test]
    fn test_implements_directive_round_trips_in_source() {
        init_logging();
        let content = "\
---
id = \"doc\"
---

# Doc

````{implements}
````

[target](target.md)

````{end}
````
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let source = codec
            .generate_source()
            .expect("generate_source should return Some");
        assert!(
            source.contains("````{implements}"),
            "implements directive must be preserved in source; source:\n{source}"
        );
        assert!(
            source.contains("````{end}"),
            "end directive must be preserved in source; source:\n{source}"
        );
    }

    // ── codespan toggle directives ────────────────────────────────────────────

    #[test]
    fn test_uses_codespan_produces_pragmatic_upstream() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{uses}`

[req](req.md)

`{end}`
";
        let (relations, diagnostics) = parse_upstream(content);
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "expected one Pragmatic upstream; got: {relations:?}"
        );
    }

    #[test]
    fn test_used_by_codespan_produces_pragmatic_downstream() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{used_by}`

[consumer](consumer.md)

`{end}`
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let downstream: Vec<_> = codec
            .current_events
            .iter()
            .flat_map(|(node, _)| node.downstream.iter().cloned())
            .collect();
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        assert_eq!(
            downstream.len(),
            1,
            "expected one downstream; got: {downstream:?}"
        );
        assert_eq!(downstream[0].kind, WeightKind::Pragmatic);
    }

    #[test]
    fn test_draws_from_codespan_produces_epistemic_upstream() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{draws_from}`

[concept](concept.md)

`{end}`
";
        let (relations, diagnostics) = parse_upstream(content);
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        let epistemic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Epistemic)
            .collect();
        assert_eq!(
            epistemic.len(),
            1,
            "expected one Epistemic upstream; got: {relations:?}"
        );
    }

    #[test]
    fn test_underlies_codespan_produces_epistemic_downstream() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{underlies}`

[derived](derived.md)

`{end}`
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let downstream: Vec<_> = codec
            .current_events
            .iter()
            .flat_map(|(node, _)| node.downstream.iter().cloned())
            .collect();
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        assert_eq!(
            downstream.len(),
            1,
            "expected one Epistemic downstream; got: {downstream:?}"
        );
        assert_eq!(downstream[0].kind, WeightKind::Epistemic);
    }

    #[test]
    fn test_precise_relation_form_epistemic_sink() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{relation}kind=epistemic, ref=sink`

[derived](derived.md)

`{end}`
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let downstream: Vec<_> = codec
            .current_events
            .iter()
            .flat_map(|(node, _)| node.downstream.iter().cloned())
            .collect();
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        assert_eq!(
            downstream.len(),
            1,
            "expected one downstream; got: {downstream:?}"
        );
        assert_eq!(downstream[0].kind, WeightKind::Epistemic);
    }

    #[test]
    fn test_precise_relation_form_pragmatic_source() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{relation}kind=pragmatic, ref=source`

[req](req.md)

`{end}`
";
        let (relations, diagnostics) = parse_upstream(content);
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "expected one Pragmatic upstream; got: {relations:?}"
        );
    }

    #[test]
    fn test_custom_verb_registration_and_use() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{relation}name=mitigates, kind=pragmatic, ref=source`

`{mitigates}`

[hazard](hazard.md)

`{end}`
";
        let (relations, diagnostics) = parse_upstream(content);
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "custom verb should produce Pragmatic upstream; got: {relations:?}"
        );
    }

    #[test]
    fn test_custom_verb_shadowing_builtin_emits_warning() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{relation}name=uses, kind=epistemic, ref=source`
";
        let (_, diagnostics) = parse_upstream(content);
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| matches!(d, ParseDiagnostic::Warning { .. }))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "expected shadow warning; got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_stray_end_codespan_emits_warning() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{end}`
";
        let (_, diagnostics) = parse_upstream(content);
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| matches!(d, ParseDiagnostic::Warning { .. }))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "expected one warning for stray `{{end}}`; got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_codespan_nested_stack_restores_outer_context() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{uses}`

[first](first.md)

`{draws_from}`

[second](second.md)

`{end}`

[third](third.md)

`{end}`
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let upstream: Vec<_> = codec
            .current_events
            .iter()
            .flat_map(|(node, _)| node.upstream.iter().cloned())
            .collect();
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
        // first and third: Pragmatic (uses context); second: Epistemic (draws_from context)
        let pragmatic: Vec<_> = upstream
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        let epistemic: Vec<_> = upstream
            .iter()
            .filter(|r| r.kind == WeightKind::Epistemic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            2,
            "first and third should be Pragmatic; got: {upstream:?}"
        );
        assert_eq!(
            epistemic.len(),
            1,
            "second should be Epistemic; got: {upstream:?}"
        );
    }

    #[test]
    fn test_missing_end_warns_at_heading_boundary() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{uses}`

[req](req.md)

## Next Section

[other](other.md)
";
        let (_, diagnostics) = parse_upstream(content);
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| matches!(d, ParseDiagnostic::Warning { .. }))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "expected one implicit-close warning; got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_codespan_directives_suppressed_from_html() {
        let content = "\
---
id = \"doc\"
---

# Doc

`{uses}`

[req](req.md)

`{end}`
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let html = codec.render_html_body();
        assert!(
            !html.contains("{uses}"),
            "directive must not appear in HTML; html:\n{html}"
        );
        assert!(
            !html.contains("{end}"),
            "`{{end}}` must not appear in HTML; html:\n{html}"
        );
        assert!(
            !html.contains("<code>"),
            "no <code> tag for directives; html:\n{html}"
        );
    }

    #[test]
    fn test_links_outside_codespan_context_are_default_epistemic() {
        let content = "\
---
id = \"doc\"
---

# Doc

[before](before.md)

`{uses}`

[inside](inside.md)

`{end}`

[after](after.md)
";
        let (relations, _) = parse_upstream(content);
        let pragmatic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Pragmatic)
            .collect();
        let epistemic: Vec<_> = relations
            .iter()
            .filter(|r| r.kind == WeightKind::Epistemic)
            .collect();
        assert_eq!(
            pragmatic.len(),
            1,
            "only 'inside' should be Pragmatic; got: {relations:?}"
        );
        assert_eq!(
            epistemic.len(),
            2,
            "before and after should be Epistemic; got: {relations:?}"
        );
    }

    #[test]
    fn test_unrecognized_codespan_passes_through_silently() {
        let content = "\
---
id = \"doc\"
---

# Doc

Use `{variable}` in your code.
";
        let (relations, diagnostics) = parse_upstream(content);
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "unrecognized {{...}} span must not emit warning; got: {diagnostics:?}"
        );
        assert!(
            relations.is_empty(),
            "no relations should be produced; got: {relations:?}"
        );
    }

    // ── {requirements_table} directive ───────────────────────────────────────

    #[test]
    fn test_requirements_table_sets_should_defer() {
        let content = "\
---
id = \"doc\"
---

# Doc

````{requirements_table}
````
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(
            codec.should_defer(),
            "should_defer() must be true when {{requirements_table}} is present"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| !matches!(d, ParseDiagnostic::Warning { .. })),
            "expected no warnings; got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_no_requirements_table_does_not_defer() {
        let content = "\
---
id = \"doc\"
---

# Doc

Some prose without any directive.
";
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(
            !codec.should_defer(),
            "should_defer() must be false when no {{requirements_table}} is present"
        );
    }

    #[test]
    fn test_requirements_table_sentinel_injected_in_generate_html() {
        use crate::codec::myst::sentinel;
        init_logging();
        let content = "\
---
id = \"doc\"
---

# Doc

Before table.

````{requirements_table}
````

After table.
";
        let mut codec = MdCodec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let fragments = codec.generate_html().expect("generate_html should succeed");
        assert_eq!(fragments.len(), 1);
        let (_, pairs, _) = &fragments[0];
        let (_, body) = &pairs[0];

        assert!(
            body.contains(sentinel("requirements_table").as_str()),
            "sentinel must be present in HTML output; body:\n{body}"
        );
        assert!(
            !body.contains("<!-- noet-requirements-table -->"),
            "intermediate marker must not appear in HTML output; body:\n{body}"
        );
        assert!(
            !body.contains("requirements_table"),
            "directive name must not appear in HTML output; body:\n{body}"
        );
        assert!(
            !body.contains("<pre>"),
            "no <pre> block should be emitted for the directive; body:\n{body}"
        );
        // Sentinel is positioned between the prose blocks
        let before_pos = body.find("Before table.").unwrap();
        let after_pos = body.find("After table.").unwrap();
        let sentinel_pos = body.find(sentinel("requirements_table").as_str()).unwrap();
        assert!(
            sentinel_pos > before_pos,
            "sentinel must appear after 'Before table.'"
        );
        assert!(
            sentinel_pos < after_pos,
            "sentinel must appear before 'After table.'"
        );
    }

    #[test]
    fn test_requirements_table_round_trips_in_source() {
        init_logging();
        let content = "\
---
id = \"doc\"
---

# Doc

````{requirements_table}
````
";
        let mut codec = MdCodec::new();
        let proto = IRNode {
            path: "doc.md".to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let source = codec
            .generate_source()
            .expect("generate_source should return Some");
        assert!(
            source.contains("````{requirements_table}"),
            "directive must be preserved verbatim in generate_source; source:\n{source}"
        );
        assert!(
            !source.contains(crate::codec::myst::sentinel("requirements_table").as_str()),
            "sentinel must not appear in generate_source output; source:\n{source}"
        );
    }

    #[test]
    fn test_requirements_table_deferred_html_replaces_sentinel() {
        init_logging();
        let dir = tempfile::tempdir().unwrap();
        let rt_sentinel = crate::codec::myst::sentinel("requirements_table");

        // Simulate what write_fragment produces: an HTML file containing the sentinel.
        let fake_html = format!(
            "<html><body><h1>Doc</h1><p>Before.</p>{}<p>After.</p></body></html>",
            rt_sentinel
        );
        let html_path = dir.path().join("doc.html");
        std::fs::write(&html_path, &fake_html).unwrap();

        // Build a codec that has has_deferred_render=true.
        let content = "\
---
id = \"doc\"
---

# Doc

````{requirements_table}
````
";
        let mut codec = MdCodec::new();
        let proto = IRNode {
            path: dir.path().join("doc.md").to_str().unwrap().to_string(),
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(codec.should_defer());

        // Directly verify the sentinel-replacement logic without a live BeliefBase:
        // read, replace, write — mirrors what generate_deferred_html does internally.
        let content_on_disk = std::fs::read_to_string(&html_path).unwrap();
        assert!(content_on_disk.contains(rt_sentinel.as_str()));

        let placeholder = "<p><em>No requirements found for this section.</em></p>\n";
        let merged = content_on_disk.replace(rt_sentinel.as_str(), placeholder);
        std::fs::write(&html_path, &merged).unwrap();

        let result = std::fs::read_to_string(&html_path).unwrap();
        assert!(
            !result.contains(rt_sentinel.as_str()),
            "sentinel must be replaced; result:\n{result}"
        );
        assert!(
            result.contains("No requirements found"),
            "replacement content must appear; result:\n{result}"
        );
        assert!(
            result.contains("Before.") && result.contains("After."),
            "surrounding prose must be preserved; result:\n{result}"
        );
    }

    // -------------------------------------------------------------------------
    // source_line tests
    // -------------------------------------------------------------------------

    /// Helper: parse markdown with MdCodec and return the list of (heading_level, source_line)
    /// pairs for every IRNode produced.
    fn parse_source_lines(content: &str) -> Vec<(usize, Option<usize>)> {
        use crate::codec::belief_ir::IRNode;
        use crate::codec::DocCodec;

        let mut codec = MdCodec::new();
        let root = IRNode {
            path: "net/doc.md".to_string(),
            heading: 2,
            source_line: Some(1),
            ..Default::default()
        };
        let mut diagnostics = vec![];
        codec
            .parse(
                content,
                root,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        codec
            .nodes()
            .iter()
            .map(|n| (n.heading, n.source_line))
            .collect()
    }

    #[test]
    fn test_source_line_document_root_is_one() {
        let md = "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\nSome prose.\n";
        let nodes = parse_source_lines(md);
        // Only the document root node (heading == 2)
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].0, 2);
        assert_eq!(nodes[0].1, Some(1), "document root must always be line 1");
    }

    #[test]
    fn test_source_line_h1_section() {
        // heading level H1 in markdown → heading == 3 in our model (network=1, doc=2, h1=3)
        let md = "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\n# First Section\n\nProse.\n";
        let nodes = parse_source_lines(md);
        // doc root + 1 section
        assert_eq!(nodes.len(), 2);
        let (lvl, line) = nodes[1];
        assert_eq!(lvl, 3);
        // "# First Section" starts at line 6
        assert_eq!(line, Some(6));
    }

    #[test]
    fn test_source_line_h2_section() {
        let md = "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\n# H1\n\nProse.\n\n## H2 Section\n\nMore prose.\n";
        let nodes = parse_source_lines(md);
        // doc root + H1 + H2
        assert_eq!(nodes.len(), 3);
        // H2 → heading == 4
        let (lvl, line) = nodes[2];
        assert_eq!(lvl, 4);
        // "## H2 Section" is at line 10
        assert_eq!(line, Some(10));
    }

    #[test]
    fn test_source_line_h3_section() {
        let md =
            "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\n# H1\n\n## H2\n\n### H3 Section\n\nProse.\n";
        let nodes = parse_source_lines(md);
        assert_eq!(nodes.len(), 4);
        // H3 → heading == 5
        let (lvl, line) = nodes[3];
        assert_eq!(lvl, 5);
        // "### H3 Section" is at line 10
        assert_eq!(line, Some(10));
    }

    // -------------------------------------------------------------------------
    // MetadataBlock vs horizontal-rule regression
    // -------------------------------------------------------------------------

    /// Regression test: a `---` horizontal rule after a heading must not be
    /// misinterpreted as a YAML metadata block delimiter. Before the fix,
    /// this triggered `debug_assert!(current.accumulator.is_none())`.
    #[test]
    fn test_horizontal_rule_after_heading_not_parsed_as_metadata() {
        use crate::codec::belief_ir::IRNode;
        use crate::codec::DocCodec;

        let md = "---\ntitle = \"Doc\"\nid = \"doc\"\n---\n\n# Section One\n\nSome prose.\n\n---\n\n# Section Two\n\nMore prose.\n";

        let mut codec = MdCodec::new();
        let root = IRNode {
            path: "net/doc.md".to_string(),
            heading: 2,
            source_line: Some(1),
            ..Default::default()
        };
        let mut diagnostics = vec![];
        codec
            .parse(
                md,
                root,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Should produce 3 nodes: doc root, Section One, Section Two.
        // The `---` between the sections is a horizontal rule — not a metadata block.
        let nodes: Vec<_> = codec.nodes().iter().map(|n| n.title()).collect();
        assert_eq!(nodes.len(), 3, "expected 3 nodes, got {nodes:?}");
    }

    /// Frontmatter at the very top of the file (before any heading) must still parse.
    #[test]
    fn test_frontmatter_before_first_heading_still_works() {
        use crate::codec::belief_ir::IRNode;
        use crate::codec::DocCodec;

        let md = "---\ntitle = \"My Title\"\nid = \"my-title\"\n---\n\n# Section\n\nProse.\n";

        let mut codec = MdCodec::new();
        let root = IRNode {
            path: "net/doc.md".to_string(),
            heading: 2,
            source_line: Some(1),
            ..Default::default()
        };
        let mut diagnostics = vec![];
        codec
            .parse(
                md,
                root,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // The frontmatter should have been parsed and merged into the root node.
        let root_node = &codec.nodes()[0];
        assert_eq!(
            root_node.title(),
            Some("My Title".to_string()),
            "frontmatter title should be parsed"
        );
    }

    // -------------------------------------------------------------------------
    // compute_source_url tests
    // -------------------------------------------------------------------------

    /// Build a minimal BeliefBase containing a network node with the given payload
    /// and metadata, then call compute_source_url directly via BeliefContext::new_for_test.
    fn make_source_url(
        net_payload: toml::value::Table,
        net_metadata: toml::value::Table,
        source_line: Option<usize>,
        root_path: &str,
    ) -> Option<String> {
        use crate::beliefbase::{BeliefBase, BeliefContext};
        use crate::codec::belief_ir::IRNode;
        use crate::properties::{BeliefKind, BeliefNode, Bid};

        let net_bid = Bid::new(crate::properties::buildonomy_namespace());
        let net_node = BeliefNode {
            bid: net_bid,
            kind: BeliefKind::Network.into(),
            title: "Test Network".to_string(),
            payload: net_payload,
            metadata: net_metadata,
            ..Default::default()
        };

        let mut states = rustc_hash::FxHashMap::default();
        states.insert(net_bid, net_node);
        let bb = BeliefBase::new(states, crate::beliefbase::BidGraph::default()).unwrap();

        // BeliefContext::new_for_test constructs a context with an empty relations guard,
        // which is sufficient for compute_source_url (only reads root_net + beliefbase()).
        let node_ref = bb.states().get(&net_bid)?;
        let ctx = BeliefContext::new_for_test(node_ref, net_bid, root_path.to_string(), &bb);

        let ir_node = IRNode {
            source_line,
            path: root_path.to_string(),
            heading: 3,
            ..Default::default()
        };

        compute_source_url(&ir_node, &ctx)
    }

    #[test]
    fn test_compute_source_url_no_git_metadata_returns_none() {
        // No metadata["git"] and no payload["git_remote_url"] → None
        let url = make_source_url(
            toml::value::Table::new(),
            toml::value::Table::new(),
            None,
            "docs/guide.md",
        );
        assert!(url.is_none());
    }

    #[test]
    fn test_compute_source_url_from_git_metadata_no_line() {
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let url = make_source_url(toml::value::Table::new(), meta, None, "docs/guide.md");
        assert_eq!(
            url,
            Some("https://github.com/org/repo/blob/main/docs/guide.md".to_string())
        );
    }

    #[test]
    fn test_compute_source_url_from_git_metadata_with_line() {
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let url = make_source_url(toml::value::Table::new(), meta, Some(42), "docs/guide.md");
        assert_eq!(
            url,
            Some("https://github.com/org/repo/blob/main/docs/guide.md#L42".to_string())
        );
    }

    #[test]
    fn test_compute_source_url_detached_head_falls_back_to_head() {
        // No "branch" key in metadata["git"] → falls back to "HEAD"
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        // branch intentionally absent
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let url = make_source_url(toml::value::Table::new(), meta, None, "src/lib.rs");
        assert_eq!(
            url,
            Some("https://github.com/org/repo/blob/HEAD/src/lib.rs".to_string())
        );
    }

    #[test]
    fn test_compute_source_url_payload_override_takes_precedence() {
        // payload["git_remote_url"] overrides metadata["git"]["remote_url"]
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let mut payload = toml::value::Table::new();
        payload.insert(
            "git_remote_url".to_string(),
            toml::Value::String("https://github.com/org/fork".to_string()),
        );

        let url = make_source_url(payload, meta, Some(7), "README.md");
        assert_eq!(
            url,
            Some("https://github.com/org/fork/blob/main/README.md#L7".to_string())
        );
    }

    #[test]
    fn test_compute_source_url_empty_payload_override_suppresses() {
        // payload["git_remote_url"] = "" suppresses source_url entirely
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let mut payload = toml::value::Table::new();
        payload.insert(
            "git_remote_url".to_string(),
            toml::Value::String(String::new()),
        );

        let url = make_source_url(payload, meta, None, "docs/guide.md");
        assert!(
            url.is_none(),
            "empty git_remote_url must suppress source_url"
        );
    }

    #[test]
    fn test_compute_source_url_empty_root_path_returns_none() {
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        // root_path is empty → None
        let url = make_source_url(toml::value::Table::new(), meta, None, "");
        assert!(url.is_none(), "empty root_path must return None");
    }

    #[test]
    fn test_compute_source_url_with_network_prefix() {
        // network_prefix = "tests/network_1" means the network dir is not the git root.
        // root_path = "subnet1/file.md" (network-relative).
        // Expected full path: "tests/network_1/subnet1/file.md".
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        git.insert(
            "network_prefix".to_string(),
            toml::Value::String("tests/network_1".to_string()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let url = make_source_url(toml::value::Table::new(), meta, Some(1), "subnet1/file.md");
        assert_eq!(
            url,
            Some(
                "https://github.com/org/repo/blob/main/tests/network_1/subnet1/file.md#L1"
                    .to_string()
            ),
            "network_prefix must be prepended to root_path in source_url"
        );
    }

    #[test]
    fn test_compute_source_url_empty_network_prefix_is_ignored() {
        // network_prefix = "" means the network IS the git root — root_path used as-is.
        let mut git = toml::value::Table::new();
        git.insert(
            "remote_url".to_string(),
            toml::Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert(
            "branch".to_string(),
            toml::Value::String("main".to_string()),
        );
        git.insert(
            "network_prefix".to_string(),
            toml::Value::String(String::new()),
        );
        let mut meta = toml::value::Table::new();
        meta.insert("git".to_string(), toml::Value::Table(git));

        let url = make_source_url(toml::value::Table::new(), meta, Some(5), "docs/guide.md");
        assert_eq!(
            url,
            Some("https://github.com/org/repo/blob/main/docs/guide.md#L5".to_string()),
            "empty network_prefix must not add a leading slash"
        );
    }

    // -------------------------------------------------------------------------
    // generate_source metadata exclusion test
    // -------------------------------------------------------------------------

    /// Confirm that metadata fields (git status, source_url) never appear in the
    /// markdown output produced by generate_source.
    ///
    /// generate_source drives exclusively from MdCodec::current_events (the raw
    /// pulldown-cmark event stream) and never reads BeliefNode::metadata.  This
    /// test verifies that invariant by parsing a document, manually injecting
    /// metadata into the first IRNode (simulating what push() does via
    /// metadata_override), and asserting that generate_source output is clean.
    #[test]
    fn test_metadata_not_in_generate_source() {
        use crate::codec::DocCodec;

        let md = "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\n# Section\n\nProse.\n";

        let mut codec = MdCodec::new();
        let root = IRNode {
            path: "net/doc.md".to_string(),
            heading: 2,
            source_line: Some(1),
            ..Default::default()
        };
        let mut diagnostics = vec![];
        codec
            .parse(
                md,
                root,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );

        // Simulate metadata_override: inject git + source_url into the first IRNode's
        // associated BeliefNode, exactly as push() does at runtime.  generate_source
        // must not include any of these fields in its output.
        // (The IRNode event stream is unaffected — we only mutate the proto document
        // to verify that generate_source never reads from it for metadata fields.)
        if let Some((proto, _events)) = codec.current_events.first_mut() {
            proto.document.insert(
                "metadata",
                toml_edit::Item::Value(toml_edit::Value::InlineTable({
                    let mut t = toml_edit::InlineTable::new();
                    t.insert(
                        "source_url",
                        toml_edit::Value::String(toml_edit::Formatted::new(
                            "https://github.com/org/repo/blob/main/net/doc.md".to_string(),
                        )),
                    );
                    t
                })),
            );
        }

        let output = codec
            .generate_source()
            .expect("generate_source must return Some");

        assert!(
            !output.contains("metadata"),
            "generate_source must not contain 'metadata'; got:\n{output}"
        );
        assert!(
            !output.contains("source_url"),
            "generate_source must not contain 'source_url'; got:\n{output}"
        );
        assert!(
            !output.contains("git"),
            "generate_source must not contain 'git'; got:\n{output}"
        );
        // Sanity: the real content must survive the round-trip.
        assert!(
            output.contains("Doc"),
            "title must be preserved; got:\n{output}"
        );
        assert!(
            output.contains("Section"),
            "heading must be preserved; got:\n{output}"
        );
        assert!(
            output.contains("Prose."),
            "body prose must be preserved; got:\n{output}"
        );
    }

    // =========================================================================
    // Inline anchor node tests (Issue 91)
    // =========================================================================

    /// Parse helper that returns (nodes, diagnostics) for inline anchor tests.
    fn parse_inline(content: &str) -> (Vec<IRNode>, Vec<ParseDiagnostic>) {
        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: "test.md".to_string(),
            heading: 2,
            ..Default::default()
        };
        codec
            .parse(
                content,
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        (codec.nodes(), diagnostics)
    }

    #[test]
    fn test_pulldown_cmark_emits_anchor_as_text_in_paragraph() {
        // Step 1 verification: {#id} in a paragraph body is NOT consumed by
        // ENABLE_HEADING_ATTRIBUTES — it survives as a Text event.
        let markdown = "Some text {#my-anchor} more text";
        let parser = MdParser::new_ext(markdown, buildonomy_md_options());
        let events: Vec<MdEvent> = parser.collect();
        let has_anchor_text = events.iter().any(|e| {
            if let MdEvent::Text(s) = e {
                s.contains("{#my-anchor}")
            } else {
                false
            }
        });
        assert!(
            has_anchor_text,
            "{{#id}} in paragraph body must survive as Text event; got: {events:?}"
        );
    }

    #[test]
    fn test_pulldown_cmark_emits_anchor_as_text_in_list_item() {
        // Tight list item: no Paragraph wrapper.
        let markdown = "- {#item-a} First\n- {#item-b} Second\n";
        let parser = MdParser::new_ext(markdown, buildonomy_md_options());
        let events: Vec<MdEvent> = parser.collect();
        let anchor_texts: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let MdEvent::Text(s) = e {
                    if s.contains("{#") {
                        Some(s.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            anchor_texts.len(),
            2,
            "Both anchors must survive as Text; got: {events:?}"
        );
    }

    #[test]
    fn test_inline_anchor_paragraph_creates_child_node() {
        let md = "## Section\n\n{#req-001} The system shall do X.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        // Expect: doc root (heading=2) + section (heading=4) + anchor node (heading=5)
        assert_eq!(
            nodes.len(),
            3,
            "doc + section + anchor = 3 nodes; got: {nodes:?}"
        );
        let anchor = &nodes[2];
        assert_eq!(anchor.heading, 5, "anchor depth = section(4) + 1");
        assert_eq!(anchor.id().as_deref(), Some("req-001"));
        assert_eq!(anchor.title().as_deref(), Some("req-001"));
    }

    #[test]
    fn test_inline_anchor_two_consecutive_are_siblings() {
        let md = "## Section\n\n{#req-001} First.\n\n{#req-002} Second.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        // doc + section + 2 anchors
        assert_eq!(nodes.len(), 4, "expected 4 nodes; got: {nodes:?}");
        let a1 = &nodes[2];
        let a2 = &nodes[3];
        assert_eq!(
            a1.heading, a2.heading,
            "consecutive anchors must be siblings (same depth)"
        );
        assert_eq!(a1.id().as_deref(), Some("req-001"));
        assert_eq!(a2.id().as_deref(), Some("req-002"));
    }

    #[test]
    fn test_inline_anchor_tight_list_item() {
        let md = "## Section\n\n- {#item-a} First\n- {#item-b} Second\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        // doc + section + 2 item anchors
        assert_eq!(nodes.len(), 4, "expected 4 nodes; got: {nodes:?}");
        let a1 = &nodes[2];
        let a2 = &nodes[3];
        assert_eq!(a1.heading, 5);
        assert_eq!(a1.id().as_deref(), Some("item-a"));
        assert_eq!(a2.heading, 5);
        assert_eq!(a2.id().as_deref(), Some("item-b"));
    }

    #[test]
    fn test_inline_anchor_after_inline_formatting() {
        // {#id} after emphasis — block-end scan should still find it.
        let md = "## Section\n\n*important* {#req-003} Details.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        assert_eq!(nodes.len(), 3, "expected 3 nodes; got: {nodes:?}");
        let anchor = &nodes[2];
        assert_eq!(anchor.id().as_deref(), Some("req-003"));
    }

    #[test]
    fn test_inline_anchor_plain_paragraph_folds_into_anchor() {
        let md = "## Section\n\n{#req-001} Requirement.\n\nPlain continuation.\n";
        let (nodes, _) = parse_inline(md);
        // Only 3 nodes: doc + section + anchor. The plain paragraph folds into anchor.
        assert_eq!(
            nodes.len(),
            3,
            "plain para should fold into anchor node; got: {nodes:?}"
        );
    }

    #[test]
    fn test_inline_anchor_followed_by_heading() {
        let md = "## Section\n\n{#req-001} Requirement.\n\n## Next Section\n\nContent.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        // doc + section + anchor + next section = 4
        assert_eq!(
            nodes.len(),
            4,
            "heading must close anchor node; got: {nodes:?}"
        );
        let anchor = &nodes[2];
        assert_eq!(anchor.id().as_deref(), Some("req-001"));
        assert_eq!(anchor.heading, 5);
        let next = &nodes[3];
        assert_eq!(next.title().as_deref(), Some("Next Section"));
        assert_eq!(next.heading, 4);
    }

    #[test]
    fn test_inline_anchor_round_trip() {
        use crate::codec::DocCodec;

        let md = "## Section\n\n{#req-001} The system shall do X.\n\nPlain continuation.\n";
        let mut codec = MdCodec::new();
        let proto = IRNode {
            path: "test.md".to_string(),
            heading: 2,
            ..Default::default()
        };
        codec
            .parse(
                md,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let output = codec.generate_source().expect("generate_source failed");
        assert!(
            output.contains("{#req-001}"),
            "anchor must survive round-trip; got:\n{output}"
        );
    }

    #[test]
    fn test_inline_anchor_relation_context_warns_at_boundary() {
        let md = "\
## Section

`{implements}`

{#req-001} Requirement.
";
        let (_, diagnostics) = parse_inline(md);
        let has_implicit_close = diagnostics.iter().any(|d| match d {
            ParseDiagnostic::Warning { message, .. } => message.contains("implicit close"),
            _ => false,
        });
        assert!(
            has_implicit_close,
            "open relation context at anchor boundary must warn; got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_inline_anchor_duplicate_id_assigns_slug_n() {
        let md = "## Section\n\n{#dup-id} First.\n\n{#dup-id} Second.\n";
        let (nodes, diagnostics) = parse_inline(md);
        // Both nodes should still be created
        assert_eq!(
            nodes.len(),
            4,
            "both anchor nodes must survive; got: {nodes:?}"
        );
        let has_collision = diagnostics.iter().any(|d| match d {
            ParseDiagnostic::Warning { message, .. } => message.contains("anchor collision"),
            _ => false,
        });
        assert!(
            has_collision,
            "duplicate inline anchor must warn; got: {diagnostics:?}"
        );
        // First keeps original, second gets slug-N
        let a1 = &nodes[2];
        let a2 = &nodes[3];
        assert_eq!(a1.id().as_deref(), Some("dup-id"));
        // slug-N appends to the full ID, not a stripped base
        assert_eq!(
            a2.id().as_deref(),
            Some("dup-id-2"),
            "duplicate must get slug-N id appended to full anchor; got: {:?}",
            a2.id()
        );
    }

    #[test]
    fn test_inline_anchor_duplicate_numeric_id_preserves_suffix() {
        // An anchor like {#swdp-63} that collides must produce swdp-63-2,
        // NOT swdp-7 (which would strip the author's numeric suffix).
        let md = "## Section\n\n{#swdp-63} First.\n\n{#swdp-63} Second.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert_eq!(nodes.len(), 4);
        let has_collision = diagnostics.iter().any(|d| match d {
            ParseDiagnostic::Warning { message, .. } => message.contains("anchor collision"),
            _ => false,
        });
        assert!(has_collision);
        let a1 = &nodes[2];
        let a2 = &nodes[3];
        assert_eq!(a1.id().as_deref(), Some("swdp-63"));
        assert_eq!(
            a2.id().as_deref(),
            Some("swdp-63-2"),
            "must append -2 to full ID, not strip numeric suffix; got: {:?}",
            a2.id()
        );
    }

    #[test]
    fn test_inline_anchor_no_split_without_anchor() {
        // Regression: paragraphs without {#id} must NOT create extra nodes.
        let md = "## Section\n\nJust a plain paragraph.\n\nAnother one.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        // doc + section = 2
        assert_eq!(
            nodes.len(),
            2,
            "no extra nodes without anchor; got: {nodes:?}"
        );
    }

    #[test]
    fn test_inline_anchor_html_strips_anchor_text() {
        use crate::codec::DocCodec;

        let md = "## Section\n\n{#req-001} The requirement text.\n";
        let mut codec = MdCodec::new();
        let proto = IRNode {
            path: "test.md".to_string(),
            heading: 2,
            ..Default::default()
        };
        codec
            .parse(
                md,
                proto,
                &mut vec![],
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let html = codec.render_html_body();
        assert!(
            html.contains("noet-inline-anchor"),
            "HTML must contain inline-anchor element; got:\n{html}"
        );
        assert!(
            html.contains("id=\"req-001\""),
            "HTML must contain id attribute; got:\n{html}"
        );
        assert!(
            !html.contains("{#req-001}"),
            "HTML must NOT contain literal {{#req-001}} text; got:\n{html}"
        );
        assert!(
            html.contains("The requirement text."),
            "HTML must contain the requirement text; got:\n{html}"
        );
    }

    #[test]
    fn test_inline_anchor_not_in_fenced_code_block() {
        // {#id} inside a fenced code block is MdEvent::Text, but the block
        // boundaries (Start/End(CodeBlock)) do NOT set/check block_start,
        // so the text must never be scanned for anchors.
        let md = "\
## Section

Some prose.

```
{#not-an-anchor} This is inside a fenced code block.
```

More prose.
";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        // doc + section = 2 (no anchor node created)
        assert_eq!(
            nodes.len(),
            2,
            "fenced code block must not create anchor node; got: {nodes:?}"
        );
        // Verify no node has the code block's ID
        assert!(
            !nodes
                .iter()
                .any(|n| n.id().as_deref() == Some("not-an-anchor")),
            "no node should have id 'not-an-anchor'"
        );
    }

    #[test]
    fn test_inline_anchor_not_in_code_span() {
        // Code spans emit MdEvent::Code, not Text — already excluded.
        let md = "## Section\n\nSee `{#not-an-anchor}` for details.\n";
        let (nodes, diagnostics) = parse_inline(md);
        assert!(
            diagnostics.is_empty(),
            "no warnings expected; got: {diagnostics:?}"
        );
        assert_eq!(
            nodes.len(),
            2,
            "code span must not create anchor node; got: {nodes:?}"
        );
    }

    #[test]
    fn test_extract_inline_anchor_basic() {
        assert_eq!(
            extract_inline_anchor("{#req-001} some text"),
            Some("req-001".to_string())
        );
        assert_eq!(
            extract_inline_anchor("prefix {#my.anchor-id} suffix"),
            Some("my.anchor-id".to_string())
        );
        assert_eq!(extract_inline_anchor("no anchor here"), None);
    }

    #[test]
    fn test_extract_inline_anchor_extended_chars() {
        // Parentheses, brackets, @ — all valid in to_anchor output
        assert_eq!(
            extract_inline_anchor("{#array.prototype.map()} text"),
            Some("array.prototype.map()".to_string())
        );
        assert_eq!(
            extract_inline_anchor("{#item[0]} text"),
            Some("item[0]".to_string())
        );
        assert_eq!(
            extract_inline_anchor("{#user@domain} text"),
            Some("user@domain".to_string())
        );
    }
}

#[cfg(test)]
mod alias_scope_tests {
    use super::*;
    use crate::codec::network::{AliasScope, AliasTemplateConfig};
    use crate::codec::proto_index::ProtoIndex;

    /// Parse `content` as a document beneath a network that declares
    /// `alias-template` with the given scope, and return the href aliases each
    /// node ended up with.
    fn aliases_under_scope(content: &str, scope: AliasScope) -> Vec<String> {
        let tmp = tempfile::tempdir().unwrap();
        let net_dir = crate::paths::canonicalize_path(tmp.path()).unwrap();
        std::fs::write(net_dir.join("index.md"), "---\nid = \"net\"\n---\n").unwrap();
        let doc_path = net_dir.join("doc.md");
        std::fs::write(&doc_path, content).unwrap();

        let proto_index = ProtoIndex::build(&net_dir, false).unwrap();
        let config = AliasTemplateConfig {
            template: "ns/{{ id }}".to_string(),
            base_url: None,
            scope,
        };
        proto_index.set_meta(
            &net_dir,
            "url_alias",
            serde_json::to_value(&config).unwrap(),
        );

        let mut codec = MdCodec::new();
        let mut diagnostics = Vec::new();
        let proto = IRNode {
            path: crate::paths::os_path_to_string(&doc_path),
            ..Default::default()
        };
        codec
            .parse(content, proto, &mut diagnostics, &proto_index)
            .unwrap();
        codec
            .current_events
            .iter()
            .flat_map(|(node, _)| node.namespace_paths.iter())
            .map(|(_ns, alias)| alias.clone())
            .collect()
    }

    const DOC: &str = "---\nid = \"doc-root\"\n---\n\n## First {#sec-one}\n\ntext\n\n## Second {#sec-two}\n\ntext\n";

    #[test]
    fn submap_scope_aliases_every_node() {
        let aliases = aliases_under_scope(DOC, AliasScope::Submap);
        assert!(
            aliases.contains(&"ns/doc-root".to_string()),
            "root should be aliased under submap, got {aliases:?}"
        );
        assert!(
            aliases.contains(&"ns/sec-one".to_string())
                && aliases.contains(&"ns/sec-two".to_string()),
            "headings should be aliased under submap, got {aliases:?}"
        );
    }

    #[test]
    fn explicit_scope_aliases_nothing_by_default() {
        let aliases = aliases_under_scope(DOC, AliasScope::Explicit);
        assert!(
            aliases.is_empty(),
            "explicit scope must alias nothing without opt-in, got {aliases:?}"
        );
    }

    #[test]
    fn explicit_scope_honours_document_opt_in() {
        let doc = "---\nid = \"doc-root\"\nalias = true\n---\n\n## First {#sec-one}\n\ntext\n";
        let aliases = aliases_under_scope(doc, AliasScope::Explicit);
        assert_eq!(
            aliases,
            vec!["ns/doc-root".to_string()],
            "only the opted-in root should be aliased"
        );
    }

    /// A heading opts in via the document root's `[sections]` table. That table is
    /// merged into heading nodes by `inject_context`, which runs after aliases are
    /// consumed — so the alias loop reads it directly. This pins that path.
    #[test]
    fn explicit_scope_honours_section_opt_in() {
        // `[sections]` keys are NodeKeys — `id://anchor` or a bare anchor — not the
        // `#anchor` form used in the heading itself.
        let doc = "---\nid = \"doc-root\"\n\n[sections.\"id://sec-two\"]\nalias = true\n---\n\n## First {#sec-one}\n\ntext\n\n## Second {#sec-two}\n\ntext\n";
        let aliases = aliases_under_scope(doc, AliasScope::Explicit);
        assert_eq!(
            aliases,
            vec!["ns/sec-two".to_string()],
            "only the opted-in heading should be aliased"
        );
    }

    /// Opt-out must override the network default, not just the absence of one.
    #[test]
    fn submap_scope_honours_document_opt_out() {
        let doc = "---\nid = \"doc-root\"\nalias = false\n---\n\n## First {#sec-one}\n\ntext\n";
        let aliases = aliases_under_scope(doc, AliasScope::Submap);
        assert_eq!(
            aliases,
            vec!["ns/sec-one".to_string()],
            "root opted out; heading still aliased by the submap default"
        );
    }

    #[test]
    fn scope_parses_from_frontmatter_and_rejects_unknown() {
        assert_eq!(
            AliasScope::from_frontmatter("submap"),
            Some(AliasScope::Submap)
        );
        assert_eq!(
            AliasScope::from_frontmatter(" Explicit "),
            Some(AliasScope::Explicit)
        );
        assert_eq!(AliasScope::from_frontmatter("root"), None);
    }

    #[test]
    fn default_scope_is_submap_for_backwards_compatibility() {
        assert_eq!(AliasScope::default(), AliasScope::Submap);
        // A config serialized before `scope` existed must still deserialize.
        let legacy = serde_json::json!({"template": "x/{{ id }}", "base_url": null});
        let parsed: AliasTemplateConfig = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.scope, AliasScope::Submap);
    }
}
