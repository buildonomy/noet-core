//! MCP resource handlers for the BeliefBase server.
//!
//! Resources are application-driven context injected by the MCP host (Claude Desktop,
//! Cursor, etc.) into agent sessions. Two resource surfaces are exposed:
//!
//! - **`noet://help/orientation`** — the LLM-targeted orientation document compiled
//!   into the binary via `include_str!`. Annotated `audience: ["assistant"]` and
//!   `priority: 0.9` so host clients auto-inject it as system context.
//!
//! - **`noet://help/{name}`** — serves the user-facing `docs/*.md` references
//!   (e.g. `query_language`, `mcp`) and any `docs/design/**/*.md` design doc from
//!   the `noet-core` source tree, compiled into the binary via `include_dir!`. Lets
//!   an agent fetch the authoritative docs on demand without filesystem access.
//!   `docs/project/` (issue trackers) and `docs/essays/` are excluded.
//!
//! ## Resource URI scheme
//!
//! ```text
//! noet://help/orientation              → orientation.md (always available)
//! noet://help/query_language           → docs/query_language.md
//! noet://help/beliefbase_architecture  → docs/design/core/beliefbase_architecture.md
//! noet://help/search_and_sharding      → docs/design/core/search_and_sharding.md
//! noet://help/{name}                   → docs/{name}.md or docs/design/<subdir>/{name}.md
//! ```
//!
//! The `name` component is the bare filename stem, without the `.md` extension
//! and without the subdirectory. Design docs are grouped into topic
//! subdirectories (`core/`, `identity/`, `annotation/`, `procedures/`,
//! `codecs/`, `presentation/`), but the URI scheme is deliberately flat: stems
//! are unique across the tree, so callers need not know the grouping.
//!
//! ## TOML frontmatter stripping
//!
//! Design docs begin with a TOML frontmatter block delimited by `---`. This is
//! stripped before serving so agents receive clean Markdown without the metadata
//! noise. The orientation doc has no frontmatter and is served verbatim.

use include_dir::{include_dir, Dir, File};
use rmcp::{
    model::{
        Annotated, Annotations, RawResource, RawResourceTemplate, Resource, ResourceContents,
        ResourceTemplate, Role,
    },
    ErrorData as McpError,
};

/// The `docs/` tree compiled into the binary at build time.
///
/// Paths within this `Dir` are relative to `docs`. The served set is the
/// top-level user-facing references (`docs/*.md`) plus everything under
/// `docs/design/**`; [`design_doc_files`] applies that filter, so use it rather
/// than `DOCS.files()` (top level only) or an unfiltered recursive walk (which
/// would also serve issue trackers and essays).
static DOCS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/docs");

/// Subdirectories of `docs/` whose contents are served as help resources.
const SERVED_SUBDIRS: &[&str] = &["design"];

/// The LLM-targeted orientation document, compiled into the binary.
const ORIENTATION_TEXT: &str = include_str!("orientation.md");

/// URI for the orientation resource.
const ORIENTATION_URI: &str = "noet://help/orientation";

/// URI template for design doc resources.
const DESIGN_DOC_URI_TEMPLATE: &str = "noet://help/{name}";

/// Every served help doc: top-level `docs/*.md` plus every Markdown file under
/// the [`SERVED_SUBDIRS`], recursively.
///
/// `Dir::files()` is not recursive; design docs are grouped one level down
/// (`design/core/`, `design/identity/`, ...), so recursing is required to see them.
fn design_doc_files() -> impl Iterator<Item = &'static File<'static>> {
    fn walk(dir: &'static Dir<'static>) -> Box<dyn Iterator<Item = &'static File<'static>>> {
        Box::new(dir.files().chain(dir.dirs().flat_map(walk)))
    }
    let top_level = DOCS.files();
    let served_subdirs = DOCS
        .dirs()
        .filter(|d| {
            d.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| SERVED_SUBDIRS.contains(&n))
        })
        .flat_map(walk);
    top_level
        .chain(served_subdirs)
        .filter(|f| f.path().extension().is_some_and(|e| e == "md"))
}

/// Resolve a design doc by its bare filename stem, searching all subdirectories.
///
/// The `noet://help/{name}` URI scheme is flat while the files are grouped, so
/// the stem must be matched against each compiled path rather than used
/// directly as a lookup key.
fn find_design_doc(stem: &str) -> Option<&'static File<'static>> {
    design_doc_files().find(|f| f.path().file_stem().is_some_and(|s| s == stem))
}

// ── Resource listing ──────────────────────────────────────────────────────────

/// Return all resources available from this MCP server.
///
/// Called in response to a `resources/list` request. Returns:
/// 1. The static orientation resource.
/// 2. One entry per served help doc compiled into the binary (see
///    `design_doc_files`).
pub fn list_resources() -> Vec<Resource> {
    let mut resources = Vec::new();

    // Orientation resource — always first so clients encounter it immediately.
    resources.push(orientation_resource_entry());

    // One entry per compiled design doc, across all topic subdirectories.
    for file in design_doc_files() {
        let path = file.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let uri = format!("noet://help/{}", stem);
        let name = format!("docs/{}", path.display());
        let description = format!("noet documentation: {}", stem.replace('_', " "));
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
    let Some(file) = find_design_doc(name) else {
        return Err(McpError::invalid_params(
            format!("Design doc not found: {name}.md"),
            None,
        ));
    };

    let raw = file.contents_utf8().ok_or_else(|| {
        McpError::internal_error(
            format!(
                "Help doc is not valid UTF-8: docs/{}",
                file.path().display()
            ),
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

    /// Design docs live in topic subdirectories under `docs/design`. `Dir::files()`
    /// is not recursive, so a non-recursive walk silently lists zero design docs.
    #[test]
    fn test_list_resources_includes_nested_design_docs() {
        let resources = list_resources();
        let design_docs = resources.len() - 1; // minus the orientation entry
        assert!(
            design_docs >= 20,
            "expected design docs from nested subdirs to be listed, got {design_docs}"
        );
    }

    /// A top-level user-facing reference must be served; MCP tool descriptions
    /// point agents at `docs/query_language.md`, so the server must hand it over.
    #[test]
    fn test_read_top_level_user_doc_by_stem() {
        let result = read_resource("noet://help/query_language")
            .expect("docs/query_language.md should be served");
        match result {
            ResourceContents::TextResourceContents { text, .. } => {
                assert!(text.contains("Quick Start"));
            }
            _ => panic!("expected text resource"),
        }
    }

    /// Issue trackers are not help content and must not be served.
    #[test]
    fn test_project_docs_not_served() {
        assert!(
            !list_resources()
                .iter()
                .any(|r| r.raw.name.starts_with("docs/project/")),
            "docs/project/ must be excluded from help resources"
        );
        assert!(read_resource("noet://help/ROADMAP").is_err());
    }

    /// A doc nested in `design/core/` must be reachable by its bare stem.
    #[test]
    fn test_read_nested_design_doc_by_stem() {
        let result = read_resource("noet://help/beliefbase_architecture")
            .expect("nested design doc should resolve by bare stem");
        match result {
            ResourceContents::TextResourceContents { text, .. } => {
                assert!(!text.is_empty());
            }
            _ => panic!("expected text resource"),
        }
    }
}
