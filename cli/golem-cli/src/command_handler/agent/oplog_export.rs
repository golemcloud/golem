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
use crate::model::agent::RawAgentId;
use crate::model::agent::oplog_export_view::{
    AgentOplogExportCoverageView, AgentOplogExportEntryView, AgentOplogExportHeaderView,
};
use crate::model::agent::oplog_html::OplogExportHtmlWriter;
use crate::oplog_export::export::{
    OplogExportError, OplogExportOptions, OplogExportSink, OplogPage, OplogRange, OplogRangePage,
    OplogReads, export_oplog, export_oplog_range,
};
use crate::oplog_export::model::{
    OplogExportCoverage, OplogExportHeader, OplogExportReport, ReportEntry,
};
use crate::oplog_export::obfuscator::{KeepFlags, TokenKey};
use async_trait::async_trait;
use golem_client::api::WorkerClient;
use golem_common::base_model::api::error_code;
use golem_common::model::AgentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogCursor, OplogScanAllowance, PublicOplogEntryWithIndex};
use jiff::Timestamp;
use std::path::PathBuf;
use std::sync::Arc;

/// The oplog reads of one agent through the worker service.
pub struct RestOplogReads {
    pub ctx: Arc<Context>,
    pub environment_id: EnvironmentId,
    pub agent_id: AgentId,
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
                &self.environment_id.0,
                &self.agent_id.0,
                from,
                count,
                cursor,
                query,
                // An export never asks for claims.
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .map_service_error()
            .map_err(read_failed)?;
        Ok(OplogPage {
            entries: response.entries,
            next: response.next,
            last_index: response.last_index,
        })
    }

    async fn read_range_page(
        &self,
        from: u64,
        to: u64,
        count: u64,
        cursor: Option<&OplogCursor>,
        allowance: Option<OplogScanAllowance>,
    ) -> anyhow::Result<OplogRangePage> {
        let clients = self.ctx.golem_clients().await.map_err(read_failed)?;
        let response = clients
            .worker_range_reads
            .get_oplog(
                &self.environment_id.0,
                &self.agent_id.0,
                Some(from),
                count,
                cursor,
                None,
                // An export never asks for claims.
                None,
                None,
                Some(to),
                allowance.map(|allowance| allowance.entries),
                allowance.map(|allowance| allowance.millis),
            )
            .await
            .map_service_error()
            .map_err(read_failed)?;
        let scan = response.scan.ok_or(OplogExportError::RangePageInvalid)?;
        Ok(OplogRangePage {
            entries: response.entries,
            next: response.next,
            scan,
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
                &self.environment_id.0,
                &self.agent_id.0,
                oplog_index,
                Some(component_revision.into()),
                // An export never asks for claims.
                None,
                None,
            )
            .await
            .map_service_error()
            .map_err(read_failed)?)
    }

    async fn read_create_entry(&self) -> anyhow::Result<PublicOplogEntryWithIndex> {
        let clients = self.ctx.golem_clients().await.map_err(read_failed)?;
        Ok(clients
            .worker
            .get_oplog_entry(
                &self.environment_id.0,
                &self.agent_id.0,
                1,
                None,
                // An export never asks for claims.
                None,
                None,
            )
            .await
            .map_service_error()
            .map_err(read_failed)?)
    }
}

/// The error of a failed read of the export: a fixed text with the HTTP status, if any. The raw
/// error stays out of the report and the command output, because its text can quote the agent
/// ID, the component and environment IDs or other upstream text. A debug log shows it on the
/// user's own terminal, for example to tell an auth, TLS or DNS failure apart.
pub fn read_failed(error: impl Into<anyhow::Error>) -> anyhow::Error {
    let error = error.into();
    tracing::debug!(
        error = format!("{error:#}"),
        "Failed to read the oplog for the export"
    );
    let service_error = error.downcast_ref::<ServiceError>();
    match service_error.and_then(ServiceError::error_code) {
        Some(error_code::STALE_CURSOR) => return OplogExportError::StaleCursor.into(),
        Some(error_code::OPLOG_EXPIRED) => return OplogExportError::Expired.into(),
        _ => {}
    }
    let status = service_error.and_then(|error| match &error.kind {
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
        generated_at: Timestamp::now().to_string(),
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
            let mut writer = OplogExportHtmlWriter::create(path, &header.root.label())?;
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
    environment_id: EnvironmentId,
    agent_id: AgentId,
    options: OplogExportOptions,
    viewer: OplogViewerArgs,
) -> anyhow::Result<()> {
    let print = viewer.html.is_none() || ctx.format().is_structured();
    let reads = RestOplogReads {
        ctx: ctx.clone(),
        environment_id,
        agent_id,
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

/// Builds one bounded obfuscated report of the range `range` of the oplog of `agent_id`, in
/// memory. The report holds no state after the call: each call pins the root again and starts
/// with the whole scan allowance of the server.
pub async fn oplog_export_report(
    ctx: Arc<Context>,
    agent_id: RawAgentId,
    keep: KeepFlags,
    max_entry_bytes: u64,
    range: OplogRange,
    limit_bytes: usize,
    document_bytes: impl Fn(&OplogExportReport) -> anyhow::Result<usize>,
) -> anyhow::Result<OplogExportReport> {
    ctx.silence_app_context_init().await;
    let agent_handler = ctx.agent_handler();
    let agent_id_match = agent_handler.match_agent_id(agent_id).await?;
    let (component, agent_id) = agent_handler
        .component_by_agent_id_match(&agent_id_match)
        .await?;
    let options = OplogExportOptions {
        keep,
        max_entry_bytes,
        from: None,
        query: None,
        page_size: ctx.http_batch_size(),
        cli_version: crate::version().to_string(),
        generated_at: Timestamp::now().to_string(),
    };
    let reads = RestOplogReads {
        ctx: ctx.clone(),
        environment_id: component.environment_id,
        agent_id,
    };
    export_oplog_range(
        reads,
        TokenKey::process(),
        options,
        range,
        limit_bytes,
        document_bytes,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::read_failed;
    use crate::error::service::{ServiceError, ServiceErrorKind, ServiceErrorResponse};
    use crate::oplog_export::export::OplogExportError;
    use golem_common::base_model::api::error_code;
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

    #[test]
    fn read_failed_gives_an_expired_read_its_own_error_without_the_service_text() {
        let error = ServiceError {
            service_name: "Worker",
            kind: ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                status_code: 400,
                errors: vec!["The oplog of MARKER-7 expired: it starts at index 3".to_string()],
                code: Some(error_code::OPLOG_EXPIRED.to_string()),
                additional_fields: Default::default(),
            }),
        };
        let error = read_failed(anyhow::Error::from(error));
        assert!(matches!(
            error.downcast_ref::<OplogExportError>(),
            Some(OplogExportError::Expired)
        ));
        assert_eq!(
            format!("{error:#}"),
            "The oplog of the agent starts after the requested index. Export again from a later index"
        );
    }

    #[test]
    fn read_failed_gives_a_stale_cursor_its_own_error() {
        let error = ServiceError {
            service_name: "Worker",
            kind: ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                status_code: 400,
                errors: vec!["Stale oplog cursor of MARKER-7".to_string()],
                code: Some(error_code::STALE_CURSOR.to_string()),
                additional_fields: Default::default(),
            }),
        };
        assert!(matches!(
            read_failed(anyhow::Error::from(error)).downcast_ref::<OplogExportError>(),
            Some(OplogExportError::StaleCursor)
        ));
    }
}
