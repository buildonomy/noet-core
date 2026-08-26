//! MCP (Model Context Protocol) server for the noet BeliefBase.
//!
//! This module exposes a `noet mcp` CLI entry point that starts an MCP server
//! over stdio, giving AI agents structured access to a compiled BeliefBase via
//! a small set of high-value tools and resources.
//!
//! ## Architecture
//!
//! ```text
//! noet mcp --output-dir <path>
//!     │
//!     ├── McpState::load_static(path)   ← reads manifest.json + *.msgpack shards
//!     │
//!     └── BeliefBaseServer (rmcp ServerHandler)
//!             ├── tools:     get_networks, search, get_context, get_submap, query, check_consistency
//!             ├── resources: noet://help/orientation, noet://help/{name}
//!             └── (no prompts — application-specific prompts belong in the corpus)
//! ```
//!
//! ## Transport
//!
//! Stdio only (this issue). HTTP/SSE transport is deferred until a concrete
//! multi-agent or collaboration use case exists (see Issue 65).
//!
//! ## Static vs. live mode
//!
//! Currently only static mode (`--output-dir`) is implemented. Live mode
//! (subscribing to `FileUpdateSyncer::belief_broadcast`) is deferred to
//! Issue 64 Step 2.
//!
//! ## Module layout
//!
//! - `mod.rs` (this file) — server struct, `ServerHandler` impl, `run_mcp_server`
//! - [`state`]  — `McpState`: loaded manifest + `BeliefBase`
//! - [`tools`]  — one function per MCP tool
//! - [`types`]  — MCP-specific JSON output structs (independent of `wasm.rs` types)
//! - [`resources`] — `noet://help/*` resource handlers

//! - `orientation.md` — LLM-targeted orientation doc, compiled into the binary

