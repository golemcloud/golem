// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A local HTTP server that serves the interactive oplog viewer in its live mode.
//!
//! The page itself embeds no entries. It calls the small JSON API of this server, which forwards
//! the requests to the worker service with the CLI's credentials. The browser never sees the
//! authentication token, and the worker service needs no CORS configuration for the viewer.

use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::log::{LogColorize, log_action, log_warn};
use crate::model::agent::oplog::AgentOplogEntryView;
use chrono::{DateTime, Utc};
use golem_client::api::WorkerClient;
use golem_common::model::oplog::OplogCursor;
use poem::http::StatusCode;
use poem::listener::{Acceptor, Listener, TcpListener};
use poem::web::{Data, Html, Json, Query};
use poem::{EndpointExt, Route, Server, get, handler};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

pub struct OplogViewerServer {
    pub ctx: Arc<Context>,
    pub component_id: Uuid,
    pub agent_id: String,
    /// The rendered viewer page in live mode.
    pub page: String,
    /// Number of entries the viewer requests per page by default.
    pub page_size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OplogPageQuery {
    from: Option<u64>,
    cursor_index: Option<u64>,
    cursor_revision: Option<u64>,
    count: Option<u64>,
    query: Option<String>,
    from_time: Option<DateTime<Utc>>,
    to_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OplogPageResponse {
    entries: Vec<AgentOplogEntryView>,
    next: Option<OplogCursor>,
    scanned_through_index: Option<u64>,
    last_index: u64,
    scan_limit_reached: bool,
}

#[handler]
async fn viewer_page(server: Data<&Arc<OplogViewerServer>>) -> Html<String> {
    Html(server.page.clone())
}

#[handler]
async fn oplog_page(
    server: Data<&Arc<OplogViewerServer>>,
    Query(params): Query<OplogPageQuery>,
) -> poem::Result<Json<OplogPageResponse>> {
    let cursor = match (params.cursor_index, params.cursor_revision) {
        (Some(next_oplog_index), Some(current_component_revision)) => Some(OplogCursor {
            next_oplog_index,
            current_component_revision,
        }),
        _ => None,
    };
    let count = params
        .count
        .unwrap_or(server.page_size)
        .clamp(1, server.page_size.max(1));
    let query = params.query.filter(|query| !query.trim().is_empty());

    let clients = server.ctx.golem_clients().await.map_err(upstream_error)?;
    let (entries, next, scanned_through_index, last_index, scan_limit_reached) =
        if query.is_some() || params.from_time.is_some() || params.to_time.is_some() {
            let result = clients
                .worker
                .search_oplog(
                    &server.component_id,
                    &server.agent_id,
                    &golem_client::model::SearchOplogRequest {
                        text: query,
                        from: params.from_time,
                        to: params.to_time,
                        count,
                        cursor,
                    },
                )
                .await
                .map_service_error()
                .map_err(upstream_error)?;
            (
                result.entries,
                result.next,
                result.scanned_through_index,
                result.last_index,
                result.scan_limit_reached,
            )
        } else {
            let result = clients
                .worker
                .get_oplog(
                    &server.component_id,
                    &server.agent_id,
                    params.from,
                    count,
                    cursor.as_ref(),
                )
                .await
                .map_service_error()
                .map_err(upstream_error)?;
            (result.entries, result.next, None, result.last_index, false)
        };

    Ok(Json(OplogPageResponse {
        entries: entries
            .into_iter()
            .map(|entry| AgentOplogEntryView {
                index: entry.oplog_index.as_u64(),
                attribution: entry.attribution,
                entry: entry.entry,
            })
            .collect(),
        next,
        scanned_through_index,
        last_index,
        scan_limit_reached,
    }))
}

#[handler]
async fn agent_metadata(
    server: Data<&Arc<OplogViewerServer>>,
) -> poem::Result<Json<serde_json::Value>> {
    let clients = server.ctx.golem_clients().await.map_err(upstream_error)?;
    let metadata = clients
        .worker
        .get_worker_metadata(&server.component_id, &server.agent_id)
        .await
        .map_service_error()
        .map_err(upstream_error)?;
    Ok(Json(
        serde_json::to_value(metadata).map_err(upstream_error)?,
    ))
}

fn upstream_error(error: impl std::fmt::Display) -> poem::Error {
    poem::Error::from_string(error.to_string(), StatusCode::BAD_GATEWAY)
}

/// Serves the viewer until the process is interrupted. Returns the error of the listener or of
/// the server; a clean interruption returns `Ok`.
pub async fn serve_oplog_viewer(
    server: OplogViewerServer,
    port: Option<u16>,
    open_browser: bool,
) -> anyhow::Result<()> {
    let server = Arc::new(server);
    let acceptor = TcpListener::bind(("127.0.0.1", port.unwrap_or(0)))
        .into_acceptor()
        .await?;
    let socket_addr = acceptor
        .local_addr()
        .iter()
        .find_map(|addr| addr.as_socket_addr().copied())
        .ok_or_else(|| anyhow::anyhow!("The viewer server has no local socket address"))?;
    let url = format!("http://{socket_addr}/");

    let app = Route::new()
        .at("/", get(viewer_page))
        .at("/api/oplog", get(oplog_page))
        .at("/api/agent", get(agent_metadata))
        .data(server);

    log_action(
        "Serving",
        format!(
            "oplog viewer at {} (press Ctrl-C to stop)",
            url.log_color_highlight()
        ),
    );
    if open_browser {
        open_in_browser(&url);
    }

    Server::new_with_acceptor(acceptor)
        .run_with_graceful_shutdown(
            app,
            async {
                let _ = tokio::signal::ctrl_c().await;
            },
            None,
        )
        .await?;
    Ok(())
}

/// Opens the URL in the default browser and reports the outcome; a failure to open the browser is
/// a warning, not an error, because the URL or path is printed anyway.
pub fn open_in_browser(url: &str) {
    match webbrowser::open(url) {
        Ok(()) => log_action("Opened", format!("{} in the default browser", url)),
        Err(error) => log_warn(format!("Could not open {url} in a browser: {error}")),
    }
}
