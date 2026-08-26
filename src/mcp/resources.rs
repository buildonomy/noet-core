//! MCP resource handlers for the BeliefBase server.
//!
//! Resources are application-driven context injected by the MCP host (Claude Desktop,
//! Cursor, etc.) into agent sessions. Two resource surfaces are exposed:
//!
//! - **`noet://help/orientation`** — the LLM-targeted orientation document compiled
//!   into the binary via `include_str!`. Annotated `audience: ["assistant"]` and
//!   `priority: 0.9` so host clients auto-inject it as system context.
//!
//! - **`noet://help/{name}`** — serves any `docs/design/*.md` file from the
//!   `noet-core` source tree, compiled into the binary via `include_dir!`. Lets an
//!   agent fetch the authoritative design docs on demand without filesystem access.
//!
//! ## Resource URI scheme
//!
//! ```text
//! noet://help/orientation              → orientation.md (always available)
//! noet://help/beliefbase_architecture  → docs/design/beliefbase_architecture.md
//! noet://help/search_and_sharding      → docs/design/search_and_sharding.md
//! noet://help/{name}                   → docs/design/{name}.md
//! ```
//!
//! The `name` component strips the `.md` extension and uses the bare filename stem.
//!
//! ## TOML frontmatter stripping
//!
//! Design docs begin with a TOML frontmatter block delimited by `---`. This is
//! stripped before serving so agents receive clean Markdown without the metadata
//! noise. The orientation doc has no frontmatter and is served verbatim.

use include_dir::{include_dir, Dir};
use rmcp::{
    model::{
        Annotated, Annotations, RawResource, RawResourceTemplate, Resource, ResourceContents,
        ResourceTemplate, Role,
    },
    ErrorData as McpError,
};

/// All `docs/design/` Markdown files compiled into the binary at build time.
///
/// Paths within this `Dir` are relative to the `noet-core` source root, so a
/// file at `docs/design/beliefbase_architecture.md` is accessible via
/// `DESIGN_DOCS.get_file("docs/design/beliefbase_architecture.md")`.
static DESIGN_DOCS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/docs/design");

/// The LLM-targeted orientation document, compiled into the binary.
const ORIENTATION_TEXT: &str = include_str!("orientation.md");

/// URI for the orientation resource.
const ORIENTATION_URI: &str = "noet://help/orientation";

/// URI template for design doc resources.
const DESIGN_DOC_URI_TEMPLATE: &str = "noet://help/{name}";

// ── Resource listing ──────────────────────────────────────────────────────────

/// Return all resources available from this MCP server.
///
/// Called in response to a `resources/list` request. Returns:
/// 1. The static orientation resource.
/// 2. One entry per `docs/design/*.md` file compiled into the binary.
pub fn list_resources() -> Vec<Resource> {
    let mut resources = Vec::new();

    // Orientation resource — always first so clients encounter it immediately.
    resources.push(orientation_resource_entry());

    // One entry per compiled design doc.
    for file in DESIGN_DOCS.files() {
        let path = file.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let uri = format!("noet://help/{}", stem);
        let name = format!("docs/design/{}.md", stem);
        let description = format!("noet design document: {}", stem.replace('_', " "));
        resources.push(Annotated::new(
            RawResource::new(uri, name)
                .with_description(description)
                .with_mime_type("text/markdown"),
            None,
        ));
    }

    resources
}

/// Return the resource templates exposed by this server.
///
/// The `noet://help/{name}` template lets clients construct URIs for any
/// design doc by name without enumerating them all upfront.
pub fn list_resource_templates() -> Vec<ResourceTemplate> {
    vec![Annotated::new(
        RawResourceTemplate::new(DESIGN_DOC_URI_TEMPLATE, "noet design document")
            .with_description(
                "Fetch a noet design document by name (filename stem without .md extension). \
                 Example: noet://help/beliefbase_architecture",
            )
            .with_mime_type("text/markdown"),
        None,
    )]
}

// ── Resource reading ──────────────────────────────────────────────────────────

/// Read the contents of a resource by URI.
///
/// Dispatches on the URI:
/// - `noet://help/orientation` → `read_orientation` (private)
/// - `noet://help/{name}` → `read_design_doc` (private)
///
/// Returns `McpError::invalid_params` if the URI is not recognized.
pub fn read_resource(uri: &str) -> Result<ResourceContents, McpError> {
    if uri == ORIENTATION_URI {
        return read_orientation();
    }

    if let Some(name) = uri.strip_prefix("noet://help/") {
        if !name.is_empty() && name != "orientation" {
            return read_design_doc(name);
        }
        if name == "orientation" {
            return read_orientation();
        }
    }

    Err(McpError::invalid_params(
        format!("Unknown resource URI: {uri}"),
        None,
    ))
}

/// Return the orientation document as a text resource.
fn read_orientation() -> Result<ResourceContents, McpError> {
    Ok(ResourceContents::TextResourceContents {
        uri: ORIENTATION_URI.to_string(),
        mime_type: Some("text/markdown".to_string()),
        text: ORIENTATION_TEXT.to_string(),
        meta: None,
    })
}

