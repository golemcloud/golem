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

//! The output documents of the obfuscated oplog export and their text form.

use crate::log::logln;
use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::*;
use crate::oplog_export::model::{
    CaseNode, CoverageList, KnownRole, ListNode, MapNode, OplogExportCoverage, OplogExportHeader,
    QuantityNode, RecordNode, ReportEntry, ReportNode, ScalarNode, StrNode, TypeDescription,
};
use serde::{Deserialize, Serialize};

/// The first document of an export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentOplogExportHeaderView(pub OplogExportHeader);

/// One entry of an export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentOplogExportEntryView(pub ReportEntry);

/// The last document of an export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentOplogExportCoverageView(pub OplogExportCoverage);

impl StructuredOutput for AgentOplogExportHeaderView {
    const KIND: &'static str = "agent.oplog-export-header";
}

impl StructuredOutput for AgentOplogExportEntryView {
    const KIND: &'static str = "agent.oplog-export-entry";
}

impl StructuredOutput for AgentOplogExportCoverageView {
    const KIND: &'static str = "agent.oplog-export-coverage";
}

const PAD: &str = "          ";

impl TextOutput for AgentOplogExportHeaderView {
    fn log(&self) {
        for line in header_lines(&self.0) {
            logln(line);
        }
    }
}

impl TextOutput for AgentOplogExportEntryView {
    fn log(&self) {
        for line in entry_lines(&self.0) {
            logln(line);
        }
    }
}

