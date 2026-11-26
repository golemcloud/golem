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

//! `agent_oplog_export`: the typed MCP tool that returns one bounded obfuscated oplog report.
//!
//! No CLI command has this tool's arguments or result. The tool reads one closed range of the
//! oplog within one scan allowance of the server, and returns the whole report as one
//! `agent.oplog-export` document, or fails.

use super::{ResultShape, as_object, output_schema};
use crate::command_handler::agent::oplog_export::oplog_export_report;
use crate::command_handler::log::render_command_output_document_masked;
use crate::context::Context;
use crate::log::OUTPUT_CAPTURE_LIMIT;
use crate::model::agent::RawAgentId;
use crate::model::agent::oplog_export_view::AgentOplogExportReportView;
use crate::model::cli_output::{StructuredOutput, focused_command_output_schema};
use crate::model::format::Format;
use crate::model::masking::MaskingConfig;
use crate::oplog_export::export::{DEFAULT_MAX_ENTRY_BYTES, OplogExportError, OplogRange};
use crate::oplog_export::model::OplogExportReport;
use crate::oplog_export::obfuscator::KeepFlags;
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub const OPLOG_EXPORT_TOOL: &str = "agent_oplog_export";

/// The instruction of an export that is too large or that its scan budget does not cover.
pub const NARROW_THE_RANGE: &str =
    "Narrow `from`/`to`, or run `golem agent oplog --obfuscate` in a terminal.";

/// The arguments of one call. `null` counts as absent, as for the other tools.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OplogExportArguments {
    pub agent_id: String,
    pub from: u64,
    pub to: u64,
    #[serde(default)]
    pub keep_scalars: Option<bool>,
    #[serde(default)]
    pub keep_shape: Option<bool>,
    #[serde(default)]
    pub keep_error_text: Option<bool>,
    #[serde(default)]
    pub max_entry_bytes: Option<u64>,
}

impl OplogExportArguments {
    /// Parses the arguments of a call. The error is one fixed text: the error of the parser
    /// quotes the values that it refuses, and a value can be customer data.
    pub fn parse(arguments: &JsonObject) -> Result<Self, String> {
        serde_json::from_value(Value::Object(arguments.clone())).map_err(|_| {
            format!(
                "`{OPLOG_EXPORT_TOOL}` takes `agent_id` (a string), `from` and `to` (integers), \
                 `keep_scalars`, `keep_shape` and `keep_error_text` (booleans) and \
                 `max_entry_bytes` (an integer), and no other argument. `agent_id`, `from` and \
                 `to` are required."
            )
        })
    }
}

pub fn tool() -> anyhow::Result<Tool> {
    let input_schema = json!({
        "type": "object",
        "properties": {
            "agent_id": {
                "type": "string",
                "description": "The agent, in the form that `agent_oplog` takes.",
            },
            "from": {
                "type": "integer",
                "minimum": 1,
                "description": "The first oplog index of the range, inclusive. Use `agent_oplog` to find the indexes.",
            },
            "to": {
                "type": "integer",
                "minimum": 1,
                "description": "The last oplog index of the range, inclusive. It must be at least `from` and at most the last index of the oplog.",
            },
            "keep_scalars": {
                "type": "boolean",
                "description": "Show numbers, bools, datetimes, durations and the mantissa, scale and unit of quantities in clear. Never byte lists, discriminants, lengths or text.",
            },
            "keep_shape": {
                "type": "boolean",
                "description": "Show discriminants, presence, and list, map and byte lengths.",
            },
            "keep_error_text": {
                "type": "boolean",
                "description": "Keep the free text of error messages. It can contain any customer value.",
            },
            "max_entry_bytes": {
                "type": "integer",
                "minimum": 0,
                "default": DEFAULT_MAX_ENTRY_BYTES,
                "description": "Read an entry that the oplog page omitted only when its size is at most this many bytes. A larger entry stays omitted with its size.",
            },
        },
        "required": ["agent_id", "from", "to"],
        "additionalProperties": false,
    });
    let output_schema = output_schema(
        ResultShape::Document,
        focused_command_output_schema(&[AgentOplogExportReportView::KIND.to_string()])?,
    );
    let description = format!(
        "Returns one complete obfuscated report of the oplog entries from `from` to `to`, \
         inclusive: customer values become tokens. Tokens are equal only inside one report; \
         treat each report as independent. The report is for a bug report, not for replay.\n\n\
         The reads of the range share one scan budget of the server. When the budget runs out \
         before the end of the range, the call fails with `scan-budget-exceeded`. When the \
         report is larger than {} MiB, the call fails with `output-too-large`. Neither failure \
         returns a part of the report. {NARROW_THE_RANGE}",
        OUTPUT_CAPTURE_LIMIT / (1024 * 1024)
    );
    Ok(Tool::new_with_raw(
        OPLOG_EXPORT_TOOL,
        Some(description.into()),
        Arc::new(as_object(input_schema)),
    )
    .with_raw_output_schema(Arc::new(as_object(output_schema)))
    .with_title("Obfuscated oplog report")
    .with_annotations(ToolAnnotations::new().read_only(true).destructive(false)))
}