/// Return a design document by its filename stem (e.g. `"beliefbase_architecture"`).
///
/// Strips TOML frontmatter (the `---`-delimited block at the top of the file)
/// before returning the content.
fn read_design_doc(name: &str) -> Result<ResourceContents, McpError> {
    let filename = format!("{}.md", name);
    let Some(file) = DESIGN_DOCS.get_file(&filename) else {
        return Err(McpError::invalid_params(
            format!("Design doc not found: docs/design/{filename}"),
            None,
        ));
    };

    let raw = file.contents_utf8().ok_or_else(|| {
        McpError::internal_error(
            format!("Design doc is not valid UTF-8: docs/design/{filename}"),
            None,
        )
    })?;

    let content = strip_toml_frontmatter(raw);
    let uri = format!("noet://help/{}", name);

    Ok(ResourceContents::TextResourceContents {
        uri,
        mime_type: Some("text/markdown".to_string()),
        text: content.to_string(),
        meta: None,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the `Resource` metadata entry for the orientation document.
///
/// Annotated with `audience: ["assistant"]` and `priority: 0.9` so MCP host
/// clients that respect these fields auto-inject the orientation as system context.
fn orientation_resource_entry() -> Resource {
    Annotated::new(
        RawResource::new(ORIENTATION_URI, "noet BeliefBase orientation")
            .with_description(
                "LLM-targeted orientation: BID/bref conventions, source=child/sink=parent \
                 graph direction, weight kinds, and canonical tool sequences for common \
                 agent tasks. Read this first.",
            )
            .with_mime_type("text/markdown"),
        {
            let mut ann = Annotations::default();
            ann.audience = Some(vec![Role::Assistant]);
            ann.priority = Some(0.9);
            Some(ann)
        },
    )
}

/// Strip a TOML frontmatter block from the beginning of a Markdown document.
///
/// noet design docs begin with:
/// ```text
/// ---
/// version = "0.1"
/// title = "..."
/// ---
/// ```
///
/// This function removes that block and returns the remaining content with
/// any leading blank lines trimmed. If no frontmatter block is found, the
/// original string is returned unchanged.
fn strip_toml_frontmatter(content: &str) -> &str {
    // Must start with `---` followed by a newline (opening delimiter).
    let after_open = if let Some(rest) = content.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = content.strip_prefix("---\r\n") {
        rest
    } else {
        return content;
    };

    // Find the closing `---` on its own line within the remaining content.
    // We search for `---` at the start, or `\n---` after content lines.
    let close_pos = if after_open.starts_with("---") {
        // Empty frontmatter: content starts immediately with closing delimiter.
        0
    } else if let Some(pos) = after_open.find("\n---") {
        pos + 1 // advance past the `\n` to point at the `---`
    } else {
        return content;
    };

    // The closing delimiter must be `---` followed by a newline or end-of-string.
    let from_close = &after_open[close_pos..];
    let after_close = if let Some(rest) = from_close.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = from_close.strip_prefix("---\r\n") {
        rest
    } else if from_close == "---" {
        ""
    } else {
        return content;
    };

    after_close.trim_start_matches(['\r', '\n'])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_frontmatter_removes_toml_block() {
        let input = "---\nversion = \"0.1\"\ntitle = \"Test\"\n---\n\n# Heading\n\nBody text.";
        let result = strip_toml_frontmatter(input);
        assert_eq!(result, "# Heading\n\nBody text.");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let input = "# Heading\n\nBody text.";
        let result = strip_toml_frontmatter(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_strip_frontmatter_empty_block() {
        let input = "---\n---\n\nContent here.";
        let result = strip_toml_frontmatter(input);
        assert_eq!(result, "Content here.");
    }

    #[test]
    fn test_orientation_text_nonempty() {
        assert!(!ORIENTATION_TEXT.is_empty());
        assert!(ORIENTATION_TEXT.contains("BID"));
        assert!(ORIENTATION_TEXT.contains("source"));
        assert!(ORIENTATION_TEXT.contains("sink"));
    }

    #[test]
    fn test_list_resources_includes_orientation() {
        let resources = list_resources();
        let orientation = resources.iter().find(|r| r.raw.uri == ORIENTATION_URI);
        assert!(orientation.is_some(), "orientation resource must be listed");
    }

    #[test]
    fn test_list_resource_templates() {
        let templates = list_resource_templates();
        assert_eq!(templates.len(), 1);
        assert!(templates[0].raw.uri_template.contains("{name}"));
    }

    #[test]
    fn test_read_orientation_returns_text() {
        let result =
            read_resource("noet://help/orientation").expect("orientation should be readable");
        match result {
            ResourceContents::TextResourceContents { text, .. } => {
                assert!(!text.is_empty());
            }
            _ => panic!("expected text resource"),
        }
    }

    #[test]
    fn test_read_unknown_uri_returns_error() {
        let result = read_resource("noet://help/nonexistent_doc_xyz");
        assert!(result.is_err());
    }
}
