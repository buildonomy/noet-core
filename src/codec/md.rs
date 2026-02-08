use pulldown_cmark::{
    BrokenLink, CowStr, Event as MdEvent, HeadingLevel, LinkType, MetadataBlockKind, Options,
    Parser as MdParser, Tag as MdTag, TagEnd as MdTagEnd,
};
use pulldown_cmark_to_cmark::{
    cmark_resume_with_source_range_and_options, Options as CmarkToCmarkOptions,
};
use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet, VecDeque},
    mem::replace,
    ops::Range,
    result::Result,
    str::FromStr,
};
/// Utilities for parsing various document types into BeliefBases
use toml_edit::value;

use crate::{
    beliefbase::BeliefContext,
    codec::{
        belief_ir::{build_title_attribute, detect_schema_from_path, ProtoBeliefNode},
        DocCodec, CODECS,
    },
    error::BuildonomyError,
    nodekey::{get_doc_path, href_to_nodekey, to_anchor, NodeKey},
    paths::{path_extension, path_join, path_normalize, path_parent},
    properties::{asset_namespace, BeliefNode, Bid, Bref, Weight, WeightKind},
};

pub use pulldown_cmark;

/// A markdown event with optional source range information
type MdEventWithRange = (MdEvent<'static>, Option<Range<usize>>);

/// A queue of markdown events with range information
type MdEventQueue = VecDeque<MdEventWithRange>;

/// A proto node paired with its markdown event queue
type ProtoNodeWithEvents = (ProtoBeliefNode, MdEventQueue);

pub fn buildonomy_md_options() -> Options {
    let mut md_options = Options::empty();
    // This is almost all the extensions, but instead of using config.all() we enable explicitly for
    // better reproduceability.
    md_options.insert(Options::ENABLE_DEFINITION_LIST);
    md_options.insert(Options::ENABLE_FOOTNOTES);
    md_options.insert(Options::ENABLE_GFM);
    md_options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    md_options.insert(Options::ENABLE_MATH);
    // md_options.insert(MdOptions::ENABLE_OLD_FOOTNOTES);
    // md_options.insert(Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS);
    // md_options.insert(Options::ENABLE_SMART_PUNCTUATION);
    md_options.insert(Options::ENABLE_STRIKETHROUGH);
    md_options.insert(Options::ENABLE_SUBSCRIPT);
    md_options.insert(Options::ENABLE_SUPERSCRIPT);
    md_options.insert(Options::ENABLE_TABLES);
    md_options.insert(Options::ENABLE_TASKLISTS);
    md_options.insert(Options::ENABLE_WIKILINKS);
    md_options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    md_options
}

/// Maps pulldown-cmark links to [href_to_nodekey].
///
/// If the link doesn't resolve in-page, returns a [NodeKey], and whether the link attributes
/// contain a link-specific title, otherwise None.
fn link_to_relation(
    link_type: &LinkType,
    dest_url: &CowStr<'_>,
    title: &CowStr<'_>,
    id: &CowStr<'_>,
) -> Option<(NodeKey, bool)> {
    match link_type {
        // Autolink like `<http://foo.bar/baz>`
        // change to reference [foo.bar/bax][bid]
        // with NodeKey::Path(api, http://foo.bar/baz)
        LinkType::Autolink => Some((href_to_nodekey(dest_url), !title.is_empty())),

        // Email address in autolink like `<john@example.org>`
        // change to reference [john@example.org][bid]
        // with NodeKey::Path(api, john@example.org)
        LinkType::Email => Some((href_to_nodekey(&format!("email:{dest_url}")), false)),

        // Inline link like `[foo](bar)`
        // change to [foo][bid]
        // with NodeKey::Path(api, bar)
        LinkType::Inline => Some((href_to_nodekey(dest_url), !title.is_empty())),

        // Reference link like `[foo][bar]`
        // if foo matches [net:title] for the reference,
        // change to [net:title], otherwise
        // change to [foo][bid]
        // with NodeKey::?(bar)
        // Reference without destination in the document, but resolved by the broken_link_callback
        LinkType::Reference => None,
        LinkType::WikiLink { has_pothole } => Some((href_to_nodekey(title), *has_pothole)),
        LinkType::ReferenceUnknown => Some((href_to_nodekey(dest_url), !title.is_empty())),

        // Collapsed link like `[foo][]`
        // change to [[net:]title]
        // with NodeKey::?(foo)
        // Collapsed link without destination in the document, but resolved by the broken_link_callback
        LinkType::Collapsed => None,
        LinkType::CollapsedUnknown => Some((href_to_nodekey(title), false)),
        // Shortcut link like `[foo]`
        // change to [net:title]
        // with NodeKey::?(foo)
        LinkType::Shortcut => None,
        // Shortcut without destination in the document, but resolved by the broken_link_callback
        LinkType::ShortcutUnknown => Some((href_to_nodekey(id), false)),
    }
}

#[derive(Debug, Clone)]
struct LinkAccumulator {
    link_type: LinkType,
    dest_url: CowStr<'static>,
    id: CowStr<'static>,
    range: Option<Range<usize>>,
    title_events: Vec<MdEvent<'static>>,
    is_image: bool,
}

impl LinkAccumulator {
    fn new(event: &MdEvent<'_>, range: &Option<Range<usize>>) -> Option<LinkAccumulator> {
        match event {
            MdEvent::Start(MdTag::Link {
                link_type,
                dest_url,
                id,
                ..
            }) => Some(LinkAccumulator {
                link_type: *link_type,
                dest_url: dest_url.clone().into_static(),
                id: id.clone().into_static(),
                range: range.clone(),
                title_events: vec![],
                is_image: false,
            }),
            MdEvent::Start(MdTag::Image {
                link_type,
                dest_url,
                id,
                ..
            }) => Some(LinkAccumulator {
                link_type: *link_type,
                dest_url: dest_url.clone().into_static(),
                id: id.clone().into_static(),
                range: range.clone(),
                title_events: vec![],
                is_image: true,
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
    /// Any additional user-provided words in the title attribute
    user_words: Option<String>,
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
                bref = Some(parsed_bid.namespace());
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
        user_words,
    }
}

/// Build a title attribute string from components.
///
/// Format: `"bref://abc123 {\"auto_title\":true} User Words"`
///
/// # Examples
///
/// ```text
/// let attr = build_title_attribute("bref://abc123", false, None);
/// assert_eq!(attr, "bref://abc123");
///
/// let attr = build_title_attribute("bref://abc123", true, Some("My Note"));
/// assert_eq!(attr, "bref://abc123 {\"auto_title\":true} My Note");
/// ```
///
/// Note: This function is tested via unit tests in the `tests` module.

/// Calculate relative path from source document to target document.
///
/// # Arguments
///
/// * `from_path` - Path to source document (e.g., "docs/guide.md")
/// * `to_path` - Path to target document (e.g., "docs/reference/api.md")
///
/// # Returns
///
/// Relative path from source to target with forward slashes (e.g., "reference/api.md").
/// Path separators are always normalized to forward slashes for cross-platform
/// Markdown/URL compatibility, regardless of the host OS.
///
/// # Examples
///
/// ```text
/// let rel = make_relative_path("docs/guide.md", "docs/reference/api.md");
/// assert_eq!(rel, "reference/api.md");
///
/// let rel = make_relative_path("docs/reference/types.md", "docs/guide.md");
/// assert_eq!(rel, "../guide.md");
/// ```
///
/// Note: This function is tested via unit tests in the `tests` module.
pub(crate) fn make_relative_path(from_path: &str, to_path: &str) -> String {
    // URL-safe path manipulation (no PathBuf to avoid Windows path separator issues)

    // Get the directory containing the source document
    let from_dir = path_parent(from_path);

    // Split paths into components
    let from_parts: Vec<&str> = if from_dir.is_empty() {
        vec![]
    } else {
        from_dir.split('/').collect()
    };
    let to_parts: Vec<&str> = to_path.split('/').collect();

    // Find common prefix length
    let mut common_len = 0;
    for (i, (from_part, to_part)) in from_parts.iter().zip(to_parts.iter()).enumerate() {
        if from_part == to_part {
            common_len = i + 1;
        } else {
            break;
        }
    }

    // Build relative path
    let mut result = Vec::new();

    // Add ../ for each remaining directory in from_path
    for _ in common_len..from_parts.len() {
        result.push("..");
    }

    // Add remaining parts of to_path
    for part in &to_parts[common_len..] {
        result.push(*part);
    }

    if result.is_empty() {
        to_path.to_string()
    } else {
        result.join("/")
    }
}

#[tracing::instrument(skip_all)]
fn check_for_link_and_push(
    events_in: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
    ctx: &BeliefContext<'_>,
    events_out: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
    stop_event: Option<&MdEvent<'_>>,
) -> bool {
    let mut changed = false;
    let mut collector: Option<LinkAccumulator> = None;
    let mut maybe_event = events_in.pop_front();
    while let Some((event, range)) = maybe_event.take() {
        let stop_event_match = stop_event.filter(|e| **e == event).is_some();
        let mut process_link = false;
        let mut original_title_attr: Option<CowStr<'static>> = None;

        if let MdEvent::Start(MdTag::Link { title, .. }) = &event {
            debug_assert!(collector.is_none());
            collector = LinkAccumulator::new(&event, &range);
            // Store the original title attribute for parsing
            original_title_attr = Some(title.clone().into_static());
        } else if let MdEvent::Start(MdTag::Image { title, .. }) = &event {
            debug_assert!(collector.is_none());
            collector = LinkAccumulator::new(&event, &range);
            // Store the original title attribute for parsing (though images use alt text)
            original_title_attr = Some(title.clone().into_static());
        } else if let Some(link_accumulator) = collector.as_mut() {
            process_link = link_accumulator.push(&event, &range);
        }

        // Don't push events if we're collecting a link
        if collector.is_none() {
            events_out.push_back((event, range));
        } else if process_link {
            let mut link_data = collector
                .take()
                .expect("Process_link is only true if collector is some.");

            let link_text = link_data.title_string();

            // Parse the title attribute to check for existing Bref
            let title_parts = original_title_attr
                .as_ref()
                .map(|t| parse_title_attribute(t.as_ref()))
                .unwrap_or(TitleAttributeParts {
                    bref: None,
                    auto_title: false,
                    user_words: None,
                });

            let normalized_dest_url = if link_data.dest_url.starts_with("/") {
                tracing::warn!(
                    "[check_for_link_and_push] noet-core cannot determine what an absolute link \
                    is in relation to, treating link as absolute relative to our parsing context. \
                    If this document is in a subnet, this may have surprising effects."
                );
                CowStr::from(path_normalize(link_data.dest_url.as_ref()))
            } else {
                // Get the directory containing the current document
                let current_dir = if path_extension(&ctx.relative_path).is_none() {
                    ctx.relative_path.as_ref()
                } else {
                    path_parent(&ctx.relative_path)
                };
                let resolved = path_join(current_dir, link_data.dest_url.as_ref(), false);

                // Normalize the path (resolve .. and .) using URL-safe path_normalize
                let normalized = path_normalize(&resolved);

                // Check if normalized path backtracks above root (starts with ../)
                if normalized.starts_with("../") || normalized == ".." {
                    tracing::warn!(
                        "[check_for_link_and_push] SECURITY: Link '{}' backtracks beyond repository root from '{}' - will be broken in HTML output",
                        link_data.dest_url,
                        ctx.relative_path
                    );
                }
                CowStr::from(normalized)
            };
            // Determine the key to use for matching
            // If title attribute contains a Bref, prioritize it
            let key = if let Some(bref) = &title_parts.bref {
                NodeKey::Bref { bref: bref.clone() }
            } else {
                // Otherwise parse from normalized dest_url
                let title = CowStr::from(link_text.clone());
                if let Some((parsed_key, _)) = link_to_relation(
                    &link_data.link_type,
                    &normalized_dest_url,
                    &title,
                    &link_data.id,
                ) {
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
                            dest_url: link_data.dest_url,
                            title: original_title_attr.unwrap_or(CowStr::from("")),
                            id: link_data.id,
                        })
                    } else {
                        MdEvent::Start(MdTag::Link {
                            link_type: link_data.link_type,
                            dest_url: link_data.dest_url,
                            title: original_title_attr.unwrap_or(CowStr::from("")),
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

            // Regularize the key using the BeliefBase context
            let regularized = key
                .regularize(ctx.belief_set(), ctx.node.bid)
                .unwrap_or(key.clone());

            let keys = vec![regularized];

            // DEBUG: Asset link tracing
            let is_asset = if let NodeKey::Path { net, path } = &key {
                if *net == asset_namespace() {
                    tracing::info!(
                        "[check_for_link_and_push] Asset link detected: path={}, is_image={}",
                        path,
                        link_data.is_image
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if is_asset {
                if let NodeKey::Path { net: _, path } = &keys[0] {
                    tracing::info!(
                        "[check_for_link_and_push] Asset key regularized to: path={}",
                        path
                    );
                }
            }

            // Check both sources (upstream) and sinks (downstream) for the link target
            // Assets are typically sink-owned (downstream), while document links are sources (upstream)
            let sources = ctx.sources();
            let sinks = ctx.sinks();

            let maybe_keyed_relation = keys.iter().find_map(|link_key| {
                // First check sources (upstream relations - documents linking TO this node)
                sources
                    .iter()
                    .find(|rel| {
                        rel.other
                            .keys(Some(ctx.home_net), None, ctx.belief_set())
                            .iter()
                            .any(|ctx_source_key| ctx_source_key == link_key)
                    })
                    .or_else(|| {
                        // Then check sinks (downstream relations - things this document links TO, like assets)
                        sinks.iter().find(|rel| {
                            rel.other
                                .keys(Some(ctx.home_net), None, ctx.belief_set())
                                .iter()
                                .any(|ctx_sink_key| ctx_sink_key == link_key)
                        })
                    })
            });

            if is_asset {
                if maybe_keyed_relation.is_some() {
                    tracing::info!("[check_for_link_and_push] Asset link FOUND");
                } else {
                    tracing::info!("[check_for_link_and_push] Asset link NOT FOUND");
                    tracing::info!(
                        "[check_for_link_and_push] Available sources: {}, sinks: {}",
                        sources.len(),
                        sinks.len()
                    );
                    tracing::info!("[check_for_link_and_push] Looking for keys: {:?}", keys);
                    if !sinks.is_empty() {
                        tracing::info!(
                            "[check_for_link_and_push] Sink keys: {:?}",
                            sinks[0]
                                .other
                                .keys(Some(ctx.home_net), None, ctx.belief_set())
                        );
                    }
                }
            }

            if let Some(relation) = maybe_keyed_relation {
                // Generate canonical format: [text](relative/path.md#anchor "bref://abc config")

                tracing::debug!(
                    "Found relation for link: title={}, id={:?}, home_path={}",
                    relation.other.title,
                    relation.other.id,
                    relation.relative_path
                );

                // 1. Calculate relative path from source to target
                // Strip any existing anchor from home_path to avoid double anchors
                let relative_path_without_anchor = get_doc_path(&relation.relative_path);
                let ctx_home_doc_path = get_doc_path(&ctx.relative_path);

                let relative_path =
                    make_relative_path(&ctx.relative_path, relative_path_without_anchor);

                // 2. Add anchor if target is a heading node
                // Extract anchor from relation.other.id or from home_path
                let maybe_anchor = relation.other.id.as_deref().or_else(|| {
                    // If id is not set, extract anchor from home_path
                    relation
                        .relative_path
                        .rfind('#')
                        .map(|idx| &relation.relative_path[idx + 1..])
                });

                // If source and target are in the same document, use fragment-only format
                let dest_with_anchor = if let Some(anchor) = maybe_anchor {
                    if relative_path_without_anchor == ctx_home_doc_path {
                        // Same document - use fragment-only format
                        format!("#{anchor}")
                    } else {
                        // Different document - use relative path with anchor
                        format!("{relative_path}#{anchor}")
                    }
                } else {
                    relative_path
                };

                // 3. Build title attribute: "bref://abc123 {config} user words"
                let bref_str = format!("bref://{}", relation.other.bid.namespace());

                // Determine if auto_title should be enabled
                // Default to false unless link text matches target title
                let should_auto_title = if title_parts.auto_title {
                    // User explicitly set auto_title
                    true
                } else if link_text == relation.other.title {
                    // Link text matches target title - enable auto update
                    true
                } else {
                    // User provided custom text - don't auto update
                    false
                };

                let new_title_attr = build_title_attribute(
                    &bref_str,
                    should_auto_title,
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
                let original_title_attr_str = original_title_attr
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default();

                if link_data.dest_url.as_ref() != dest_with_anchor
                    || original_title_attr_str != new_title_attr
                    || link_text != new_link_text
                {
                    changed = true;
                    link_data.dest_url = CowStr::from(dest_with_anchor);
                    link_data.title_events = vec![MdEvent::Text(CowStr::from(new_link_text))];

                    tracing::debug!(
                        "Transformed link to canonical format: dest={}, title_attr={}, text={}",
                        link_data.dest_url,
                        new_title_attr,
                        link_data
                            .title_events
                            .first()
                            .map(|e| format!("{e:?}"))
                            .unwrap_or_default()
                    );
                }

                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.dest_url,
                        title: CowStr::from(new_title_attr),
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.dest_url,
                        title: CowStr::from(new_title_attr),
                        id: link_data.id,
                    })
                };
                events_out.push_back((start_event, None));
            } else {
                // No matching relation found - leave link unchanged
                tracing::info!(
                    "Returned context does not have any source edges matching potential link(s)\n\
                     \tsource_links: {:?}.\n\
                     \tctx sink links: {:?}",
                    keys,
                    ctx.sources()
                        .iter()
                        .flat_map(|extended_ref| extended_ref.other.keys(
                            Some(ctx.home_net),
                            None,
                            ctx.belief_set()
                        ))
                        .collect::<Vec<NodeKey>>()
                );

                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.dest_url,
                        title: original_title_attr.unwrap_or(CowStr::from("")),
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.dest_url,
                        title: original_title_attr.unwrap_or(CowStr::from("")),
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
            MdEvent::End(MdTagEnd::Heading(_)) => {
                if header_end.is_none() {
                    header_end = Some(idx + 1)
                }
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

/// Extract anchor from heading node (e.g., {#intro} syntax).
/// Returns the anchor ID without the '#' prefix.
///
/// TODO: This is a placeholder until Issue 3 implements anchor parsing.
/// Currently checks for "anchor" or "id" fields in the document.
fn extract_anchor_from_node(node: &ProtoBeliefNode) -> Option<String> {
    node.document
        .get("anchor")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            node.document
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Find metadata match for a ProtoBeliefNode with priority: BID > Anchor > Title.
///
/// Returns a reference to the matching metadata table if found.
fn find_metadata_match<'a>(
    node: &ProtoBeliefNode,
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
    if let Some(anchor) = extract_anchor_from_node(node) {
        // Try as Id variant (anchors are IDs within a document)
        let anchor_key = NodeKey::Id {
            net: Bid::nil(),
            id: anchor.clone(),
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
                net: Bid::nil(),
                id: anchor,
            };
            if let Some(meta) = metadata.get(&id_key) {
                return Some((id_key, meta));
            }
        }
    }

    None
}

/// Merge metadata from a TomlTable into a ProtoBeliefNode's document.
/// Preserves existing fields, adds new fields from metadata.
fn merge_metadata_into_node(node: &mut ProtoBeliefNode, metadata: &toml_edit::Table) {
    for (key, value) in metadata.iter() {
        // Don't overwrite existing fields in the node
        if !node.document.contains_key(key) {
            node.document.insert(key, value.clone());
        }
    }
}

/// Determine node ID with collision detection.
/// Priority: explicit ID > title-derived ID > bref (on collision)

#[derive(Debug, Default, Clone)]
pub struct MdCodec {
    current_events: Vec<ProtoNodeWithEvents>,
    content: String,
    /// Track which section keys have been matched during inject_context phase
    matched_sections: HashSet<NodeKey>,
    /// Track heading IDs within current document for collision detection
    seen_ids: HashSet<String>,
}

impl MdCodec {
    pub fn new() -> Self {
        MdCodec {
            current_events: Vec::new(),
            content: String::new(),
            matched_sections: HashSet::new(),
            seen_ids: HashSet::new(),
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
}

impl DocCodec for MdCodec {
    /// convert proto,
    /// insert bid into source if proto.bid is none
    /// rewrite links according to builder.doc_bb relations
    fn nodes(&self) -> Vec<ProtoBeliefNode> {
        self.current_events
            .iter()
            .map(|(proto, _)| proto.clone())
            .collect()
    }

    fn inject_context(
        &mut self,
        node: &ProtoBeliefNode,
        ctx: &BeliefContext<'_>,
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

        let proto_events = self
            .current_events
            .iter_mut()
            .find(|(proto, _)| proto == node)
            .ok_or(BuildonomyError::Codec(
                "No proto node stored in codec matching node argument".to_string(),
            ))?;

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

                // Merge metadata into the heading node
                merge_metadata_into_node(&mut proto_events.0, metadata_table);
                sections_metadata_merged = true;

                tracing::debug!(
                    "Matched heading '{}' to section metadata via key: {:?}",
                    proto_events
                        .0
                        .document
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<untitled>"),
                    matched_key
                );
            }
        }

        // Network-level collision detection and ID injection
        let mut id_changed = false;
        if proto_events.0.heading > 2 {
            // This is a heading node (not document)
            // Use ctx.node.id (which has collision-corrected value from push)
            // Fall back to Bref if None (collision detected)
            let final_id = ctx
                .node
                .id
                .clone()
                .unwrap_or_else(|| ctx.node.bid.namespace().to_string());

            // Store the final ID in the proto
            if proto_events.0.id.as_deref() != Some(&final_id) {
                tracing::debug!(
                    "Setting section ID: proto.id={:?} -> final_id='{}' for title='{}'",
                    proto_events.0.id,
                    final_id,
                    proto_events
                        .0
                        .document
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<untitled>")
                );
                proto_events.0.id = Some(final_id);
                id_changed = true;
            }

            // Inject ID into heading event if it differs from original
            // Find the original ID from the heading event (check current_events, not proto_events.1)
            let original_event_id = current_events.iter().find_map(|(event, _)| {
                if let MdEvent::Start(MdTag::Heading { id, .. }) = event {
                    id.as_ref().map(|s| s.to_string())
                } else {
                    None
                }
            });

            // Determine if we need to inject
            let needs_injection = proto_events.0.id.as_ref() != original_event_id.as_ref();

            if needs_injection {
                // Mutate heading event to inject final ID and clear range
                // Clearing the range forces cmark_resume to use event data instead of source
                // IMPORTANT: Modify current_events, not proto_events.1 (which was taken via mem::take)
                for (event, range) in current_events.iter_mut() {
                    if let MdEvent::Start(MdTag::Heading { id, .. }) = event {
                        *id = proto_events.0.id.as_ref().map(|s| CowStr::from(s.clone()));
                        *range = None; // Clear range to force writing modified ID
                        break;
                    }
                }
                // Set id_changed after injection to trigger text regeneration
                id_changed = true;
            }

            // Store ID in document for BeliefNode conversion
            // We do this during inject_context rather than parse to avoid spurious update events
            if let Some(ref id) = proto_events.0.id {
                if proto_events.0.document.get("id").is_none() {
                    proto_events.0.document.insert("id", value(id.clone()));
                }
            }
        }

        // Only update frontmatter for document nodes (heading == 2), never for section nodes (heading > 2)
        // Section metadata stays in document-level "sections" table (Issue 02)
        if (frontmatter_changed.is_some() || sections_metadata_merged || id_changed)
            && proto_events.0.heading == 2
        {
            let metadata_string = proto_events.0.as_frontmatter();
            update_or_insert_frontmatter(&mut current_events, &metadata_string)?;
        }

        let link_changed =
            check_for_link_and_push(&mut current_events, ctx, &mut proto_events.1, None);
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

        if let Some(text) = maybe_text {
            proto_events.0.document.insert("text", value(text.clone()));
            // If sections metadata was merged OR frontmatter changed, create new node from proto
            // This ensures we capture both context updates AND sections metadata
            let new_node = if sections_metadata_merged || frontmatter_changed.is_some() {
                match BeliefNode::try_from(&proto_events.0) {
                    Ok(node) => node,
                    Err(e) => {
                        tracing::warn!("Failed to convert updated proto to BeliefNode: {:?}", e);
                        frontmatter_changed.unwrap_or(ctx.node.clone())
                    }
                }
            } else {
                frontmatter_changed.unwrap_or(ctx.node.clone())
            };
            let mut new_node_with_text = new_node;
            new_node_with_text
                .payload
                .insert("text".to_string(), toml::Value::String(text));
            Ok(Some(new_node_with_text))
        } else if sections_metadata_merged || frontmatter_changed.is_some() {
            // No text regeneration needed, but metadata was merged or context changed
            // Create new BeliefNode from the updated ProtoBeliefNode
            match BeliefNode::try_from(&proto_events.0) {
                Ok(new_node) => Ok(Some(new_node)),
                Err(e) => {
                    tracing::warn!(
                        "Failed to convert proto with merged metadata to BeliefNode: {:?}",
                        e
                    );
                    Ok(frontmatter_changed)
                }
            }
        } else {
            Ok(None)
        }
    }

    fn generate_source(&self) -> Option<String> {
        let events = self
            .current_events
            .iter()
            .flat_map(|(_p, events)| events.iter().cloned());
        Self::events_to_text(&self.content, events)
    }

    fn generate_html(&self) -> Result<Vec<(String, String)>, BuildonomyError> {
        // Rewrite document links to .html for HTML output and break invalid backtracking links
        let mut relative_path = self
            .current_events
            .first()
            .map(|(proto, _)| proto.path.clone())
            .unwrap_or_default();
        if path_extension(&relative_path).is_some() {
            relative_path = path_parent(&relative_path).to_string();
        }
        fn rewrite_md_links_to_html(
            relative_path: &str,
            event: MdEvent<'static>,
        ) -> MdEvent<'static> {
            match event {
                MdEvent::Start(MdTag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => {
                    // Check for invalid backtracking links (detected during context injection)
                    // Even if dest_url is an anchor path,
                    let url_str = path_normalize(&path_join(
                        relative_path,
                        &dest_url,
                        dest_url.starts_with('#'),
                    ));
                    if url_str.starts_with("../") {
                        // Break invalid backtracking link
                        return MdEvent::Start(MdTag::Link {
                            link_type,
                            dest_url: CowStr::from("#"),
                            title: CowStr::from("⚠️ Invalid link - backtracks beyond repository"),
                            id,
                        });
                    }

                    let should_rewrite = title.contains("bref://");
                    let new_url = if should_rewrite {
                        // Use anchor-aware extension checking
                        if let Some(ext) = path_extension(&url_str) {
                            let codec_extensions = CODECS.extensions();
                            if codec_extensions.iter().any(|ce| ce == ext) {
                                // Check if there's an anchor
                                if let Some(anchor_idx) = url_str.find('#') {
                                    // Replace extension before anchor: file.md#anchor -> file.html#anchor
                                    let path_part = &url_str[..anchor_idx];
                                    let anchor_part = &url_str[anchor_idx..];
                                    let new_path = path_part.replace(&format!(".{}", ext), ".html");
                                    CowStr::from(format!("{}{}", new_path, anchor_part))
                                } else {
                                    // No anchor: file.md -> file.html
                                    CowStr::from(url_str.replace(&format!(".{}", ext), ".html"))
                                }
                            } else {
                                dest_url
                            }
                        } else {
                            dest_url
                        }
                    } else {
                        dest_url
                    };

                    MdEvent::Start(MdTag::Link {
                        link_type,
                        dest_url: new_url,
                        title,
                        id,
                    })
                }
                MdEvent::Start(MdTag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => {
                    // Check for invalid backtracking images (detected during context injection)
                    let url_str = dest_url.to_string();
                    if url_str.starts_with("../") {
                        // Break invalid backtracking image with red X data URI
                        let data_uri = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16'%3E%3Cpath d='M2 2 L14 14 M14 2 L2 14' stroke='%23ff0000' stroke-width='2'/%3E%3C/svg%3E";
                        return MdEvent::Start(MdTag::Image {
                            link_type,
                            dest_url: CowStr::from(data_uri),
                            title: CowStr::from("⚠️ Invalid image - backtracks beyond repository"),
                            id,
                        });
                    }

                    // Images don't need link rewriting, pass through unchanged
                    MdEvent::Start(MdTag::Image {
                        link_type,
                        dest_url,
                        title,
                        id,
                    })
                }
                _ => event,
            }
        }

        // Get source path from ProtoBeliefNode's path field to compute output filename
        let source_path = self
            .current_events
            .first()
            .map(|(proto, _)| &proto.path)
            .ok_or_else(|| {
                BuildonomyError::Codec("Document missing for HTML generation".to_string())
            })?;

        // Extract filename and convert extension to .html
        // Extract filename and convert extension to .html
        // Handle empty path (tests) by defaulting to "document.html"
        let output_filename = if source_path.is_empty() {
            "document.html".to_string()
        } else {
            // Extract filename using URL-safe string manipulation (no PathBuf)
            let filename = source_path.rsplit('/').next().ok_or_else(|| {
                BuildonomyError::Codec("Cannot extract filename from path".to_string())
            })?;

            if let Some(stem) = filename.strip_suffix(".md") {
                format!("{}.html", stem)
            } else {
                filename.to_string()
            }
        };

        // Generate HTML body from markdown events
        let events = self
            .current_events
            .iter()
            .flat_map(|(_p, events)| events.iter().map(|(e, _)| e.clone()))
            .map(|e| rewrite_md_links_to_html(&relative_path, e));

        let mut html_body = String::new();
        pulldown_cmark::html::push_html(&mut html_body, events);

        Ok(vec![(output_filename, html_body)])
    }

    fn finalize(&mut self) -> Result<Vec<(ProtoBeliefNode, BeliefNode)>, BuildonomyError> {
        let mut modified_nodes = Vec::new();

        // Step 1: Build sections table from all section nodes (heading > 2)
        // This happens AFTER all inject_context() calls, so sections have BIDs
        let mut sections_table = toml_edit::Table::new();

        for (section_proto, _) in self.current_events.iter().skip(1) {
            // Skip document node (index 0), collect section nodes (heading > 2)
            if section_proto.heading > 2 {
                // Get or generate section ID
                // Sections without IDs are collision cases where Bref should be used
                let section_id = if let Some(id) = section_proto.id.as_ref() {
                    id.clone()
                } else {
                    // Generate Bref from BID for sections without IDs (collision cases)
                    if let Some(bid_value) = section_proto.document.get("bid") {
                        if let Some(bid_str) = bid_value.as_str() {
                            if let Ok(bid) = crate::properties::Bid::try_from(bid_str) {
                                let bref = bid.namespace().to_string();
                                tracing::debug!(
                                    "finalize() - Generated Bref '{}' for section without ID: title={:?}",
                                    bref,
                                    section_proto.document.get("title").and_then(|v| v.as_str())
                                );
                                bref
                            } else {
                                tracing::warn!(
                                    "finalize() - Section has invalid BID, skipping: title={:?}",
                                    section_proto.document.get("title").and_then(|v| v.as_str())
                                );
                                continue;
                            }
                        } else {
                            tracing::warn!(
                                "finalize() - Section BID is not a string, skipping: title={:?}",
                                section_proto.document.get("title").and_then(|v| v.as_str())
                            );
                            continue;
                        }
                    } else {
                        tracing::warn!(
                            "finalize() - Section has no BID, skipping: title={:?}",
                            section_proto.document.get("title").and_then(|v| v.as_str())
                        );
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
                // No sections in markdown, check if we need to remove existing sections
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

                // Document was modified, need to create updated BeliefNode
                match BeliefNode::try_from(&doc_proto.0) {
                    Ok(updated_node) => {
                        modified_nodes.push((doc_proto.0.clone(), updated_node));
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to convert modified document node to BeliefNode: {:?}",
                            e
                        );
                    }
                }
            }
        }

        Ok(modified_nodes)
    }

    fn parse(
        &mut self,
        content: String,
        mut current: ProtoBeliefNode,
    ) -> Result<(), BuildonomyError> {
        // Initial parse and format to try and make pulldown_cmark <-> pulldown_cmark_to_cmark idempotent
        self.content = content;
        self.current_events = Vec::default();
        self.matched_sections.clear();
        self.seen_ids.clear();
        if let Some(schema) = detect_schema_from_path(&current.path) {
            if current.document.get("schema").is_none() {
                current.document.insert("schema", value(schema));
            }
        }
        let mut proto_events = VecDeque::new();
        let mut link_collector: Option<LinkAccumulator> = None;
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
                debug_assert!(link_collector.is_none());
                link_collector = Some(link_data);
            }
            let mut push_relation = false;
            if let Some(link_data) = link_collector.as_mut() {
                push_relation = link_data.push(event.borrow(), &Some(offset.clone()));
            }
            if push_relation {
                let link_data = link_collector.take().expect(
                    "Push relation is only true if link_data is some and the link end tag is found",
                );
                if let Some((node_key, _)) = link_to_relation(
                    &link_data.link_type,
                    &link_data.dest_url,
                    &CowStr::from(link_data.title_string()),
                    &link_data.id.clone(),
                ) {
                    let title = link_data.title_string();
                    let payload = if !title.is_empty()
                        && title != link_data.dest_url.as_ref()
                        && title != link_data.id.as_ref()
                    {
                        let mut weight = Weight::default();
                        weight.set::<String>("title", title).ok();
                        Some(weight)
                    } else {
                        None
                    };
                    current
                        .upstream
                        .push((node_key, WeightKind::Epistemic, payload));
                }
            }

            // log::debug!("[codec::md]: {:?}", event);
            match event.borrow() {
                MdEvent::Start(MdTag::MetadataBlock(_)) => {
                    debug_assert!(current.accumulator.is_none());
                    current.accumulator = Some(String::new());
                }
                MdEvent::End(MdTagEnd::MetadataBlock(_)) => {
                    let toml_string = current.accumulator.take().expect(
                        "to never encounter an end tag before a start tag and always initialize \
                         accum to Some in the start tag",
                    );

                    match ProtoBeliefNode::from_str(&toml_string) {
                        Ok(mut proto) => {
                            current.merge(&mut proto);
                        }
                        Err(e) => {
                            // Fallback to simple deserialization if TomlCodec fails
                            tracing::warn!("ProtoBeliefNode toml parse failed: {:?}", e);
                            current.errors.push(e);
                        }
                    };
                }
                MdEvent::Text(cow_str) | MdEvent::InlineHtml(cow_str) | MdEvent::Code(cow_str) => {
                    if !current.document.contains_key("title") || current.content.is_empty() {
                        if let Some(accum_string_ref) = current.accumulator.as_mut() {
                            *accum_string_ref += " ";
                            *accum_string_ref += cow_str;
                        } else {
                            current.accumulator = Some(cow_str.to_string());
                        }
                    }
                }
                MdEvent::Start(MdTag::Heading {
                    level,
                    id,
                    classes: _,
                    attrs: _,
                }) => {
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
                    let normalized_id = id.as_ref().map(|id_str| to_anchor(id_str));
                    let new_current = ProtoBeliefNode {
                        path: current.path.clone(),
                        heading,
                        id: normalized_id,
                        ..Default::default()
                    };
                    // Inherit the schema type from the prior parse. If the node has an explicit
                    // schema, it will overwrite this when merging the node's toml.
                    let mut proto_to_push = replace(&mut current, new_current);
                    proto_to_push.traverse_schema()?;
                    let proto_to_push_events = std::mem::take(&mut proto_events);
                    self.current_events
                        .push((proto_to_push, proto_to_push_events));
                }
                MdEvent::End(MdTagEnd::Heading(_)) => {
                    // We should never encounter a heading end tag before a heading start tag, and
                    // we initialize title_accum to Some(String::new) in the start tag.
                    let title = current.accumulator.take().unwrap_or_default();
                    current.document.insert("title", value(&title));

                    // Collision detection: determine final ID based on explicit ID, title, and seen IDs
                    // Only for section headings (heading > 2), not document nodes
                    if current.heading > 2 {
                        let explicit_id = current.id.as_deref();

                        // Determine candidate ID (explicit or title-derived)
                        let candidate = if let Some(id) = explicit_id {
                            to_anchor(id)
                        } else {
                            to_anchor(&title)
                        };

                        // Check for collision
                        let final_id = if self.seen_ids.contains(&candidate) {
                            // Collision detected! We can't generate Bref yet (no BID at parse time)
                            // Use None to signal that inject_context() should generate the Bref
                            None
                        } else {
                            Some(candidate.clone())
                        };

                        // Track this ID to detect future collisions (only if we assigned one)
                        if let Some(ref id) = final_id {
                            self.seen_ids.insert(id.clone());
                        }

                        // Store final ID in node's id field
                        // Note: We store in document during inject_context to avoid spurious update events
                        current.id = final_id;
                    }
                }
                _ => {}
            }
            proto_events.push_back((event.into_static(), Some(offset)));
        }
        current.traverse_schema()?;
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

        Ok(())
    }
}

pub fn to_html(content: &str, output: &mut String) -> Result<(), BuildonomyError> {
    let parser = MdParser::new_ext(content, buildonomy_md_options());
    pulldown_cmark::html::write_html_fmt(output, parser)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        nodekey::{to_anchor, NodeKey},
        properties::Bid,
    };
    use std::collections::HashMap;
    use toml_edit::{DocumentMut, Table as TomlTable};

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

    /// Extract anchor from heading node (e.g., {#intro} syntax).
    /// Returns the normalized anchor ID without the '#' prefix.
    fn extract_anchor_from_node(node: &ProtoBeliefNode) -> Option<String> {
        // Return the parsed and normalized ID from heading syntax
        node.id.clone()
    }

    /// Find metadata match for a ProtoBeliefNode with priority: BID > Anchor > Title.
    fn find_metadata_match<'a>(
        node: &ProtoBeliefNode,
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
        if let Some(anchor) = extract_anchor_from_node(node) {
            // Try as Id variant (anchors are IDs within a document)
            let anchor_key = NodeKey::Id {
                net: Bid::nil(),
                id: anchor.clone(),
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
                    net: Bid::nil(),
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

        assert_eq!(metadata.len(), 2);

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

[sections.introduction]
schema = "Section"
complexity = "high"

[sections.background]
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
            net: Bid::nil(),
            id: "introduction".to_string(),
        };
        assert!(metadata.contains_key(&intro_key));
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
        // Using existing to_anchor from nodekey module
        // Note: to_anchor trims / and #, lowercases, replaces whitespace with -,
        // and removes punctuation for HTML/URL compatibility
        assert_eq!(to_anchor("Introduction"), "introduction");
        assert_eq!(to_anchor("My Section Title"), "my-section-title");
        assert_eq!(to_anchor("Section 2.1: Overview"), "section-21-overview");
        assert_eq!(to_anchor("API & Reference"), "api--reference");
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

        let node = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 4,
            id: None,
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
            net: Bid::nil(),
            id: "intro".to_string(),
        };

        let mut table = TomlTable::new();
        table.insert("complexity", value("medium"));
        metadata.insert(key, table);

        // Create a node with matching anchor
        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));
        doc.insert("anchor", value("intro"));

        let node = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 4,
            id: Some("intro".to_string()), // Normalized anchor
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
            net: Bid::nil(),
            id: "introduction".to_string(),
        };

        let mut table = TomlTable::new();
        table.insert("complexity", value("low"));
        metadata.insert(key, table);

        // Create a node with matching title (no BID, no anchor)
        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));

        let node = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 4,
            id: None,
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
            net: Bid::nil(),
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

        let node = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 4,
            id: None,
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
            net: Bid::nil(),
            id: "intro".to_string(),
        };
        let mut anchor_table = TomlTable::new();
        anchor_table.insert("source", value("anchor"));
        metadata.insert(anchor_key, anchor_table);

        // Add title match (using Id variant)
        let title_key = NodeKey::Id {
            net: Bid::nil(),
            id: "introduction".to_string(),
        };
        let mut title_table = TomlTable::new();
        title_table.insert("source", value("title"));
        metadata.insert(title_key, title_table);

        // Create node with anchor and title (no BID)
        let mut doc = DocumentMut::new();
        doc.insert("title", value("Introduction"));

        let node = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 4,
            id: Some("intro".to_string()), // Explicit anchor from {#intro} syntax
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

        let node = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 4,
            id: None,
        };

        let result = find_metadata_match(&node, &metadata);
        assert!(result.is_none());
    }

    // ========================================================================
    #[test]
    fn test_to_anchor_consistency() {
        // Verify to_anchor() behavior for collision detection
        assert_eq!(to_anchor("Details"), "details");
        assert_eq!(to_anchor("Section One"), "section-one");
        assert_eq!(to_anchor("API & Reference"), "api--reference");
        assert_eq!(to_anchor("My-Section!"), "my-section");

        // Case insensitivity
        assert_eq!(to_anchor("Details"), to_anchor("DETAILS"));
        assert_eq!(to_anchor("Section"), to_anchor("section"));
    }

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
        // Test that IDs are normalized during parse (without explicit ID syntax)
        use toml_edit::DocumentMut;

        let markdown = "## My-Section!";
        let mut codec = MdCodec::new();

        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000001"));
        doc.insert("schema", value("Document"));

        let proto = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 2,
            id: None,
        };

        codec.parse(markdown.to_string(), proto).unwrap();

        // Verify ID was normalized from title during parse
        let heading_node = codec.current_events.iter().find(|(p, _)| p.heading > 2);
        assert!(heading_node.is_some(), "Should have heading node");
        let (proto, _) = heading_node.unwrap();
        assert_eq!(
            proto.id.as_deref(),
            Some("my-section"),
            "ID should be normalized to lowercase without punctuation"
        );
    }

    #[test]
    fn test_id_collision_bref_fallback() {
        // Test that Bref is used when collision is detected during parse
        use toml_edit::DocumentMut;

        let markdown = "## Details\n\n## Details";
        let mut codec = MdCodec::new();

        let mut doc = DocumentMut::new();
        doc.insert("bid", value("10000000-0000-0000-0000-000000000001"));
        doc.insert("schema", value("Document"));

        let proto = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 2,
            id: None,
        };

        codec.parse(markdown.to_string(), proto).unwrap();

        // Verify first "Details" has title-derived ID, second has Bref
        let heading_nodes: Vec<&(ProtoBeliefNode, MdEventQueue)> = codec
            .current_events
            .iter()
            .filter(|(p, _)| p.heading > 2)
            .collect();

        assert_eq!(heading_nodes.len(), 2, "Should have 2 heading nodes");

        // First should have title-derived ID
        assert_eq!(
            heading_nodes[0].0.id.as_deref(),
            Some("details"),
            "First 'Details' should have title-derived ID"
        );

        // Second should have None (collision detected, Bref will be generated in inject_context)
        assert_eq!(
            heading_nodes[1].0.id.as_deref(),
            None,
            "Second 'Details' should have None due to collision (Bref assigned in inject_context)"
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
        let expected_bref = Bid::try_from(bid_str).unwrap().namespace();
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
        let attr = build_title_attribute("bref://abc123456789", false, None);
        assert_eq!(attr, "bref://abc123456789");
    }

    #[test]
    fn test_build_title_attribute_with_auto_title() {
        let attr = build_title_attribute("bref://abc123456789", true, None);
        assert_eq!(attr, "bref://abc123456789 {\"auto_title\":true}");
    }

    #[test]
    fn test_build_title_attribute_with_user_words() {
        let attr = build_title_attribute("bref://abc123456789", false, Some("My Note"));
        assert_eq!(attr, "bref://abc123456789 My Note");
    }

    #[test]
    fn test_build_title_attribute_full() {
        let attr = build_title_attribute("bref://abc123456789", true, Some("My Note"));
        assert_eq!(attr, "bref://abc123456789 {\"auto_title\":true} My Note");
    }

    #[test]
    fn test_make_relative_path_same_dir() {
        let rel = make_relative_path("docs/guide.md", "docs/api.md");
        assert_eq!(rel, "api.md");
    }

    #[test]
    fn test_make_relative_path_nested() {
        let rel = make_relative_path("docs/guide.md", "docs/reference/api.md");
        assert_eq!(rel, "reference/api.md");
    }

    #[test]
    fn test_make_relative_path_parent() {
        let rel = make_relative_path("docs/reference/types.md", "docs/guide.md");
        assert_eq!(rel, "../guide.md");
    }

    #[test]
    fn test_make_relative_path_root_to_nested() {
        let rel = make_relative_path("README.md", "docs/guide.md");
        assert_eq!(rel, "docs/guide.md");
    }

    #[test]
    fn test_make_relative_path_nested_to_root() {
        let rel = make_relative_path("docs/guide.md", "README.md");
        assert_eq!(rel, "../README.md");
    }

    #[test]
    fn test_parse_build_roundtrip() {
        let original = "bref://abc123456789 {\"auto_title\":true} Custom Text";
        let parts = parse_title_attribute(original);
        let rebuilt = build_title_attribute(
            &format!("bref://{}", parts.bref.unwrap()),
            parts.auto_title,
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
        use crate::codec::DocCodec;
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

        let proto = ProtoBeliefNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: "test.md".to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading: 2,
            id: None,
        };

        codec.parse(markdown.to_string(), proto).unwrap();
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

        // Check that InlineHtml heading generated proper ID
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

        let method_id = method_section.id.as_ref().expect("Should have ID");
        assert_eq!(
            method_id, "method-title",
            "InlineHtml should contribute to ID"
        );

        // Check that Code heading generated proper ID
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

        let code_id = code_section.id.as_ref().expect("Should have ID");
        assert_eq!(
            code_id, "using--code--in-title",
            "Code should contribute to ID (backticks become spaces)"
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

        let mixed_id = mixed_section.id.as_ref().expect("Should have ID");
        assert_eq!(
            mixed_id, "mixed--html--and--code--content",
            "Mixed InlineHtml and Code should contribute to ID (backticks become spaces)"
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
        let mut proto = ProtoBeliefNode::default();
        proto
            .document
            .insert("bid", value("01234567-89ab-cdef-0123-456789abcdef"));
        proto.document.insert("title", value("Test Document"));

        codec
            .parse(markdown.to_string(), proto)
            .expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, html_content) = &fragments[0];

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
        let mut proto = ProtoBeliefNode::default();
        proto
            .document
            .insert("bid", value("12345678-1234-5678-1234-567812345678"));
        proto.document.insert("title", value("Minimal Doc"));

        codec
            .parse(markdown.to_string(), proto)
            .expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, html_content) = &fragments[0];

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
        let mut proto = ProtoBeliefNode::default();
        proto
            .document
            .insert("bid", value("12345678-1234-5678-1234-567812345678"));
        proto.document.insert("title", value("Link Test"));

        codec
            .parse(markdown.to_string(), proto)
            .expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, html_content) = &fragments[0];

        // Verify .md links WITH bref:// are rewritten to .html
        assert!(html_content.contains("href=\"./other.html\""));
        assert!(html_content.contains("href=\"docs/page.html#section-1\""));

        // Verify .md links WITHOUT bref:// are NOT rewritten (we didn't parse them)
        assert!(html_content.contains("href=\"https://example.com/doc.md\""));

        // Verify already-.html links with bref:// are unchanged
        assert!(html_content.contains("href=\"./page.html\""));

        // Verify only links with bref:// were rewritten
        assert!(!html_content.contains("href=\"./other.md\""));
        assert!(!html_content.contains("href=\"docs/page.md#"));
    }

    // Note: Integration test for static asset tracking needed with full GraphBuilder flow
    // MdCodec::parse only creates ProtoBeliefNodes; relations are created by GraphBuilder
}
