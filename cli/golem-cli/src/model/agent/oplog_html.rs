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

use crate::model::agent::oplog::AgentOplogEntryView;
use serde::Serialize;

/// The self-contained viewer page. It carries no external assets, so the generated file works
/// from a `file://` URL. The data document is embedded as JSON in a `<script type="application/json">`
/// element that the viewer reads on load.
const VIEWER_TEMPLATE: &str = include_str!("oplog_viewer.html");
const DATA_PLACEHOLDER: &str = "__OPLOG_DATA__";
const TITLE_PLACEHOLDER: &str = "__OPLOG_TITLE__";

/// How the viewer page obtains its entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OplogViewerMode {
    /// Every entry is embedded in the page.
    Static,
    /// The page loads entries page by page from the local viewer server
    /// (see `crate::command_handler::agent::oplog_serve`).
    Live,
}

/// The document embedded into the HTML oplog viewer. `entries` has the same shape as the
/// `agent.oplog` structured output documents, so the viewer can also load a `--format json` dump.
/// In live mode `entries` is empty and `from` and `query` are the initial position and server
/// query of the viewer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OplogViewerDocument {
    pub mode: OplogViewerMode,
    pub agent_id: String,
    pub component_name: String,
    pub component_id: String,
    pub generated_at: String,
    pub cli_version: String,
    pub from: Option<u64>,
    pub query: Option<String>,
    /// Number of entries one page of the live API returns.
    pub page_size: u64,
    pub entries: Vec<AgentOplogEntryView>,
}

pub fn render_oplog_viewer_html(document: &OplogViewerDocument) -> anyhow::Result<String> {
    let json = serde_json::to_string(document)?;
    Ok(VIEWER_TEMPLATE
        .replacen(TITLE_PLACEHOLDER, &escape_html_text(&document.agent_id), 1)
        .replacen(DATA_PLACEHOLDER, &escape_json_for_script(&json), 1))
}

/// Escapes the sequences that would terminate a `<script>` element or open a comment, using JSON
/// string escapes so the embedded document stays valid JSON.
fn escape_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/").replace("<!--", "<\\u0021--")
}

fn escape_html_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::public_oplog_entry::{LogParams, SuspendParams};
    use golem_common::model::oplog::{LogLevel, PublicOplogEntry, PublicOplogEntryAttribution};
    use test_r::test;

    fn document(entries: Vec<AgentOplogEntryView>) -> OplogViewerDocument {
        OplogViewerDocument {
            mode: OplogViewerMode::Static,
            agent_id: "CounterAgent(\"<counter>\")".to_string(),
            component_name: "app:counter".to_string(),
            component_id: "00000000-0000-0000-0000-000000000000".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
            cli_version: "test".to_string(),
            from: None,
            query: None,
            page_size: 100,
            entries,
        }
    }

    fn entry(index: u64, entry: PublicOplogEntry) -> AgentOplogEntryView {
        AgentOplogEntryView {
            index,
            attribution: PublicOplogEntryAttribution::agent(),
            entry: entry.into(),
        }
    }

    #[test]
    fn viewer_embeds_entries_as_json_and_escapes_the_title() {
        let html = render_oplog_viewer_html(&document(vec![
            entry(
                1,
                PublicOplogEntry::Suspend(SuspendParams {
                    timestamp: Timestamp::from(0),
                }),
            ),
            entry(
                2,
                PublicOplogEntry::Log(LogParams {
                    timestamp: Timestamp::from(0),
                    level: LogLevel::Stdout,
                    context: String::new(),
                    message: "hello".to_string(),
                    trace_context: None,
                }),
            ),
        ]))
        .unwrap();

        assert!(
            html.contains("<title>Golem oplog · CounterAgent(&quot;&lt;counter&gt;&quot;)</title>")
        );
        assert!(!html.contains(DATA_PLACEHOLDER));
        assert!(!html.contains(TITLE_PLACEHOLDER));

        let start = html
            .find("<script id=\"oplog-data\" type=\"application/json\">")
            .unwrap();
        let data_start = start + "<script id=\"oplog-data\" type=\"application/json\">".len();
        let data_end = data_start + html[data_start..].find("</script>").unwrap();
        let embedded: serde_json::Value =
            serde_json::from_str(&html[data_start..data_end]).unwrap();
        assert_eq!(embedded["agentId"], "CounterAgent(\"<counter>\")");
        assert_eq!(embedded["entries"].as_array().unwrap().len(), 2);
        assert_eq!(embedded["entries"][1]["entry"]["type"], "Log");
        assert_eq!(embedded["entries"][1]["index"], 2);
    }

    #[test]
    fn script_terminators_in_payloads_do_not_break_the_page() {
        let html = render_oplog_viewer_html(&document(vec![entry(
            1,
            PublicOplogEntry::Log(LogParams {
                timestamp: Timestamp::from(0),
                level: LogLevel::Stderr,
                context: String::new(),
                message: "</script><!-- <script>alert(1)</script>".to_string(),
                trace_context: None,
            }),
        )]))
        .unwrap();

        let data_start = html.find("<script id=\"oplog-data\"").unwrap();
        let data_end = data_start + html[data_start..].find("</script>").unwrap();
        let data = &html[data_start..data_end];
        assert!(data.contains("<\\/script>"));
        assert!(data.contains("<\\u0021--"));
        assert!(!data.contains("</script>"));
        assert!(!data.contains("<!--"));
    }
}
