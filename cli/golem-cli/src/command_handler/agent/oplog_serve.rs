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

use crate::command_handler::agent::oplog_export::{RestOplogReads, read_failed};
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::log::{LogColorize, log_action, log_warn};
use crate::model::agent::oplog::AgentOplogEntryView;
use crate::model::agent::oplog_html::{
    OplogLiveExportViewerDocument, OplogViewerMode, render_oplog_live_export_viewer_html,
};
use crate::oplog_export::export::{OplogExportError, OplogExportOptions, OplogExportSession};
use crate::oplog_export::model::{OplogExportCoverage, ReportEntry};
use crate::oplog_export::obfuscator::TokenKey;
use golem_client::api::WorkerClient;
use golem_common::model::oplog::{OplogCursor, PublicOplogPageEntry};
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
    /// Show the entries that oplog pages omit in their omitted form instead of reading them.
    pub omit_payloads: bool,
    /// The export session of `--serve --obfuscate`. It holds the `Start` functions for the whole
    /// server process, and the mutex serialises the page calls.
    pub export: Option<tokio::sync::Mutex<OplogExportSession<RestOplogReads>>>,
}

/// The output view of an oplog page entry. Unless `omit_payloads` is set, an entry that the page
/// omitted is read in full. When that read fails, the view keeps the omitted form and the CLI
/// logs a warning, so that one entry does not hide the rest of the oplog.
pub async fn oplog_page_entry_view(
    ctx: &Context,
    component_id: &Uuid,
    agent_name: &str,
    page_entry: PublicOplogPageEntry,
    omit_payloads: bool,
) -> anyhow::Result<AgentOplogEntryView> {
    let omitted = match page_entry {
        PublicOplogPageEntry::Included(entry) => return Ok(entry.into()),
        PublicOplogPageEntry::Omitted(omitted) => omitted,
    };
    if omit_payloads {
        return Ok(omitted.into());
    }

    let clients = ctx.golem_clients().await?;
    match clients
        .worker
        .get_oplog_entry(
            component_id,
            agent_name,
            omitted.oplog_index.as_u64(),
            Some(omitted.component_revision.into()),
        )
        .await
        .map_service_error()
    {
        Ok(entry) => Ok(entry.into()),
        Err(error) => {
            log_warn(format!(
                "Failed to read oplog entry {} ({}, {} bytes), it stays omitted: {error:#}",
                omitted.oplog_index, omitted.entry_type, omitted.size
            ));
            Ok(omitted.into())
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OplogPageQuery {
    from: Option<u64>,
    cursor_index: Option<u64>,
    cursor_revision: Option<u64>,
    count: Option<u64>,
    query: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OplogPageResponse {
    entries: Vec<AgentOplogEntryView>,
    next: Option<OplogCursor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OplogExportPageResponse {
    entries: Vec<ReportEntry>,
    next: Option<OplogCursor>,
    coverage: OplogExportCoverage,
}

const EXPORT_FAILED_TEXT: &str = "The export of the agent failed.";

const ROOT_CHANGED_TEXT: &str =
    "The agent was deleted and created again. Restart the viewer to read the new agent.";

#[handler]
async fn viewer_page(server: Data<&Arc<OplogViewerServer>>) -> Html<String> {
    Html(server.page.clone())
}

#[handler]
async fn oplog_page(
    server: Data<&Arc<OplogViewerServer>>,
    Query(params): Query<OplogPageQuery>,
) -> poem::Result<Json<serde_json::Value>> {
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

    if let Some(export) = &server.export {
        let mut session = export.lock().await;
        session.check_root().await.map_err(export_error)?;
        let page = session
            .render_page(params.from, cursor.as_ref(), count, query.as_deref())
            .await
            .map_err(export_error)?;
        session.check_root().await.map_err(export_error)?;
        return Ok(Json(
            serde_json::to_value(OplogExportPageResponse {
                entries: page.entries,
                next: page.next,
                coverage: page.coverage,
            })
            .map_err(|error| export_error(error.into()))?,
        ));
    }

    let clients = server.ctx.golem_clients().await.map_err(upstream_error)?;
    let result = clients
        .worker
        .get_oplog(
            &server.component_id,
            &server.agent_id,
            params.from,
            count,
            cursor.as_ref(),
            query.as_deref(),
        )
        .await
        .map_service_error()
        .map_err(upstream_error)?;

    let mut entries = Vec::with_capacity(result.entries.len());
    for page_entry in result.entries {
        entries.push(
            oplog_page_entry_view(
                &server.ctx,
                &server.component_id,
                &server.agent_id,
                page_entry,
                server.omit_payloads,
            )
            .await
            .map_err(upstream_error)?,
        );
    }

    Ok(Json(
        serde_json::to_value(OplogPageResponse {
            entries,
            next: result.next,
        })
        .map_err(upstream_error)?,
    ))
}

#[handler]
async fn agent_metadata(
    server: Data<&Arc<OplogViewerServer>>,
) -> poem::Result<Json<serde_json::Value>> {
    if let Some(export) = &server.export {
        return export_agent_metadata(&server, export)
            .await
            .map(Json)
            .map_err(export_error);
    }
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

/// The agent metadata of the served export: only the fields of the status line, and only while
/// the agent is the pinned root.
async fn export_agent_metadata(
    server: &OplogViewerServer,
    export: &tokio::sync::Mutex<OplogExportSession<RestOplogReads>>,
) -> anyhow::Result<serde_json::Value> {
    let clients = server.ctx.golem_clients().await.map_err(read_failed)?;
    let metadata = clients
        .worker
        .get_worker_metadata(&server.component_id, &server.agent_id)
        .await
        .map_service_error()
        .map_err(read_failed)?;
    export
        .lock()
        .await
        .check_fingerprint(metadata.fingerprint.0)?;
    let metadata = serde_json::to_value(metadata)?;
    let mut out = serde_json::Map::new();
    for field in EXPORT_AGENT_METADATA_FIELDS {
        if let Some(value) = metadata.get(field) {
            out.insert(field.to_string(), value.clone());
        }
    }
    Ok(serde_json::Value::Object(out))
}

/// The fields of the agent metadata that the viewer reads for its status line. The served
/// viewer of an export answers only these, never the agent ID, the last error, env or config.
const EXPORT_AGENT_METADATA_FIELDS: [&str; 7] = [
    "status",
    "lastOplogIndex",
    "retryCount",
    "componentRevision",
    "pendingInvocationCount",
    "skippedRegions",
    "deletedRegions",
];

fn upstream_error(error: impl std::fmt::Display) -> poem::Error {
    poem::Error::from_string(error.to_string(), StatusCode::BAD_GATEWAY)
}

/// The response of a failed request of the served export. It holds only fixed texts: the text of
/// an [`OplogExportError`], or one text for every other error.
fn export_error(error: anyhow::Error) -> poem::Error {
    match error.downcast_ref::<OplogExportError>() {
        Some(OplogExportError::RootChanged) => {
            poem::Error::from_string(ROOT_CHANGED_TEXT, StatusCode::CONFLICT)
        }
        Some(error) => poem::Error::from_string(error.to_string(), StatusCode::BAD_GATEWAY),
        None => poem::Error::from_string(EXPORT_FAILED_TEXT, StatusCode::BAD_GATEWAY),
    }
}

/// Pins the export session, then serves the viewer of the obfuscated export until the process
/// is interrupted. A failed pin fails the command before the server starts.
pub async fn serve_oplog_export_viewer(
    ctx: Arc<Context>,
    component_id: Uuid,
    agent_id: String,
    options: OplogExportOptions,
    port: Option<u16>,
    open_browser: bool,
) -> anyhow::Result<()> {
    let from = options.from;
    let query = options.query.clone();
    let page_size = options.page_size;
    let reads = RestOplogReads {
        ctx: ctx.clone(),
        component_id,
        agent_name: agent_id.clone(),
    };
    let session = OplogExportSession::pin(reads, TokenKey::process(), options).await?;
    let page = render_oplog_live_export_viewer_html(&OplogLiveExportViewerDocument {
        mode: OplogViewerMode::LiveExport,
        header: session.header(),
        from,
        query,
        page_size,
    })?;
    serve_oplog_viewer(
        OplogViewerServer {
            ctx,
            component_id,
            agent_id,
            page,
            page_size,
            omit_payloads: false,
            export: Some(tokio::sync::Mutex::new(session)),
        },
        port,
        open_browser,
    )
    .await
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
