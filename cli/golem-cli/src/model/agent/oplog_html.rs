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
use crate::oplog_export::model::{OplogExportCoverage, OplogExportHeader, ReportEntry};
use anyhow::{Context, anyhow};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    /// Every entry of an obfuscated export is embedded in the page.
    Export,
    /// The page loads obfuscated pages from the local viewer server.
    LiveExport,
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
    render_template(&document.agent_id, &serde_json::to_string(document)?)
}

/// The document of the served viewer of an obfuscated export. It holds no agent ID and no
/// component; the page shows the root reference of the header.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OplogLiveExportViewerDocument {
    pub mode: OplogViewerMode,
    pub header: OplogExportHeader,
    pub from: Option<u64>,
    pub query: Option<String>,
    pub page_size: u64,
}

pub fn render_oplog_live_export_viewer_html(
    document: &OplogLiveExportViewerDocument,
) -> anyhow::Result<String> {
    render_template(
        &document.header.root.pseudonym(),
        &serde_json::to_string(document)?,
    )
}

fn render_template(title: &str, json: &str) -> anyhow::Result<String> {
    Ok(VIEWER_TEMPLATE
        .replacen(TITLE_PLACEHOLDER, &escape_html_text(title), 1)
        .replacen(DATA_PLACEHOLDER, &escape_json_for_script(json), 1))
}

/// Writes the viewer page of an obfuscated export entry by entry into a temporary file next to
/// the target, and replaces the target only in [`OplogExportHtmlWriter::finish`]. A writer that is
/// dropped before `finish` deletes its temporary file and leaves the target unchanged.
pub struct OplogExportHtmlWriter {
    path: PathBuf,
    parent: PathBuf,
    file: tempfile::NamedTempFile,
    suffix: &'static str,
    entries: usize,
}