/// Builds the report and returns it as one compact JSON document, the form that the command
/// output prints. A document larger than `limit_bytes` with its newline fails with
/// [`OplogExportError::OutputTooLarge`].
pub async fn run(
    ctx: Arc<Context>,
    arguments: OplogExportArguments,
    limit_bytes: usize,
) -> anyhow::Result<String> {
    let keep = KeepFlags {
        scalars: arguments.keep_scalars.unwrap_or_default(),
        shape: arguments.keep_shape.unwrap_or_default(),
        error_text: arguments.keep_error_text.unwrap_or_default(),
    };
    let report = oplog_export_report(
        ctx,
        RawAgentId(arguments.agent_id),
        keep,
        arguments.max_entry_bytes.unwrap_or(DEFAULT_MAX_ENTRY_BYTES),
        OplogRange {
            from: arguments.from,
            to: arguments.to,
        },
        limit_bytes,
        |report| Ok(document(report.clone())?.len() + 1),
    )
    .await?;
    fitted_document(report, limit_bytes)
}

/// The compact JSON document of `report`, as the command output prints it, without its newline.
fn document(report: OplogExportReport) -> anyhow::Result<String> {
    render_command_output_document_masked(
        Format::Json,
        false,
        MaskingConfig::hide_secrets(),
        AgentOplogExportReportView(report),
    )
}

/// The compact JSON document of `report`, as the command output prints it, without its newline.
/// Fails with [`OplogExportError::OutputTooLarge`] when the document and its newline take more
/// than `limit_bytes` bytes.
pub fn fitted_document(report: OplogExportReport, limit_bytes: usize) -> anyhow::Result<String> {
    let matched_entries = report.entries.len() as u64;
    let document = document(report)?;
    if document.len() + 1 > limit_bytes {
        return Err(OplogExportError::OutputTooLarge {
            limit_bytes,
            matched_entries: Some(matched_entries),
        }
        .into());
    }
    Ok(document)
}

/// The error code, message and details of a failed export, for the errors that only an export
/// has; `None` for the other errors.
pub fn export_failure(error: &anyhow::Error) -> Option<(&'static str, String, Map<String, Value>)> {
    let export_error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<OplogExportError>())?;
    let message = export_error.to_string();
    let (code, details) = match export_error {
        OplogExportError::ScanBudgetExceeded { .. } => ("scan-budget-exceeded", Map::new()),
        OplogExportError::OutputTooLarge {
            limit_bytes,
            matched_entries,
        } => {
            let mut details = Map::from_iter([("limit_bytes".to_string(), json!(limit_bytes))]);
            if let Some(matched_entries) = matched_entries {
                details.insert("matched_entries".to_string(), json!(matched_entries));
            }
            ("output-too-large", details)
        }
        OplogExportError::RangeInvalid { .. } | OplogExportError::RangePastEnd { .. } => {
            ("invalid-arguments", Map::new())
        }
        OplogExportError::StaleCursor => ("stale-cursor", Map::new()),
        OplogExportError::Expired => ("oplog-expired", Map::new()),
        OplogExportError::RootChanged
        | OplogExportError::CreateTooLarge { .. }
        | OplogExportError::CreateMissing
        | OplogExportError::ReadFailed { .. }
        | OplogExportError::ReadFailedWithoutResponse
        | OplogExportError::RangePageInvalid => ("command-failed", Map::new()),
    };
    Some((code, message, details))
}
