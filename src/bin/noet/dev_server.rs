//! Development server with live reload for HTML output
//!
//! Provides a simple HTTP server that:
//! - Serves static HTML files from the output directory
//! - Watches HTML directory for changes via filesystem notifications
//! - Sends Server-Sent Events (SSE) to notify clients of file changes

use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    routing::get,
    Router,
};
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use std::{convert::Infallible, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tower_http::{services::ServeDir, trace::TraceLayer};

/// Notification sent when files change and HTML is regenerated
#[derive(Debug, Clone)]
pub struct ReloadNotification {
    /// Optional path that changed (for future granular reload)
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
}

/// Shared state for the dev server
#[derive(Clone)]
struct DevServerState {
    /// Broadcast channel for reload notifications
    reload_tx: broadcast::Sender<ReloadNotification>,
    /// Root directory being served
    #[allow(dead_code)]
    html_root: PathBuf,
}

/// Development server for viewing HTML output with live reload
pub struct DevServer {
    /// Broadcast sender for notifying clients of changes
    reload_tx: broadcast::Sender<ReloadNotification>,
    /// Port the server is running on
    port: u16,
    /// HTML output directory
    html_root: PathBuf,
}

impl DevServer {
    /// Create a new dev server
    ///
    /// # Arguments
    /// * `html_root` - Directory containing generated HTML files
    /// * `port` - Port to bind the server to
    pub fn new(html_root: PathBuf, port: u16) -> Self {
        // Channel capacity: keep last 100 reload events
        let (reload_tx, _) = broadcast::channel(100);

        Self {
            reload_tx,
            port,
            html_root,
        }
    }

    /// Start the dev server (blocking until shutdown signal)
    pub async fn serve(
        self,
        shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));

        let state = DevServerState {
            reload_tx: self.reload_tx.clone(),
            html_root: self.html_root.clone(),
        };

        // Build the router
        let app = Router::new()
            .route("/events", get(sse_handler))
            .nest_service("/", ServeDir::new(&self.html_root))
            .layer(TraceLayer::new_for_http())
            .with_state(state);

        tracing::info!("Dev server starting on http://{}", addr);
        println!("\n🚀 Dev server running at http://{}", addr);
        println!("📁 Serving: {}", self.html_root.display());
        println!("🔄 Live reload enabled\n");

        // Start file watcher for HTML directory
        let reload_tx_for_watcher = self.reload_tx.clone();
        let html_root_clone = self.html_root.clone();

        std::thread::spawn(move || {
            let mut debouncer = new_debouncer(
                Duration::from_millis(500),
                None,
                move |result: DebounceEventResult| {
                    match result {
                        Ok(events) => {
                            // Check if any .html files changed
                            let has_html_change = events.iter().any(|event| {
                                event.paths.iter().any(|path| {
                                    path.extension()
                                        .and_then(|ext| ext.to_str())
                                        .map(|ext| ext == "html")
                                        .unwrap_or(false)
                                })
                            });

                            if has_html_change {
                                tracing::debug!(
                                    "[DevServer] HTML file changed, sending reload notification"
                                );
                                let _ =
                                    reload_tx_for_watcher.send(ReloadNotification { path: None });
                            }
                        }
                        Err(errors) => {
                            tracing::warn!("[DevServer] File watcher errors: {:?}", errors);
                        }
                    }
                },
            )
            .expect("Failed to create file watcher for dev server");

            debouncer
                .watcher()
                .watch(&html_root_clone, RecursiveMode::Recursive)
                .expect("Failed to watch HTML directory");

            tracing::info!(
                "[DevServer] File watcher started for {}",
                html_root_clone.display()
            );

            // Keep the watcher alive
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        });

        // Start the server with graceful shutdown
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(shutdown_signal)
            .await?;

        tracing::info!("Dev server shut down");
        Ok(())
    }
}

/// SSE endpoint handler
async fn sse_handler(
    State(state): State<DevServerState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.reload_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let stream = stream.map(|result| {
        match result {
            Ok(_notification) => {
                // Send reload event to browser
                Ok(Event::default().event("reload").data("reload"))
            }
            Err(_) => {
                // Lagged behind, send reload anyway
                Ok(Event::default().event("reload").data("reload"))
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