impl TextOutput for AgentOplogExportCoverageView {
    fn log(&self) {
        for line in coverage_lines(&self.0) {
            logln(line);
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn header_lines(header: &OplogExportHeader) -> Vec<String> {
    let OplogExportHeader {
        obfuscate,
        keep_scalars,
        keep_shape,
        keep_error_text,
        filtered,
        root,
        cli_version,
        generated_at,
    } = header;
    let mut keep = vec![];
    if *keep_scalars {
        keep.push("scalars");
    }
    if *keep_shape {
        keep.push("shape");
    }
    if *keep_error_text {
        keep.push("error text");
    }
    let keep = if keep.is_empty() {
        "none".to_string()
    } else {
        keep.join(", ")
    };
    vec![
        format_message_highlight("OPLOG EXPORT"),
        format!(
            "{PAD}root:              {}",
            format_main_id(&root.pseudonym())
        ),
        format!("{PAD}obfuscated:        {}", yes_no(*obfuscate)),
        format!("{PAD}kept:              {keep}"),
        format!("{PAD}filtered:          {}", yes_no(*filtered)),
        format!("{PAD}cli version:       {cli_version}"),
        format!("{PAD}generated at:      {generated_at}"),
    ]
}

fn entry_lines(entry: &ReportEntry) -> Vec<String> {
    let ReportEntry {
        index,
        timestamp,
        kind,
        attribution,
        body,
        omitted,
    } = entry;
    let mut lines = vec![format!("{}: ", format_main_id(&format!("#{index:0>5}")))];
    match omitted {
        Some(omission) => {
            lines.push(format!(
                "{PAD}{}",
                format_message_highlight("OMITTED ENTRY")
            ));
            lines.push(format!("{PAD}at:                {}", format_id(timestamp)));
            lines.push(format!("{PAD}entry type:        {}", format_id(kind)));
            lines.push(format!("{PAD}size:              {} bytes", omission.size));
            lines.push(format!(
                "{PAD}reason:            {}",
                serde_text(&omission.reason)
            ));
        }
        None => {
            lines.push(format!("{PAD}{}", format_message_highlight(kind)));
            lines.push(format!("{PAD}at:                {}", format_id(timestamp)));
        }
    }
    match attribution {
        None => lines.push(format!("{PAD}owner:             agent")),
        Some(node) => push_node(&mut lines, PAD, Some("owner"), node),
    }
    match body {
        Some(ReportNode::Record(RecordNode {
            type_name: None,
            role: None,
            fields,
        })) => {
            for (i, field) in fields.iter().enumerate() {
                push_node(
                    &mut lines,
                    PAD,
                    Some(&field_label(field.name.as_deref(), i)),
                    &field.value,
                );
            }
        }
        Some(node) => push_node(&mut lines, PAD, Some("body"), node),
        None => {}
    }
    lines
}

fn coverage_lines(coverage: &OplogExportCoverage) -> Vec<String> {
    let OplogExportCoverage {
        incomplete,
        hydration_failures,
        start_unknown,
        schema_less_rpc_values,
        notes,
    } = coverage;
    let mut lines = vec![format_message_highlight("COVERAGE")];
    push_list(&mut lines, "omitted entries", hydration_failures, |item| {
        format!(
            "#{:0>5} {} ({} bytes, {})",
            item.index,
            item.kind,
            item.size,
            serde_text(&item.reason)
        )
    });
    push_list(&mut lines, "unknown Start", start_unknown, |item| {
        format!("#{:0>5} (Start #{:0>5})", item.index, item.start_index)
    });
    push_list(
        &mut lines,
        "schema-less RPC values",
        schema_less_rpc_values,
        |item| format!("#{:0>5} {}", item.index, item.path),
    );
    for note in notes {
        lines.push(format!("{PAD}note: {note}"));
    }
    if *incomplete {
        lines.push(format!(
            "{PAD}{}",
            format_warn("Some entries are not in the report in full.")
        ));
    }
    lines
}

fn push_list<T>(
    lines: &mut Vec<String>,
    title: &str,
    list: &CoverageList<T>,
    item: impl Fn(&T) -> String,
) {
    lines.push(format!("{PAD}{title}: {} discovered", list.discovered));
    for shown in &list.shown {
        lines.push(format!("{PAD}  {}", item(shown)));
    }
    let hidden = list.discovered.saturating_sub(list.shown.len() as u64);
    if hidden > 0 {
        lines.push(format!("{PAD}  … {hidden} more"));
    }
}

/// The serde name of a unit enum value, such as `tooLarge`.
fn serde_text(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

fn field_label(name: Option<&str>, index: usize) -> String {
    name.map(str::to_string)
        .unwrap_or_else(|| format!("#{index}"))
}

/// Renders a report node as text lines. It prints only what the node holds: a node without a
/// kept value prints its token.
pub fn render_report_node(pad: &str, node: &ReportNode) -> Vec<String> {
    let mut lines = vec![];
    push_node(&mut lines, pad, None, node);
    lines
}

fn push_node(lines: &mut Vec<String>, pad: &str, label: Option<&str>, node: &ReportNode) {
    let prefix = label.map(|l| format!("{l}: ")).unwrap_or_default();
    let inner = format!("{pad}  ");
    let mut head = |text: String| lines.push(format!("{pad}{prefix}{text}"));
    match node {
        ReportNode::Record(RecordNode {
            type_name,
            role,
            fields,
        }) => {
            head(format!("record{}", annotations(type_name, role)));
            for (i, field) in fields.iter().enumerate() {
                push_node(
                    lines,
                    &inner,
                    Some(&field_label(field.name.as_deref(), i)),
                    &field.value,
                );
            }
        }
        ReportNode::Tuple(tuple) => {
            head("tuple".to_string());
            for (i, element) in tuple.elements.iter().enumerate() {
                push_node(lines, &inner, Some(&format!("#{i}")), element);
            }
        }
        ReportNode::Case(CaseNode {
            case_kind,
            type_name,
            role,
            case,
            case_index,
            payload,
            tag_token,
            token,
        }) => {
            let mut text = format!("{}{}", serde_text(case_kind), annotations(type_name, role));
            if let Some(case) = case {
                text.push_str(&format!(" {case}"));
            }
            if let Some(case_index) = case_index {
                text.push_str(&format!(" (case {case_index})"));
            }
            if let Some(tag_token) = tag_token {
                text.push_str(&format!(" tag {tag_token}"));
            }
            if let Some(token) = token {
                text.push_str(&format!(" {token}"));
            }
            head(text);
            if let Some(payload) = payload {
                push_node(lines, &inner, Some("payload"), payload);
            }
        }
        ReportNode::List(ListNode {
            length,
            token,
            elements,
        }) => {
            head(format!("list{} {token}", length_text(length)));
            for (i, element) in elements.iter().flatten().enumerate() {
                push_node(lines, &inner, Some(&format!("#{i}")), element);
            }
        }
        ReportNode::Map(MapNode {
            length,
            token,
            entries,
        }) => {
            head(format!("map{} {token}", length_text(length)));
            for (i, entry) in entries.iter().flatten().enumerate() {
                push_node(lines, &inner, Some(&format!("key #{i}")), &entry.key);
                push_node(lines, &inner, Some(&format!("value #{i}")), &entry.value);
            }
        }
        ReportNode::Bytes(bytes) => {
            head(format!(
                "bytes{} {}",
                length_text(&bytes.length),
                bytes.token
            ));
        }
        ReportNode::Scalar(ScalarNode {
            scalar_kind,
            value,
            token,
        }) => {
            let shown = match value {
                Some(value) => value.to_string(),
                None => token.to_string(),
            };
            head(format!("{} {shown}", serde_text(scalar_kind)));
        }
        ReportNode::Quantity(QuantityNode {
            mantissa,
            scale,
            unit,
            unit_token,
            token,
        }) => {
            let mut text = "quantity".to_string();
            if let Some(mantissa) = mantissa {
                text.push_str(&format!(" mantissa {mantissa}"));
            }
            if let Some(scale) = scale {
                text.push_str(&format!(" scale {scale}"));
            }
            if let Some(unit) = unit {
                text.push_str(&format!(" unit {unit}"));
            }
            if let Some(unit_token) = unit_token {
                text.push_str(&format!(" unit {unit_token}"));
            }
            if mantissa.is_none() && scale.is_none() {
                text.push_str(&format!(" {token}"));
            }
            head(text);
        }
        ReportNode::Str(StrNode { token, value }) => match (value, token) {
            (Some(value), _) => head(format!("{value:?}")),
            (None, Some(token)) => head(token.to_string()),
            (None, None) => head("str".to_string()),
        },
        ReportNode::Agent(agent) => head(format!("agent {}", agent.token)),
        ReportNode::Redacted(redacted) => {
            head(format!("redacted ({})", serde_text(&redacted.reason)))
        }
        ReportNode::TypeDescription(description) => {
            head(format!("type {}", type_text(&description.description)))
        }
        ReportNode::Text(text) => head(text.value.clone()),
        ReportNode::Number(number) => head(number.value.to_string()),
        ReportNode::Timestamp(timestamp) => head(timestamp.value.to_string()),
        ReportNode::Flag(flag) => head(flag.value.to_string()),
    }
}

fn annotations(type_name: &Option<String>, role: &Option<KnownRole>) -> String {
    let mut text = String::new();
    if let Some(type_name) = type_name {
        text.push_str(&format!(" {type_name}"));
    }
    if let Some(role) = role {
        text.push_str(&format!(" ({})", serde_text(role)));
    }
    text
}

fn length_text(length: &Option<u64>) -> String {
    match length {
        Some(length) => format!(" (length {length})"),
        None => String::new(),
    }
}

/// A one-line text of a type description, such as `record Point { x: s32, y: s32 }`.
fn type_text(description: &TypeDescription) -> String {
    let TypeDescription {
        kind,
        type_name,
        role,
        members,
        elements,
        names,
        base_unit,
        allowed_suffixes,
        definitions,
    } = description;
    let mut text = serde_text(kind);
    text.push_str(&annotations(type_name, role));
    if !members.is_empty() {
        let members: Vec<String> = members
            .iter()
            .map(|m| match &m.body {
                Some(body) => format!("{}: {}", m.name, type_text(body)),
                None => m.name.clone(),
            })
            .collect();
        text.push_str(&format!(" {{ {} }}", members.join(", ")));
    }
    if !elements.is_empty() {
        let elements: Vec<String> = elements.iter().map(type_text).collect();
        text.push_str(&format!("<{}>", elements.join(", ")));
    }
    if !names.is_empty() {
        text.push_str(&format!(" [{}]", names.join(", ")));
    }
    if let Some(base_unit) = base_unit {
        text.push_str(&format!(" unit {base_unit}"));
    }
    if !allowed_suffixes.is_empty() {
        text.push_str(&format!(" suffixes [{}]", allowed_suffixes.join(", ")));
    }
    if !definitions.is_empty() {
        let definitions: Vec<String> = definitions
            .iter()
            .map(|d| match &d.body {
                Some(body) => format!("{} = {}", d.name, type_text(body)),
                None => d.name.clone(),
            })
            .collect();
        text.push_str(&format!(" where {}", definitions.join("; ")));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oplog_export::model::{
        BytesNode, CaseKind, FlagNode, MapEntryNode, NumberNode, OmittedEntryRef, OmittedReason,
        RedactedNode, RedactionReason, ReportAgentRef, ReportField, ReportOmission, ReportToken,
        ScalarKind, SchemaLessRpcValueRef, StartUnknownRef, TextNode, TimestampNode, TupleNode,
        TypeDescriptionNode, TypeKind,
    };
    use golem_common::model::Timestamp;
    use test_r::test;

    fn token(domain: &str, n: u8) -> ReportToken {
        ReportToken(format!("{domain}:{}", format!("{n:02x}").repeat(16)))
    }

    fn text(node: &ReportNode) -> String {
        render_report_node("", node).join("\n")
    }

    #[test]
    fn report_node_renderer_prints_only_what_the_node_holds() {
        let list = ReportNode::List(ListNode {
            length: None,
            token: token("list", 1),
            elements: None,
        });
        assert_eq!(text(&list), format!("list {}", token("list", 1)));
        let list = ReportNode::List(ListNode {
            length: Some(3),
            token: token("list", 1),
            elements: Some(vec![ReportNode::Number(NumberNode { value: 42 })]),
        });
        assert_eq!(
            text(&list),
            format!("list (length 3) {}\n  #0: 42", token("list", 1))
        );

        let bytes = ReportNode::Bytes(BytesNode {
            length: None,
            token: token("bytes", 2),
        });
        assert!(!text(&bytes).contains("length"));
        let map = ReportNode::Map(MapNode {
            length: Some(1),
            token: token("map", 3),
            entries: Some(vec![MapEntryNode {
                key: ReportNode::Text(TextNode { value: "k".into() }),
                value: ReportNode::Flag(FlagNode { value: true }),
            }]),
        });
        assert_eq!(
            text(&map),
            format!(
                "map (length 1) {}\n  key #0: k\n  value #0: true",
                token("map", 3)
            )
        );

        let hidden_case = ReportNode::Case(CaseNode {
            case_kind: CaseKind::Variant,
            type_name: None,
            role: None,
            case: None,
            case_index: None,
            payload: None,
            tag_token: None,
            token: Some(token("variant", 4)),
        });
        assert_eq!(
            text(&hidden_case),
            format!("variant {}", token("variant", 4))
        );
        let union = ReportNode::Case(CaseNode {
            case_kind: CaseKind::Union,
            type_name: None,
            role: None,
            case: None,
            case_index: None,
            payload: Some(Box::new(ReportNode::Str(StrNode {
                token: Some(token("str", 5)),
                value: None,
            }))),
            tag_token: Some(token("str", 6)),
            token: None,
        });
        assert_eq!(
            text(&union),
            format!(
                "union tag {}\n  payload: {}",
                token("str", 6),
                token("str", 5)
            )
        );
        let kept_case = ReportNode::Case(CaseNode {
            case_kind: CaseKind::Enum,
            type_name: Some("Color".into()),
            role: None,
            case: Some("red".into()),
            case_index: Some(0),
            payload: None,
            tag_token: None,
            token: None,
        });
        assert_eq!(text(&kept_case), "enum Color red (case 0)");

        let scalar = |value| {
            ReportNode::Scalar(ScalarNode {
                scalar_kind: ScalarKind::Int,
                value,
                token: token("int", 7),
            })
        };
        assert_eq!(text(&scalar(None)), format!("int {}", token("int", 7)));
        assert_eq!(text(&scalar(Some(serde_json::json!(5)))), "int 5");

        let quantity = |mantissa, scale| {
            ReportNode::Quantity(QuantityNode {
                mantissa,
                scale,
                unit: None,
                unit_token: None,
                token: token("quantity", 8),
            })
        };
        assert_eq!(
            text(&quantity(None, None)),
            format!("quantity {}", token("quantity", 8))
        );
        assert_eq!(
            text(&quantity(Some(15), Some(1))),
            "quantity mantissa 15 scale 1"
        );

        let str_value = ReportNode::Str(StrNode {
            token: None,
            value: Some("boom".into()),
        });
        assert_eq!(text(&str_value), "\"boom\"");
        let agent = ReportNode::Agent(ReportAgentRef {
            token: token("agent", 9),
        });
        assert_eq!(text(&agent), format!("agent {}", token("agent", 9)));
        let redacted = ReportNode::Redacted(RedactedNode {
            reason: RedactionReason::QuotaToken,
        });
        assert_eq!(text(&redacted), "redacted (quotaToken)");
        let description = ReportNode::TypeDescription(TypeDescriptionNode {
            description: TypeDescription {
                kind: TypeKind::Record,
                type_name: Some("Point".into()),
                role: None,
                members: vec![crate::oplog_export::model::NamedTypeDescription {
                    name: "x".into(),
                    body: Some(TypeDescription {
                        kind: TypeKind::S32,
                        type_name: None,
                        role: None,
                        members: vec![],
                        elements: vec![],
                        names: vec![],
                        base_unit: None,
                        allowed_suffixes: vec![],
                        definitions: vec![],
                    }),
                }],
                elements: vec![],
                names: vec![],
                base_unit: None,
                allowed_suffixes: vec![],
                definitions: vec![],
            },
        });
        assert_eq!(text(&description), "type record Point { x: s32 }");
        let record = ReportNode::Record(RecordNode {
            type_name: None,
            role: Some(KnownRole::Multimodal),
            fields: vec![
                ReportField {
                    name: Some("name".into()),
                    value: ReportNode::Text(TextNode {
                        value: "B3-NAME".into(),
                    }),
                },
                ReportField {
                    name: None,
                    value: ReportNode::Tuple(TupleNode {
                        elements: vec![ReportNode::Timestamp(TimestampNode {
                            value: Timestamp::from(0),
                        })],
                    }),
                },
            ],
        });
        let record_text = text(&record);
        assert!(
            record_text.starts_with("record (multimodal)\n  name: B3-NAME\n  #1: tuple\n    #0: ")
        );
    }

    fn entry() -> ReportEntry {
        ReportEntry {
            index: 7,
            timestamp: Timestamp::from(0),
            kind: "Log".into(),
            attribution: None,
            body: None,
            omitted: None,
        }
    }

    #[test]
    fn omitted_entry_renders_kind_size_and_reason() {
        for (reason, reason_text) in [
            (OmittedReason::TooLarge, "tooLarge"),
            (OmittedReason::HydrationFailed, "hydrationFailed"),
        ] {
            let lines = entry_lines(&ReportEntry {
                omitted: Some(ReportOmission { reason, size: 4242 }),
                ..entry()
            })
            .join("\n");
            assert!(lines.contains("OMITTED ENTRY"), "{lines}");
            assert!(lines.contains("Log"), "{lines}");
            assert!(lines.contains("4242 bytes"), "{lines}");
            assert!(lines.contains(reason_text), "{lines}");
        }
    }

    #[test]
    fn coverage_renders_every_list_with_discovered_and_notes() {
        let mut coverage = OplogExportCoverage::new(vec!["NOTE-TEXT".into()]);
        coverage.hydration_failures = CoverageList {
            shown: vec![OmittedEntryRef {
                index: 11,
                kind: "Start".into(),
                size: 99,
                reason: OmittedReason::TooLarge,
            }],
            discovered: 7,
        };
        coverage.start_unknown = CoverageList {
            shown: vec![StartUnknownRef {
                index: 12,
                start_index: 3,
            }],
            discovered: 7,
        };
        coverage.schema_less_rpc_values = CoverageList {
            shown: vec![SchemaLessRpcValueRef {
                index: 13,
                path: "request.input".into(),
            }],
            discovered: 7,
        };
        coverage.update_incomplete();
        let lines = coverage_lines(&coverage).join("\n");
        for expected in [
            "omitted entries: 7 discovered",
            "#00011 Start (99 bytes, tooLarge)",
            "unknown Start: 7 discovered",
            "#00012 (Start #00003)",
            "schema-less RPC values: 7 discovered",
            "#00013 request.input",
            "… 6 more",
            "note: NOTE-TEXT",
            "not in the report in full",
        ] {
            assert!(lines.contains(expected), "{expected} in {lines}");
        }
        let complete = coverage_lines(&OplogExportCoverage::new(vec![])).join("\n");
        assert!(!complete.contains("not in the report in full"));
        assert!(complete.contains("omitted entries: 0 discovered"));
    }

    #[test]
    fn header_renders_the_root_reference_only() {
        let root = ReportAgentRef {
            token: token("agent", 0xab),
        };
        let lines = header_lines(&OplogExportHeader {
            obfuscate: true,
            keep_scalars: false,
            keep_shape: true,
            keep_error_text: false,
            filtered: true,
            root: root.clone(),
            cli_version: "1.2.3".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
        })
        .join("\n");
        assert!(lines.contains(&root.pseudonym()));
        assert!(!lines.contains(&root.token.0));
        assert!(lines.contains("kept:              shape"));
        assert!(lines.contains("filtered:          yes"));
    }
}
