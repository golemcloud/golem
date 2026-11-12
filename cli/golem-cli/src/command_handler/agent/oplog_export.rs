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

//! `golem agent oplog --obfuscate`: the export over the REST oplog reads of the CLI.

use crate::command::shared_args::{OplogExportArgs, OplogViewerArgs};
use crate::command_handler::Handlers;
use crate::command_handler::agent::oplog_serve::open_in_browser;
use crate::context::Context;
use crate::error::service::{MapServiceError, ServiceError, ServiceErrorKind};
use crate::fs;
use crate::log::{LogColorize, log_action, log_warn};
use crate::model::agent::oplog_export_view::{
    AgentOplogExportCoverageView, AgentOplogExportEntryView, AgentOplogExportHeaderView,
};
use crate::model::agent::oplog_html::OplogExportHtmlWriter;
use crate::oplog_export::export::{
    OplogExportError, OplogExportOptions, OplogExportSink, OplogPage, OplogReads, export_oplog,
};
use crate::oplog_export::model::{OplogExportCoverage, OplogExportHeader, ReportEntry};
use crate::oplog_export::obfuscator::{KeepFlags, TokenKey};
use async_trait::async_trait;
use chrono::Utc;
use golem_client::api::WorkerClient;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntryWithIndex};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// The oplog reads of one agent through the worker service.
pub struct RestOplogReads {
    pub ctx: Arc<Context>,
    pub component_id: Uuid,
    pub agent_name: String,
}

#[async_trait]
impl OplogReads for RestOplogReads {
    async fn read_page(
        &self,
        from: Option<u64>,
        count: u64,
        cursor: Option<&OplogCursor>,
        query: Option<&str>,
    ) -> anyhow::Result<OplogPage> {
        let clients = self.ctx.golem_clients().await.map_err(read_failed)?;
        let response = clients
            .worker
            .get_oplog(
                &self.component_id,
                &self.agent_name,
                from,
                count,
                cursor,
                query,
            )
            .await
            .map_service_error()
            .map_err(read_failed)?;
        Ok(OplogPage {
            entries: response.entries,
            next: response.next,
        })
    }

    async fn read_entry(
        &self,
        oplog_index: u64,
        component_revision: ComponentRevision,
    ) -> anyhow::Result<PublicOplogEntryWithIndex> {
        let clients = self.ctx.golem_clients().await.map_err(read_failed)?;
        Ok(clients
            .worker
            .get_oplog_entry(
                &self.component_id,
                &self.agent_name,
                oplog_index,
                Some(component_revision.into()),
            )
            .await
            .map_service_error()
            .map_err(read_failed)?)
    }
}

/// The error of a failed read of the export: a fixed text with the HTTP status, if any. The raw
/// error is dropped, because its text can quote the agent ID, the component and environment IDs
/// or other upstream text.
pub fn read_failed(error: impl Into<anyhow::Error>) -> anyhow::Error {
    let error = error.into();
    let status = error
        .downcast_ref::<ServiceError>()
        .and_then(|error| match &error.kind {
            ServiceErrorKind::ErrorResponse(response) => Some(response.status_code),
            ServiceErrorKind::UnexpectedResponse { status_code, .. } => Some(*status_code),
            ServiceErrorKind::ReqwestError(error) => error.status().map(|status| status.as_u16()),
            ServiceErrorKind::MiddlewareError(_)
            | ServiceErrorKind::ReqwestHeaderError(_)
            | ServiceErrorKind::SerdeError(_) => None,
        });
    match status {
        Some(status) => OplogExportError::ReadFailed { status }.into(),
        None => OplogExportError::ReadFailedWithoutResponse.into(),
    }
}

pub fn export_options(
    export: &OplogExportArgs,
    from: Option<u64>,
    query: Option<String>,
    page_size: u64,
) -> OplogExportOptions {
    OplogExportOptions {
        keep: KeepFlags {
            scalars: export.keep_scalars,
            shape: export.keep_shape,
            error_text: export.keep_error_text,
        },
        max_entry_bytes: export.max_entry_bytes,
        from,
        query,
        page_size,
        cli_version: crate::version().to_string(),
        generated_at: Utc::now().to_rfc3339(),
    }
}

