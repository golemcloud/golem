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

use crate::log::logln;
use crate::model::cli_output::StructuredOutput;
use crate::model::masking::Masked;
use crate::model::text_format::*;
use golem_client::model::{
    Invocation, InvocationOutcome, InvocationPage, InvocationState, InvocationSummary,
};
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationGetView(pub Invocation);

impl Masked for InvocationGetView {}

impl MessageWithFields for InvocationGetView {
    fn message(&self) -> String {
        format!(
            "Invocation {} is {}",
            format_message_highlight(&self.0.summary.invocation_id),
            format_message_highlight(&self.0.summary.state.to_string())
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = summary_fields(&self.0.summary);
        match &self.0.result {
            Some(InvocationOutcome::Method(method)) => fields.push((
                "Result".to_string(),
                pretty_json(
                    &serde_json::to_value(&method.value)
                        .ok()
                        .and_then(|mut value| value.get_mut("value").map(serde_json::Value::take)),
                ),
            )),
            Some(InvocationOutcome::Tool(tool)) => fields.push((
                "Result".to_string(),
                pretty_json(&serde_json::to_value(&tool.result).ok()),
            )),
            Some(InvocationOutcome::Failure(failure)) => {
                fields.push(("Error".to_string(), format_error(&failure.error)))
            }
            None => {}
        }
        fields
    }
}

impl StructuredOutput for InvocationGetView {
    const KIND: &'static str = "invocation.get";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationListView(pub InvocationPage);

impl StructuredOutput for InvocationListView {
    const KIND: &'static str = "invocation.list";
}

impl TextOutput for InvocationListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Invocation ID").fixed(),
            Column::new("State").fixed(),
            Column::new("Kind").fixed(),
            Column::new("Operation").fixed(),
            Column::new("Changed at").fixed(),
        ]);
        for invocation in &self.0.invocations {
            table.add_row(vec![
                invocation.invocation_id.clone(),
                invocation.state.to_string(),
                invocation
                    .kind
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                invocation.operation.clone().unwrap_or_default(),
                invocation.changed_at.to_rfc3339(),
            ]);
        }
        log_table(table);
        if let Some(cursor) = &self.0.next_cursor {
            logln("");
            logln(format!(
                "Next page: --cursor {}",
                format_message_highlight(cursor)
            ));
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationCancelView(pub InvocationSummary);

impl Masked for InvocationCancelView {}

impl MessageWithFields for InvocationCancelView {
    fn message(&self) -> String {
        let id = format_message_highlight(&self.0.invocation_id);
        match self.0.state {
            InvocationState::Cancelled => format!("Cancelled invocation {id}"),
            _ => format!(
                "Invocation {id} is {} and was not cancelled",
                format_message_highlight(&self.0.state.to_string())
            ),
        }
    }

    fn fields(&self) -> Vec<(String, String)> {
        summary_fields(&self.0)
    }
}

impl StructuredOutput for InvocationCancelView {
    const KIND: &'static str = "invocation.cancel";
}

fn summary_fields(summary: &InvocationSummary) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();
    fields
        .fmt_field("Invocation ID", &summary.invocation_id, format_main_id)
        .fmt_field("Agent", &summary.agent_id.agent_id, format_id)
        .fmt_field("Environment ID", &summary.environment_id, format_id)
        .fmt_field("State", &summary.state, ToString::to_string)
        .fmt_field_option("Kind", &summary.kind, ToString::to_string)
        .fmt_field_option("Operation", &summary.operation, ToString::to_string)
        .fmt_field_option("Accepted at", &summary.accepted_at, |at| at.to_rfc3339())
        .fmt_field("Changed at", &summary.changed_at, |at| at.to_rfc3339());
    fields.build()
}

fn pretty_json(value: &Option<serde_json::Value>) -> String {
    value
        .as_ref()
        .and_then(|value| serde_json::to_string_pretty(value).ok())
        .unwrap_or_default()
}