impl OplogExportHtmlWriter {
    pub fn create(path: &Path, title: &str) -> anyhow::Result<Self> {
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
                parent.to_path_buf()
            }
            _ => PathBuf::from("."),
        };
        let mut file = tempfile::NamedTempFile::new_in(&parent).with_context(|| {
            format!("failed to create a temporary file in {}", parent.display())
        })?;
        let template = VIEWER_TEMPLATE.replacen(TITLE_PLACEHOLDER, &escape_html_text(title), 1);
        let split = template
            .find(DATA_PLACEHOLDER)
            .ok_or_else(|| anyhow!("the viewer template has no data placeholder"))?;
        let suffix_start = VIEWER_TEMPLATE
            .find(DATA_PLACEHOLDER)
            .ok_or_else(|| anyhow!("the viewer template has no data placeholder"))?
            + DATA_PLACEHOLDER.len();
        file.write_all(&template.as_bytes()[..split])?;
        Ok(Self {
            path: path.to_path_buf(),
            parent,
            file,
            suffix: &VIEWER_TEMPLATE[suffix_start..],
            entries: 0,
        })
    }

    fn write_json(&mut self, value: &impl Serialize) -> anyhow::Result<()> {
        let json = serde_json::to_string(value)?;
        self.file
            .write_all(escape_json_for_script(&json).as_bytes())?;
        Ok(())
    }

    pub fn header(&mut self, header: &OplogExportHeader) -> anyhow::Result<()> {
        self.file.write_all(br#"{"mode":"export","header":"#)?;
        self.write_json(header)?;
        self.file.write_all(br#","entries":["#)?;
        Ok(())
    }

    pub fn entry(&mut self, entry: &ReportEntry) -> anyhow::Result<()> {
        if self.entries > 0 {
            self.file.write_all(b",")?;
        }
        self.write_json(entry)?;
        self.entries += 1;
        Ok(())
    }

    /// Writes the coverage and the rest of the page, then replaces the target file.
    pub fn finish(mut self, coverage: &OplogExportCoverage) -> anyhow::Result<()> {
        self.file.write_all(br#"],"coverage":"#)?;
        self.write_json(coverage)?;
        self.file.write_all(b"}")?;
        self.file.write_all(self.suffix.as_bytes())?;
        self.file.as_file_mut().sync_all()?;
        self.file
            .persist(&self.path)
            .map_err(|error| anyhow!("failed to write {}: {}", self.path.display(), error.error))?;
        std::fs::File::open(&self.parent)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("failed to sync {}", self.parent.display()))?;
        Ok(())
    }
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

    fn export_header() -> OplogExportHeader {
        OplogExportHeader {
            obfuscate: true,
            keep_scalars: false,
            keep_shape: false,
            keep_error_text: false,
            filtered: false,
            root: crate::oplog_export::model::ReportAgentRef {
                token: crate::oplog_export::model::ReportToken(format!(
                    "agent:{}",
                    "ab".repeat(16)
                )),
            },
            cli_version: "test".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn export_entry(index: u64, name: &str) -> ReportEntry {
        use crate::oplog_export::model::{RecordNode, ReportField, ReportNode, TextNode};
        ReportEntry {
            index,
            timestamp: Timestamp::from(0),
            kind: "Start".to_string(),
            attribution: None,
            body: Some(ReportNode::Record(RecordNode {
                type_name: None,
                role: None,
                fields: vec![ReportField {
                    name: Some("functionName".to_string()),
                    value: ReportNode::Text(TextNode {
                        value: name.to_string(),
                    }),
                }],
            })),
            omitted: None,
        }
    }

    fn embedded(html: &str) -> serde_json::Value {
        let open = "<script id=\"oplog-data\" type=\"application/json\">";
        let data_start = html.find(open).unwrap() + open.len();
        let data_end = data_start + html[data_start..].find("</script>").unwrap();
        serde_json::from_str(&html[data_start..data_end]).unwrap()
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    fn write_export(path: &Path, entries: &[ReportEntry]) {
        let header = export_header();
        let mut writer = OplogExportHtmlWriter::create(path, &header.root.pseudonym()).unwrap();
        writer.header(&header).unwrap();
        for entry in entries {
            writer.entry(entry).unwrap();
        }
        writer
            .finish(&OplogExportCoverage::new(vec!["a note".to_string()]))
            .unwrap();
    }

    #[test]
    fn export_html_writer_embeds_only_report_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("report.html");
        let entries = vec![export_entry(1, "one"), export_entry(2, "two")];
        write_export(&path, &entries);
        let html = std::fs::read_to_string(&path).unwrap();
        let header = export_header();
        assert!(html.contains(&format!(
            "<title>Golem oplog · {}</title>",
            header.root.pseudonym()
        )));
        let doc = embedded(&html);
        assert_eq!(doc["mode"], "export");
        assert_eq!(doc["header"], serde_json::to_value(&header).unwrap());
        assert_eq!(doc["entries"], serde_json::to_value(&entries).unwrap());
        assert_eq!(doc["coverage"]["notes"][0], "a note");
        for key in ["agentId", "componentName", "componentId", "query"] {
            assert!(doc.get(key).is_none(), "{key}");
        }
        assert!(!html.contains(DATA_PLACEHOLDER));
        assert!(!html.contains(TITLE_PLACEHOLDER));
        assert_eq!(files_in(path.parent().unwrap()), vec!["report.html"]);
    }

    #[test]
    fn export_html_writer_writes_an_empty_entry_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.html");
        write_export(&path, &[]);
        let doc = embedded(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(doc["entries"], serde_json::json!([]));
    }

    #[test]
    fn export_html_writer_escapes_script_terminators() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.html");
        let entries = vec![export_entry(1, "</script><!-- <b>")];
        write_export(&path, &entries);
        let html = std::fs::read_to_string(&path).unwrap();
        let doc = embedded(&html);
        assert_eq!(
            doc["entries"][0]["body"]["fields"][0]["value"]["value"],
            "</script><!-- <b>"
        );
        assert!(!html.contains("</script><!--"));
    }

    #[test]
    fn export_html_writer_replaces_the_file_only_on_finish() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.html");
        std::fs::write(&path, "OLD").unwrap();
        let header = export_header();
        let mut writer = OplogExportHtmlWriter::create(&path, "t").unwrap();
        writer.header(&header).unwrap();
        writer.entry(&export_entry(1, "one")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "OLD");
        writer.finish(&OplogExportCoverage::new(vec![])).unwrap();
        let doc = embedded(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(doc["entries"][0]["index"], 1);
        assert_eq!(files_in(dir.path()), vec!["report.html"]);
    }

    #[test]
    fn dropped_export_html_writer_keeps_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.html");
        std::fs::write(&path, b"OLD\x00BYTES").unwrap();
        {
            let mut writer = OplogExportHtmlWriter::create(&path, "t").unwrap();
            writer.header(&export_header()).unwrap();
            writer.entry(&export_entry(1, "one")).unwrap();
            writer.entry(&export_entry(2, "two")).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"OLD\x00BYTES");
        assert_eq!(files_in(dir.path()), vec!["report.html"]);
    }

    #[test]
    fn live_export_document_holds_no_identity() {
        let header = export_header();
        let html = render_oplog_live_export_viewer_html(&OplogLiveExportViewerDocument {
            mode: OplogViewerMode::LiveExport,
            header: header.clone(),
            from: Some(3),
            query: None,
            page_size: 50,
        })
        .unwrap();
        let doc = embedded(&html);
        assert_eq!(doc["mode"], "liveexport");
        assert_eq!(doc["header"], serde_json::to_value(&header).unwrap());
        assert_eq!(doc["pageSize"], 50);
        for key in ["agentId", "componentName", "componentId", "entries"] {
            assert!(doc.get(key).is_none(), "{key}");
        }
        assert!(html.contains(&format!(
            "<title>Golem oplog · {}</title>",
            header.root.pseudonym()
        )));
    }
}