/// Prints the documents of an export and writes its HTML page.
struct CliExportSink {
    ctx: Arc<Context>,
    print: bool,
    html_path: Option<PathBuf>,
    html: Option<OplogExportHtmlWriter>,
    entry_count: usize,
}

impl OplogExportSink for CliExportSink {
    fn header(&mut self, header: &OplogExportHeader) -> anyhow::Result<()> {
        if let Some(path) = &self.html_path {
            let mut writer = OplogExportHtmlWriter::create(path, &header.root.pseudonym())?;
            writer.header(header)?;
            self.html = Some(writer);
        }
        if self.print {
            self.ctx
                .log_handler()
                .log_output(AgentOplogExportHeaderView(header.clone()))?;
        }
        Ok(())
    }

    fn entry(&mut self, entry: &ReportEntry) -> anyhow::Result<()> {
        if let Some(writer) = &mut self.html {
            writer.entry(entry)?;
        }
        self.entry_count += 1;
        if self.print {
            self.ctx
                .log_handler()
                .log_output(AgentOplogExportEntryView(entry.clone()))?;
        }
        Ok(())
    }

    fn coverage(&mut self, coverage: &OplogExportCoverage) -> anyhow::Result<()> {
        if let Some(writer) = self.html.take() {
            writer.finish(coverage)?;
        }
        if self.print {
            self.ctx
                .log_handler()
                .log_output(AgentOplogExportCoverageView(coverage.clone()))?;
        }
        Ok(())
    }
}

/// Exports the obfuscated oplog of one agent to stdout and, with `--html`, to a viewer page.
pub async fn cmd_oplog_export(
    ctx: Arc<Context>,
    component_id: Uuid,
    agent_name: String,
    options: OplogExportOptions,
    viewer: OplogViewerArgs,
) -> anyhow::Result<()> {
    let print = viewer.html.is_none() || ctx.format().is_structured();
    let reads = RestOplogReads {
        ctx: ctx.clone(),
        component_id,
        agent_name,
    };
    let mut sink = CliExportSink {
        ctx: ctx.clone(),
        print,
        html_path: viewer.html.clone(),
        html: None,
        entry_count: 0,
    };
    export_oplog(reads, TokenKey::process(), options, &mut sink).await?;

    if let Some(html_path) = viewer.html {
        log_action(
            "Generated",
            format!(
                "obfuscated HTML oplog report with {} entries: {}",
                sink.entry_count,
                html_path.display().to_string().log_color_highlight()
            ),
        );
        if viewer.open {
            let absolute = fs::absolute_lexical_path(&html_path)?;
            match url::Url::from_file_path(&absolute) {
                Ok(file_url) => open_in_browser(file_url.as_str()),
                Err(()) => log_warn(format!(
                    "Could not build a file URL for {}",
                    absolute.display()
                )),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::read_failed;
    use crate::error::service::{ServiceError, ServiceErrorKind, ServiceErrorResponse};
    use test_r::test;

    #[test]
    fn read_failed_keeps_only_the_status() {
        let error = ServiceError {
            service_name: "Worker",
            kind: ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                status_code: 400,
                errors: vec!["agent env-1/comp-1/Probe(\"MARKER-7\") does not exist".to_string()],
                code: Some("MARKER-7".to_string()),
                additional_fields: Default::default(),
            }),
        };
        let error = read_failed(anyhow::Error::from(error).context("reading MARKER-7"));
        assert_eq!(
            format!("{error:#}"),
            "A read of the agent failed with HTTP status 400"
        );
        assert_eq!(
            format!("{:#}", read_failed(anyhow::anyhow!("MARKER-7"))),
            "A read of the agent failed before the server answered"
        );
    }
}
