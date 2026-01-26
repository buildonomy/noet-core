use pulldown_cmark::{
    BrokenLink, CowStr, Event as MdEvent, HeadingLevel, LinkType, MetadataBlockKind, Options,
    Parser as MdParser, Tag as MdTag, TagEnd as MdTagEnd,
};
use pulldown_cmark_to_cmark::{
    cmark_resume_with_source_range_and_options, Options as CmarkToCmarkOptions,
};
use std::{
    borrow::Borrow, collections::VecDeque, mem::replace, ops::Range, result::Result, str::FromStr,
};
/// Utilities for parsing various document types into BeliefBases
use toml_edit::value;

use crate::{
    beliefbase::BeliefContext,
    codec::{
        belief_ir::{detect_schema_from_path, ProtoBeliefNode},
        DocCodec,
    },
    error::BuildonomyError,
    nodekey::{href_to_nodekey, NodeKey},
    properties::{BeliefNode, Weight, WeightKind},
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
    // md_options.insert(MdOptions::ENABLE_HEADING_ATTRIBUTES);
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
            }),
            _ => None,
        }
    }

    // Returns whether event is a [MdTagEnd::Link]
    fn push(&mut self, event: &MdEvent<'_>, range: &Option<Range<usize>>) -> bool {
        if let MdEvent::End(MdTagEnd::Link) = event {
            return true;
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
        if let MdEvent::Start(MdTag::Link { .. }) = &event {
            debug_assert!(collector.is_none());
            collector = LinkAccumulator::new(&event, &range);
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

            let title = CowStr::from(link_data.title_string());
            if let Some((key, mut has_manual_title)) = link_to_relation(
                &link_data.link_type,
                &link_data.dest_url,
                &title,
                &link_data.id,
            ) {
                // Regularize the key using the BeliefBase context, falling back to the original if it fails
                let regularized = key
                    .regularize(ctx.belief_set(), ctx.node.bid)
                    .unwrap_or(key.clone());
                let keys = vec![regularized];
                // let mut has_pothole = false;
                let sources = ctx.sources();
                let maybe_keyed_relation = keys.iter().find_map(|link_key| {
                    sources.iter().find(|rel| {
                        rel.other
                            .keys(Some(ctx.home_net), None, ctx.belief_set())
                            .iter()
                            .any(|ctx_source_key| ctx_source_key == link_key)
                    })
                });
                if let Some(relation) = maybe_keyed_relation {
                    let bref_string = CowStr::from(String::from(relation.other.bid.namespace()));
                    let doc_ref = relation.as_link_ref();
                    let is_title_really_manual = format!("{bref_string}:{title}");
                    if is_title_really_manual == doc_ref {
                        has_manual_title = false;
                    }
                    let default_ref = CowStr::from(doc_ref);
                    if !has_manual_title {
                        link_data.title_events = vec![MdEvent::Text(default_ref.clone())];
                        if link_data.dest_url != default_ref {
                            changed = true;
                            link_data.dest_url = default_ref;
                        }
                        // link_data.link_type = LinkType::WikiLink { has_pothole };
                    } else if has_manual_title && link_data.dest_url != bref_string {
                        // has_pothole = true;
                        changed = true;
                        link_data.dest_url = bref_string;
                    } else {
                        tracing::debug!("Link unchanged. Moving on");
                    }
                    // tracing::debug!(
                    //     "Normalized refs:\n\
                    //      \tmanual_title: {}\n\
                    //      \tdest_url: {}\n\
                    //      \ttitle: {}",
                    //     has_manual_title,
                    //     link_data.dest_url,
                    //     title,
                    // );
                } else {
                    tracing::info!(
                        "Returned context does not have any source edges matching potential link(s)\n\
                         \tsource_links: {:?}.\n\
                         \tctx sink links: {:?}",
                        keys,
                        ctx.sources().iter().flat_map(|extended_ref| extended_ref.other.keys(Some(ctx.home_net), None, ctx.belief_set())).collect::<Vec<NodeKey>>()
                    );
                }
            } else {
                link_data.link_type = match title.is_empty() || title == link_data.id {
                    true => LinkType::Shortcut,
                    false => LinkType::Reference,
                };
            }
            events_out.push_back((
                MdEvent::Start(MdTag::Link {
                    link_type: link_data.link_type,
                    dest_url: link_data.dest_url,
                    title: title.clone(),
                    id: link_data.id,
                }),
                None,
            ));
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
            events_out.push_back((MdEvent::End(MdTagEnd::Link), new_range));
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
            let end = match event {
                MdEvent::Text(_) => {
                    if let Some(ref title_range) = range {
                        toml_string_range = Some(title_range.end..title_range.end)
                    }
                    false
                }
                MdEvent::Start(MdTag::Heading { .. }) => false,
                MdEvent::End(MdTagEnd::Heading(_)) => true,
                _ => {
                    tracing::warn!(
                        "Not expecting any other types of events than text or Heading tags. \
                        received: {:?}",
                        event
                    );
                    false
                }
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
            let end = match event {
                MdEvent::Text(ref cow_str) => {
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
                _ => {
                    tracing::warn!(
                        "Not expecting any other types of events than text or Metadatablock end. \
                         received: {:?}",
                        event
                    );
                    false
                }
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

#[derive(Debug, Default, Clone)]
pub struct MdCodec {
    current_events: Vec<ProtoNodeWithEvents>,
    content: String,
}

impl MdCodec {
    pub fn new() -> Self {
        MdCodec {
            ..Default::default()
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
        if frontmatter_changed.is_some() {
            let metadata_string = if current_events
                .iter()
                .any(|e| matches!(e.0, MdEvent::Start(MdTag::Heading { .. })))
            {
                proto_events.0.as_subsection()
            } else {
                proto_events.0.as_frontmatter()
            };
            update_or_insert_frontmatter(&mut current_events, &metadata_string)?;
        }

        let link_changed =
            check_for_link_and_push(&mut current_events, ctx, &mut proto_events.1, None);
        let maybe_text = if frontmatter_changed.is_some() || link_changed {
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
            let mut new_node = frontmatter_changed.unwrap_or(ctx.node.clone());
            new_node
                .payload
                .insert("text".to_string(), toml::Value::String(text));
            Ok(Some(new_node))
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

    fn parse(
        &mut self,
        content: String,
        mut current: ProtoBeliefNode,
    ) -> Result<(), BuildonomyError> {
        // Initial parse and format to try and make pulldown_cmark <-> pulldown_cmark_to_cmark idempotent
        self.content = content;
        self.current_events = Vec::default();
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
                MdEvent::Text(cow_str) => {
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
                    id: _,
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
                    let new_current = ProtoBeliefNode {
                        path: current.path.clone(),
                        heading,
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
                    current.document.insert(
                        "title",
                        value(current.accumulator.take().unwrap_or_default()),
                    );
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
    /// Returns the anchor ID without the '#' prefix.
    fn extract_anchor_from_node(node: &ProtoBeliefNode) -> Option<String> {
        // TODO: Parse anchor from heading syntax once Issue 3 is implemented
        // For now, check if there's an "id" or "anchor" field in the document
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
}