pub mod resources;
pub mod state;
pub mod tools;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "service")]
use crate::db::DbConnection;
#[cfg(feature = "service")]
use crate::shard::manifest::{GlobalShardMeta, NetworkShardMeta, ShardManifest};
use rmcp::{
    model::{
        CallToolResult, Content, Implementation, ListResourceTemplatesResult, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
        Tool,
    },
    serve_server,
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use serde_json::Map as SjMap;

use crate::error::BuildonomyError;
use state::McpState;

// ── Server struct ─────────────────────────────────────────────────────────────

/// The rmcp server handler for the noet BeliefBase MCP server.
///
/// Holds an `Arc<McpState>` so the loaded BeliefBase is shared across all
/// concurrent handler invocations without cloning.
#[derive(Clone)]
pub struct BeliefBaseServer {
    state: Arc<parking_lot::RwLock<Arc<McpState>>>,
    /// Mtime of `manifest.json` at last load, for auto-reload detection.
    /// `None` in live mode (no file watching needed).
    manifest_mtime: Arc<parking_lot::Mutex<Option<std::time::SystemTime>>>,
}

impl BeliefBaseServer {
    pub fn new(state: Arc<McpState>) -> Self {
        let mtime = state
            .output_dir
            .as_ref()
            .and_then(|dir| manifest_mtime(dir));
        Self {
            state: Arc::new(parking_lot::RwLock::new(state)),
            manifest_mtime: Arc::new(parking_lot::Mutex::new(mtime)),
        }
    }

    /// Check if the manifest file has changed since the last load.
    /// If so, reload the BeliefBase from the output directory.
    fn maybe_reload(&self) {
        let state = self.state.read().clone();
        let Some(ref output_dir) = state.output_dir else {
            return; // live mode — no file watching
        };

        let current_mtime = match manifest_mtime(output_dir) {
            Some(t) => t,
            None => return, // manifest doesn't exist (yet)
        };

        let mut last = self.manifest_mtime.lock();
        if *last == Some(current_mtime) {
            return; // no change
        }

        tracing::info!(
            "manifest.json changed on disk — reloading BeliefBase from {}",
            output_dir.display()
        );

        match McpState::load_static(output_dir) {
            Ok(new_state) => {
                *self.state.write() = new_state;
                *last = Some(current_mtime);
                tracing::info!(
                    networks = self.state.read().manifest.networks.len(),
                    "BeliefBase reloaded successfully"
                );
            }
            Err(e) => {
                tracing::warn!("Failed to reload BeliefBase: {e} — continuing with stale data");
            }
        }
    }
}

/// Get the mtime of the BeliefBase output marker file.
/// Checks sharded layout (`beliefbase/manifest.json`) first, then monolithic
/// (`beliefbase.msgpack`). Returns the mtime of whichever exists.
fn manifest_mtime(output_dir: &std::path::Path) -> Option<std::time::SystemTime> {
    let sharded = output_dir.join("beliefbase").join("manifest.json");
    let monolithic = output_dir.join("beliefbase.msgpack");
    std::fs::metadata(&sharded)
        .or_else(|_| std::fs::metadata(&monolithic))
        .and_then(|m| m.modified())
        .ok()
}

// ── ServerHandler impl ────────────────────────────────────────────────────────

impl ServerHandler for BeliefBaseServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_server_info(Implementation::new("noet-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "BeliefBase MCP server. Call get_networks first to orient, \
             then use search, get_context, get_submap, query, or check_consistency. \
             The noet://help/orientation resource contains full usage guidance.",
        )
    }

    // ── Tools ─────────────────────────────────────────────────────────────────

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Build tool descriptors using Tool::new(name, description, schema).
        // Tool is #[non_exhaustive] so struct literal construction is not allowed.
        let tools = vec![
            Tool::new(
                "get_networks",
                "List all compiled networks in the BeliefBase with stats and an orientation note. \
                 Call this first in any agent session.",
                schema_for::<types::GetNetworksInput>(),
            ),
            Tool::new(
                "search",
                "Full-text TF-IDF search across all loaded networks. Returns ranked results \
                 with BIDs, titles, and snippets. Use result BIDs with get_context.",
                schema_for::<types::SearchInput>(),
            ),
            Tool::new(
                "get_context",
                "Return a node and its full relationship context: sources (children), \
                 sinks (parents), typed edges, and owned edges from {maps_to} directives. \
                 This is the primary navigation tool.",
                schema_for::<types::GetContextInput>(),
            ),
            Tool::new(
                "get_submap",
                "Return a pragmatic-edge traceability subgraph rooted at a BID. \
                 Useful for exploring the dependency tree around a node.",
                schema_for::<types::GetSubmapInput>(),
            ),
            Tool::new(
                "query",
                "Execute a query against the BeliefBase using the textual grammar. \
                 See docs/design/query_model.md §9.5 for syntax. \
                 Examples: \"id://my-network composed_of(*)\", \
                 \"title:auth AND schema:procedure\", \
                 \"id:class-a uses(1) NOT id:class-b uses(1)\".",
                schema_for::<types::QueryInput>(),
            ),
            Tool::new(
                "check_consistency",
                "Surface unresolved cross-references and orphaned edges. \
                 Returns a summary of structural issues in the loaded BeliefBase.",
                schema_for::<types::CheckConsistencyInput>(),
            ),
            Tool::new(
                "bref",
                "Translate a full BID (UUID) into its 5-character hex bref alias. \
                 Pure computation — no BeliefBase lookup required. Use this when you \
                 have a BID from one tool and need the bref for display, filtering, \
                 or passing to another tool's network parameter.",
                schema_for::<types::BrefInput>(),
            ),
            Tool::new(
                "get_traceability",
                "Return the direct edge-count matrix for the structural submap of a node. \
                 Rows are nodes reachable via Section edges; columns are in/out counts per \
                 WeightKind (section, epistemic, pragmatic). Answers: what is the connectivity \
                 of each node in this structural scope?",
                schema_for::<types::GetTraceabilityInput>(),
            ),
            Tool::new(
                "get_maps_to",
                "Return the flat list of {maps_to} claims for a set of owner BIDs. \
                 Scans each node's owned_edges for traceability claims (source → sink via owner). \
                 Cheaper than get_maps_to_traceability — pass BIDs directly without submap \
                 resolution. Answers: what does each of these nodes claim to cover?",
                schema_for::<types::GetMapsToInput>(),
            ),
            Tool::new(
                "get_maps_to_traceability",
                "Return the full three-level claim index for the structural submap of a node: \
                 owner → sink → {kind: [sources]}. This is the maps_to mode of the traceability \
                 matrix — the compliance-review primitive. Answers: for each item in my \
                 traceability network, what does it claim to cover, and via which source nodes \
                 per relationship kind? More expensive than get_traceability or get_maps_to.",
                schema_for::<types::GetMapsToTraceabilityInput>(),
            ),
        ];

        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Auto-reload if the output directory's manifest.json has changed.
        self.maybe_reload();
        let state = self.state.read().clone();
        let args = request.arguments.unwrap_or_default();

        let json_result: serde_json::Value = match request.name.as_ref() {
            "get_networks" => {
                let input: types::GetNetworksInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::get_networks(&state, input)?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "search" => {
                let input: types::SearchInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::search(&state, input)?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "get_context" => {
                let input: types::GetContextInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::get_context(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "get_submap" => {
                let input: types::GetSubmapInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::get_submap(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "query" => {
                let input: types::QueryInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::query(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "check_consistency" => {
                let input: types::CheckConsistencyInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::check_consistency(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "bref" => {
                let input: types::BrefInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::bref(&state, input)?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "get_traceability" => {
                let input: types::GetTraceabilityInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::get_traceability(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "get_maps_to" => {
                let input: types::GetMapsToInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::get_maps_to(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            "get_maps_to_traceability" => {
                let input: types::GetMapsToTraceabilityInput =
                    serde_json::from_value(serde_json::Value::Object(args))
                        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                serde_json::to_value(tools::get_maps_to_traceability(&state, input).await?)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?
            }
            other => {
                return Err(McpError::invalid_params(
                    format!("Unknown tool: {other}"),
                    None,
                ))
            }
        };

        let content = Content::json(json_result)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![content]))
    }

    // ── Resources ─────────────────────────────────────────────────────────────

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: resources::list_resources(),
            ..Default::default()
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: resources::list_resource_templates(),
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResult, McpError> {
        let contents = resources::read_resource(&request.uri)?;
        Ok(rmcp::model::ReadResourceResult::new(vec![contents]))
    }
}

// ── Schema helper ─────────────────────────────────────────────────────────────

/// Generate an `Arc<JsonObject>` schema from a type that implements `schemars::JsonSchema`.
///
/// Used to populate `Tool.input_schema` for all MCP tools. The schema describes
/// the expected JSON structure of the tool's input, enabling MCP clients to
/// validate and autocomplete tool arguments.
fn schema_for<T: schemars::JsonSchema>() -> Arc<SjMap<String, serde_json::Value>> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(&schema).expect("schemars output is always valid JSON");
    Arc::new(json.as_object().cloned().unwrap_or_default())
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Start the MCP server in static mode and serve until the client disconnects.
///
/// Called from `src/bin/noet/main.rs` when `noet mcp --output-dir <path>` is invoked.
/// Loads shard files from `output_dir` into an in-memory `BeliefBase` and serves
/// queries against it. Results reflect the last `noet parse` run.
///
/// When `output_dir` is `None`, returns an error directing the user to use either
/// `--output-dir` or `--watch`.
pub fn run_mcp_server(output_dir: Option<PathBuf>) -> Result<(), BuildonomyError> {
    let path = output_dir.ok_or_else(|| {
        BuildonomyError::Service(
            "No source specified. Use --output-dir <path> for static mode or \
             --watch <path> for live mode."
                .to_string(),
        )
    })?;

    tracing::info!(path = %path.display(), "loading BeliefBase from output directory (static mode)");
    let state = McpState::load_static(&path)?;

    tracing::info!(
        networks = state.manifest.networks.len(),
        "BeliefBase loaded — starting MCP server on stdio"
    );

    run_server(state)
}

/// Start the MCP server in live mode using a `DbConnection` from a running `WatchService`.
///
/// Called from `src/bin/noet/main.rs` when `noet mcp --watch <path>` is invoked.
/// Clones the `WatchService`'s `DbConnection` (`global_bb`) as the query source.
/// The transaction task continuously commits compiled nodes to `belief_cache.db`;
/// MCP reads whatever has been committed so far. No subscriber task or in-memory
/// rebuild is required — `DbConnection` is `Arc`-backed and cheap to clone.
///
/// `html_output` is optional: if provided, the `search` tool loads `.idx.msgpack`
/// files from `<html_output>/search/`. If absent, search returns empty results.
#[cfg(feature = "service")]
pub fn run_mcp_server_live(
    db: DbConnection,
    html_output: Option<PathBuf>,
    watch_service: &crate::watch::WatchService,
) -> Result<(), BuildonomyError> {
    // Build a ShardManifest from the known networks so get_networks returns useful output.
    let manifest = manifest_from_watch_service(watch_service);

    tracing::info!(
        networks = manifest.networks.len(),
        "live DB connected — starting MCP server on stdio"
    );

    let state = McpState::from_db(db, manifest, html_output);
    run_server(state)
}

/// Shared server startup: construct the rmcp service and block until disconnect.
fn run_server(state: std::sync::Arc<McpState>) -> Result<(), BuildonomyError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| BuildonomyError::Service(format!("Failed to build tokio runtime: {e}")))?;

    runtime.block_on(async move {
        let server = BeliefBaseServer::new(state);
        let service = serve_server(server, rmcp::transport::stdio())
            .await
            .map_err(|e| BuildonomyError::Service(format!("MCP server startup failed: {e}")))?;

        tracing::info!("MCP server running — waiting for client to disconnect");
        service
            .waiting()
            .await
            .map(|_| ())
            .map_err(|e| BuildonomyError::Service(format!("MCP server join error: {e}")))
    })
}

/// Build a minimal `ShardManifest` from the networks registered in a `WatchService`.
///
/// In live mode there are no shard files, so shard-path and size fields are
/// left empty/zero. The manifest is only used by `get_networks` to list network
/// titles and brefs for the agent's orientation call.
#[cfg(feature = "service")]
fn manifest_from_watch_service(service: &crate::watch::WatchService) -> ShardManifest {
    let networks = service
        .get_networks()
        .unwrap_or_default()
        .into_iter()
        .map(|rec| NetworkShardMeta {
            bref: rec.node.bid.bref().to_string(),
            bid: rec.node.bid.to_string(),
            title: rec.node.title.clone(),
            node_count: 0,
            relation_count: 0,
            estimated_size_mb: 0.0,
            path: String::new(), // no shard file in live mode
            search_index_path: String::new(),
            search_index_size_kb: 0.0,
        })
        .collect();

    ShardManifest {
        version: "1.0".to_string(),
        sharded: false,
        memory_budget_mb: 0.0,
        networks,
        global: GlobalShardMeta {
            node_count: 0,
            estimated_size_mb: 0.0,
            path: String::new(),
        },
    }
}
