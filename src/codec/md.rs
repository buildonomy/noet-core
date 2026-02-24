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
    fs::File,
    io::{BufRead, BufReader, Read},
    mem::replace,
    ops::Range,
    path::Path,
    result::Result,
    str::FromStr,
};
/// Utilities for parsing various document types into BeliefBases
use toml_edit::value;

use crate::{
    beliefbase::BeliefContext,
    codec::{belief_ir::ProtoBeliefNode, DocCodec, CODECS},
    error::BuildonomyError,
    nodekey::{href_to_nodekey, NodeKey},
    paths::{as_anchor, os_path_to_string, to_anchor, AnchorPath},
    properties::{href_namespace, BeliefKind, BeliefNode, Bid, Bref, Weight, WeightKind},
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
) -> Option<NodeKey> {
    match link_type {
        // Autolink like `<http://foo.bar/baz>`
        // with NodeKey::Path(href_net, http://foo.bar/baz)
        LinkType::Autolink => Some(href_to_nodekey(dest_url)),

        // Email address in autolink like `<john@example.org>`
        // with NodeKey::Path(href_net, email:john@example.org)
        LinkType::Email => Some(href_to_nodekey(&format!("email:{dest_url}"))),

        // Inline link like `[foo](bar)`
        // with NodeKey::Path(api, bar)
        LinkType::Inline => Some(href_to_nodekey(dest_url)),

        // Reference link like `[foo][bar]`
        // Reference without destination in the document, but resolved by the broken_link_callback
        LinkType::Reference => None,
        // Prioritize 'id' nodekey over 'path' for wikilinks only
        LinkType::WikiLink { .. } => Some(href_to_nodekey(dest_url)),
        LinkType::ReferenceUnknown => Some(href_to_nodekey(dest_url)),

        // Collapsed link like `[foo][]`
        // change to [[net:]title]
        // with NodeKey::?(foo)
        // Collapsed link without destination in the document, but resolved by the broken_link_callback
        LinkType::Collapsed => None,
        LinkType::CollapsedUnknown => Some(href_to_nodekey(title)),
        // Shortcut link like `[foo]`
        // change to [net:title]
        // with NodeKey::?(foo)
        LinkType::Shortcut => Some(href_to_nodekey(dest_url)),
        // Shortcut without destination in the document, but resolved by the broken_link_callback
        LinkType::ShortcutUnknown => Some(href_to_nodekey(id)),
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
                dest_url: dest_url.clone().into_static(),
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
                dest_url: dest_url.clone().into_static(),
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
/// * `auto_title` - If true, adds {"auto_title":true} metadata
/// * `user_words` - Optional user-provided text to append
///
/// # Examples
/// ```
/// use noet_core::codec::belief_ir::build_title_attribute;
/// let attr = build_title_attribute("bref://abc123", false, None);
/// assert_eq!(attr, "bref://abc123");
///
/// let attr = build_title_attribute("bref://abc123", true, Some("My Note"));
/// assert_eq!(attr, "bref://abc123 {\"auto_title\":true} My Note");
/// ```
pub fn build_title_attribute(bref: &str, auto_title: bool, user_words: Option<&str>) -> String {
    let mut parts = vec![bref.to_string()];

    if auto_title {
        parts.push("{\"auto_title\":true}".to_string());
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

fn check_for_link_and_push(
    events_in: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
    ctx: &BeliefContext<'_>,
    events_out: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
    stop_event: Option<&MdEvent<'_>>,
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
                if let Some(parsed_key) = link_to_relation(
                    &link_data.link_type,
                    &link_data.dest_url,
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
                            title: link_data.title,
                            id: link_data.id,
                        })
                    } else {
                        MdEvent::Start(MdTag::Link {
                            link_type: link_data.link_type,
                            dest_url: link_data.dest_url,
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

            // Regularize the key using the BeliefContext fields directly
            let regularized = key.regularize_unchecked(ctx.root_net, &ctx.root_path);
            let keys = vec![regularized];

            // Check both sources (upstream) and sinks (downstream) for the link target Assets and
            // document links are sources (upstream), but its possible they're upstream as well
            let sources = ctx.sources();
            let sinks = ctx.sinks();

            let maybe_keyed_relation = keys.iter().find_map(|link_key| {
                // First check sources (upstream relations - documents linking TO this node)
                sources
                    .iter()
                    .find(|rel| {
                        rel.other
                            .keys(Some(ctx.root_net), None, ctx.belief_set())
                            .iter()
                            .chain(
                                rel.other
                                    .keys(Some(rel.home_net), None, ctx.belief_set())
                                    .iter(),
                            )
                            .any(|ctx_source_key| ctx_source_key == link_key)
                    })
                    .or_else(|| {
                        // Then check sinks (downstream relations - things this document links TO, like assets)
                        sinks.iter().find(|rel| {
                            rel.other
                                .keys(Some(ctx.root_net), None, ctx.belief_set())
                                .iter()
                                .chain(
                                    rel.other
                                        .keys(Some(rel.home_net), None, ctx.belief_set())
                                        .iter(),
                                )
                                .any(|ctx_sink_key| ctx_sink_key == link_key)
                        })
                    })
            });

            if let Some(relation) = maybe_keyed_relation {
                // Generate canonical format: [text](relative/path.md#anchor "bref://abc config")

                let relative_path = if relation.home_net == href_namespace() {
                    relation.root_path.clone()
                } else {
                    // 1. Calculate relative path from source to target
                    // Strip any existing anchor from home_path to avoid double anchors
                    let ctx_ap = AnchorPath::from(&ctx.root_path);

                    let mut relative_path = ctx_ap.path_to(&relation.root_path, true);
                    let relative_ap = AnchorPath::from(&relation.root_path);

                    if let Some(id) = relation.other.id.as_deref() {
                        relative_path = relative_ap.join(as_anchor(id));
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
                if link_data.dest_url.as_ref() != relative_path
                    || link_data.title.as_ref() != new_title_attr
                    || link_text != new_link_text
                {
                    changed = true;
                    link_data.dest_url = CowStr::from(relative_path);
                    link_data.title_events = vec![MdEvent::Text(CowStr::from(new_link_text))];
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
                        .flat_map(|extended_ref| {
                            let mut keys =
                                extended_ref
                                    .other
                                    .keys(Some(ctx.home_net), None, ctx.belief_set());
                            keys.append(&mut extended_ref.other.keys(
                                Some(extended_ref.home_net),
                                None,
                                ctx.belief_set(),
                            ));
                            keys
                        })
                        .collect::<Vec<NodeKey>>()
                );

                let start_event = if link_data.is_image {
                    MdEvent::Start(MdTag::Image {
                        link_type: link_data.link_type,
                        dest_url: link_data.dest_url,
                        title: link_data.title,
                        id: link_data.id,
                    })
                } else {
                    MdEvent::Start(MdTag::Link {
                        link_type: link_data.link_type,
                        dest_url: link_data.dest_url,
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

pub fn to_html(content: &str, output: &mut String) -> Result<(), BuildonomyError> {
    let parser = MdParser::new_ext(content, buildonomy_md_options());
    pulldown_cmark::html::write_html_fmt(output, parser)?;
    Ok(())
}

fn read_frontmatter<R: Read>(reader: R) -> std::io::Result<Option<String>> {
    let mut buf_reader = BufReader::new(reader);
    let mut frontmatter = String::new();
    let mut line = String::new();

    // Read first line to check if frontmatter exists
    buf_reader.read_line(&mut line)?;
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

#[derive(Debug, Default, Clone)]
pub struct MdCodec {
    pub current_events: Vec<ProtoNodeWithEvents>,
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
    /// Parse a path into a proto node by reading the metadata frontmatter (if any)
    fn proto(
        &self,
        repo_path: &Path,
        path: &Path,
    ) -> Result<Option<ProtoBeliefNode>, BuildonomyError> {
        if !repo_path.exists() {
            return Err(BuildonomyError::Codec(format!(
                "[ProtoBeliefState::new] Root repository path does not exist: {:?}",
                repo_path
            )));
        };
        let rel_path = match path.is_relative() {
            true => path.to_path_buf(),
            false => path.canonicalize()?.strip_prefix(repo_path)?.to_path_buf(),
        };
        let file_path = repo_path.join(&rel_path);
        if rel_path
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|&ext| ext == "md")
            .is_none()
        {
            tracing::debug!(
                "MdCodec::proto called with path {rel_path:?}, which has a non-'md' \
            file extension. Returning None"
            );
            return Ok(None);
        }
        let reader = File::open(&file_path)?;
        let frontmatter = read_frontmatter(reader)?;

        let mut proto = if let Some(fm) = frontmatter {
            if !fm.is_empty() {
                ProtoBeliefNode::from_str(&fm)?
            } else {
                ProtoBeliefNode::default()
            }
        } else {
            // No frontmatter is fine for regular markdown documents
            ProtoBeliefNode::default()
        };
        proto.path = os_path_to_string(&rel_path);
        // Document heading
        proto.heading = 2;
        proto.kind.insert(BeliefKind::Document);
        Ok(Some(proto))
    }

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
            }
        }

        // Network-level collision detection and ID injection
        let mut id_changed = false;
        if proto_events.0.heading > 2 {
            // This is a heading node (not document)
            // Use ctx.node.id() (which has collision-corrected value from push)
            let final_id = ctx.node.id();
            // Store the final ID in the proto
            if proto_events.0.id().as_ref() != Some(&final_id) {
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
        fn rewrite_md_links_to_html(
            root_ap: &AnchorPath,
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
                    let full_path = root_ap.join(&dest_url);
                    let url_str = root_ap.path_to(&full_path, true);
                    tracing::debug!(
                        "doc_path (relative to base): {}\n\
                        dest_url (relative to doc_path): {}\n\
                        url (relative to base): {}\n\
                        url (relative to doc dir): {}",
                        root_ap.path,
                        dest_url,
                        full_path,
                        url_str
                    );
                    if full_path.starts_with("../") {
                        // Break invalid backtracking link
                        return MdEvent::Start(MdTag::Link {
                            link_type,
                            dest_url: CowStr::from(format!("#{full_path}")),
                            title: CowStr::from("⚠️ Invalid link - backtracks beyond repository"),
                            id,
                        });
                    }

                    let url_ap = AnchorPath::from(&url_str);
                    let should_rewrite = title.contains("bref://");
                    let new_url = if should_rewrite {
                        // Use anchor-aware extension checking
                        if url_ap.is_anchor() {
                            tracing::debug!("is anchor");
                            CowStr::from(as_anchor(url_ap.anchor()))
                        } else if CODECS.get(&url_ap).is_some() {
                            // Check if there's an anchor
                            let res = CowStr::from(url_ap.replace_extension("html"));
                            tracing::debug!("replacing {url_str} with {res}");
                            res
                        } else {
                            tracing::debug!("no extension for {url_str}");
                            dest_url
                        }
                    } else {
                        tracing::debug!("no bref element in title attribute for {dest_url}");
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

        let doc_path = self
            .current_events
            .first()
            .map(|(proto, _)| proto.path.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or("document.md".to_string());
        let doc_ap = AnchorPath::from(&doc_path);
        // Get source path from ProtoBeliefNode's path field to compute output filename

        // Extract filename and convert extension to .html
        // Extract filename and convert extension to .html
        // Handle empty path (tests) by defaulting to "document.html"
        if doc_ap.filestem().is_empty() {
            return Err(BuildonomyError::Codec(format!(
                "Markdown file has no filename! {}",
                doc_path
            )));
        }
        let output_filename = format!("{}.html", doc_ap.filestem());

        // Generate HTML body from markdown events
        let events = self
            .current_events
            .iter()
            .flat_map(|(_p, events)| events.iter().map(|(e, _)| e.clone()))
            .map(|e| rewrite_md_links_to_html(&doc_ap, e));

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
        content: &str,
        mut current: ProtoBeliefNode,
    ) -> Result<(), BuildonomyError> {
        // Initial parse and format to try and make pulldown_cmark <-> pulldown_cmark_to_cmark idempotent
        self.content = content.to_string();
        self.current_events = Vec::default();
        self.matched_sections.clear();
        self.seen_ids.clear();
        let mut proto_events = VecDeque::new();
        let mut link_stack: Vec<LinkAccumulator> = Vec::new();
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
                if let Some(node_key) = link_to_relation(
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
                    let maybe_normalized_id = id.as_ref().map(|id_str| to_anchor(id_str));
                    let mut new_current = ProtoBeliefNode {
                        path: current.path.clone(),
                        heading,
                        ..Default::default()
                    };
                    if let Some(normalized_id) = maybe_normalized_id {
                        new_current.document.insert("id", value(normalized_id));
                    }
                    // Inherit the schema type from the prior parse. If the node has an explicit
                    // schema, it will overwrite this when merging the node's toml.
                    let mut proto_to_push = replace(&mut current, new_current);
                    proto_to_push.traverse_schema()?;

                    if proto_to_push.id().is_none() {
                        if let Some(title) = proto_to_push
                            .document
                            .get("title")
                            .and_then(|title_val| title_val.as_str().map(|str| str.to_string()))
                        {
                            proto_to_push
                                .document
                                .insert("id", value(to_anchor(&title)));
                        }
                    }
                    let proto_to_push_events = std::mem::take(&mut proto_events);
                    self.current_events
                        .push((proto_to_push, proto_to_push_events));
                }
                MdEvent::End(MdTagEnd::Heading(_)) => {
                    // We should never encounter a heading end tag before a heading start tag, and
                    // we initialize title_accum to Some(String::new) in the start tag.
                    if current
                        .accumulator
                        .as_ref()
                        .filter(|title| !title.is_empty())
                        .is_some()
                    {
                        let title = current.accumulator.take().unwrap_or_default();
                        current.document.insert("title", value(&title));
                    } else {
                        // Don't count this as a new section --- glue it back onto the last proto
                        if let Some((last_proto, mut last_event_vec)) = self.current_events.pop() {
                            current = last_proto;
                            last_event_vec.append(&mut proto_events);
                            proto_events = last_event_vec;
                        }
                    }
                }
                _ => {}
            }
            proto_events.push_back((event.into_static(), Some(offset)));
        }
        current.traverse_schema()?;
        if current.id().is_none() {
            if let Some(title) = current
                .document
                .get("title")
                .and_then(|title_val| title_val.as_str().map(|str| str.to_string()))
            {
                current.document.insert("id", value(to_anchor(&title)));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::init_logging;
    use crate::{nodekey::NodeKey, paths::to_anchor, properties::Bid};
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
        };

        codec.parse(markdown, proto).unwrap();

        // Verify ID was normalized from title during parse
        let heading_node = codec.current_events.iter().find(|(p, _)| p.heading > 2);
        assert!(heading_node.is_some(), "Should have heading node");
        let (proto, _) = heading_node.unwrap();
        assert_eq!(
            proto.id().as_deref(),
            Some("my-section"),
            "ID should be normalized to lowercase without punctuation"
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
        };

        codec.parse(markdown, proto).unwrap();
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

        let method_id = method_section.id().expect("Should have ID");
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

        let code_id = code_section.id().expect("Should have ID");
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

        let mixed_id = mixed_section.id().expect("Should have ID");
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

        codec.parse(markdown, proto).expect("Parse failed");

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

        codec.parse(markdown, proto).expect("Parse failed");

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
        let mut proto = ProtoBeliefNode::default();
        proto
            .document
            .insert("bid", value("12345678-1234-5678-1234-567812345678"));
        proto.document.insert("title", value("Link Test"));

        codec.parse(markdown, proto).expect("Parse failed");

        let fragments = codec.generate_html().expect("HTML generation failed");
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (_path, html_content) = &fragments[0];

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
    // MdCodec::parse only creates ProtoBeliefNodes; relations are created by GraphBuilder
}
