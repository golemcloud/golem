use futures_concurrency::prelude::*;
use golem_rust::golem_agentic::golem::tool::streams::{ByteStreamCloseCause, StreamWriteError};
use golem_rust::schema::tool::Tool;
use golem_rust::schema::validation::value::validate_value;
use golem_rust::schema::{SchemaGraph, SchemaType, SchemaValue};
use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, ToolInvokeError,
    UnderlyingTool,
};
use golem_rust::{FromSchema, IntoSchema, TypedSchemaValue, universal_tool_middleware};
use std::collections::BTreeSet;

const MAX_LITERAL_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema)]
pub struct LiteralRule {
    pub pattern: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema)]
pub struct StructuredRule {
    pub selector: String,
    pub pattern: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema)]
pub struct RedactionParameters {
    pub structured: Vec<StructuredRule>,
    pub stdout: Vec<LiteralRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Field(String),
    Index(usize),
    Wildcard,
}

#[derive(Debug, Clone)]
struct CompiledStructuredRule {
    selector: Vec<Segment>,
    pattern: String,
    replacement: String,
}

#[derive(Debug, Clone)]
pub struct Policy {
    structured: Vec<CompiledStructuredRule>,
    stdout: Vec<LiteralRule>,
}

impl Policy {
    pub fn compile(
        parameters: RedactionParameters,
        tool: &Tool,
        command_path: &[String],
    ) -> Result<Self, String> {
        validate_literal_rules(
            parameters
                .structured
                .iter()
                .map(|rule| (&rule.pattern, &rule.replacement)),
            "structured",
        )?;
        validate_literal_rules(
            parameters
                .stdout
                .iter()
                .map(|rule| (&rule.pattern, &rule.replacement)),
            "stdout",
        )?;

        let command_index = tool
            .command_index_by_path(command_path)
            .ok_or_else(|| format!("unknown command path `{}`", command_path.join(" ")))?;
        let result_type = tool.commands.nodes[command_index]
            .body
            .as_ref()
            .and_then(|body| body.result.as_ref())
            .map(|result| &result.type_);
        if !parameters.structured.is_empty() && result_type.is_none() {
            return Err("structured rules require a command result".to_string());
        }

        let mut structured = Vec::with_capacity(parameters.structured.len());
        for rule in parameters.structured {
            let selector = parse_selector(&rule.selector)?;
            validate_selector(
                &tool.schema,
                result_type.expect("checked structured result"),
                &selector,
                &rule.selector,
            )?;
            structured.push(CompiledStructuredRule {
                selector,
                pattern: rule.pattern,
                replacement: rule.replacement,
            });
        }
        Ok(Self {
            structured,
            stdout: parameters.stdout,
        })
    }

    pub fn redact_result(&self, result: TypedSchemaValue) -> Result<TypedSchemaValue, String> {
        let (graph, mut value) = result.into_parts();
        redact_selected(
            &graph,
            &graph.root,
            &mut value,
            &mut Vec::new(),
            &self.structured,
        )?;
        validate_value(&graph, &graph.root, &value).map_err(|errors| {
            format!(
                "redacted result violates its declared schema: {}",
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
        Ok(TypedSchemaValue::new(graph, value))
    }

    pub fn stdout_redactor(&self) -> Result<StreamingRedactor, String> {
        StreamingRedactor::new(self.stdout.clone())
    }
}

fn validate_literal_rules<'a>(
    rules: impl Iterator<Item = (&'a String, &'a String)>,
    channel: &str,
) -> Result<(), String> {
    let mut patterns = BTreeSet::new();
    for (pattern, replacement) in rules {
        if pattern.is_empty() {
            return Err(format!("{channel} patterns must not be empty"));
        }
        if replacement.is_empty() {
            return Err(format!("{channel} replacements must not be empty"));
        }
        if pattern.len() > MAX_LITERAL_BYTES || replacement.len() > MAX_LITERAL_BYTES {
            return Err(format!(
                "{channel} patterns and replacements must be at most {MAX_LITERAL_BYTES} bytes"
            ));
        }
        if !patterns.insert(pattern) {
            return Err(format!("duplicate {channel} pattern `{pattern}`"));
        }
    }
    Ok(())
}

fn parse_selector(selector: &str) -> Result<Vec<Segment>, String> {
    let bytes = selector.as_bytes();
    if bytes.first() != Some(&b'$') {
        return Err(format!("selector `{selector}` must start with `$`"));
    }
    let mut index = 1;
    let mut segments = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'.' => {
                index += 1;
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-'))
                {
                    index += 1;
                }
                if start == index {
                    return Err(format!("selector `{selector}` has an empty field segment"));
                }
                segments.push(Segment::Field(selector[start..index].to_string()));
            }
            b'[' => {
                index += 1;
                if bytes.get(index) == Some(&b'*') && bytes.get(index + 1) == Some(&b']') {
                    segments.push(Segment::Wildcard);
                    index += 2;
                    continue;
                }
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                if start == index || bytes.get(index) != Some(&b']') {
                    return Err(format!(
                        "selector `{selector}` has an invalid index segment"
                    ));
                }
                let value = selector[start..index]
                    .parse::<usize>()
                    .map_err(|_| format!("selector `{selector}` index is too large"))?;
                segments.push(Segment::Index(value));
                index += 1;
            }
            _ => return Err(format!("selector `{selector}` has unsupported syntax")),
        }
    }
    Ok(segments)
}

fn dereference<'a>(
    graph: &'a SchemaGraph,
    mut ty: &'a SchemaType,
) -> Result<&'a SchemaType, String> {
    let mut visited = BTreeSet::new();
    while let SchemaType::Ref { id, .. } = ty {
        if !visited.insert(id) {
            return Err(format!("selector encountered recursive alias `{id}`"));
        }
        ty = &graph
            .lookup(id)
            .ok_or_else(|| format!("selector encountered missing type `{id}`"))?
            .body;
    }
    Ok(ty)
}

fn validate_selector(
    graph: &SchemaGraph,
    ty: &SchemaType,
    segments: &[Segment],
    source: &str,
) -> Result<(), String> {
    let ty = dereference(graph, ty)?;
    if let SchemaType::Option { inner, .. } = ty {
        return validate_selector(graph, inner, segments, source);
    }
    let Some((segment, rest)) = segments.split_first() else {
        return match ty {
            SchemaType::String { .. } => Ok(()),
            SchemaType::Text { restrictions, .. }
                if restrictions.languages.is_none()
                    && restrictions.min_length.is_none()
                    && restrictions.max_length.is_none()
                    && restrictions.regex.is_none() =>
            {
                Ok(())
            }
            SchemaType::Text { .. } => Err(format!(
                "selector `{source}` targets constrained text; language, regex, and length constraints cannot be proven safe for substring replacement"
            )),
            SchemaType::Enum { .. } => Err(format!(
                "selector `{source}` targets an enum; replacement could violate its declared cases"
            )),
            SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. }
            | SchemaType::Stream { .. } => Err(format!(
                "selector `{source}` targets an opaque resource, which redaction never inspects"
            )),
            _ => Err(format!(
                "selector `{source}` must resolve to an unconstrained string or text value"
            )),
        };
    };
    match (segment, ty) {
        (Segment::Field(name), SchemaType::Record { fields, .. }) => {
            let field = fields
                .iter()
                .find(|field| field.name == *name)
                .ok_or_else(|| format!("selector `{source}` names unknown field `{name}`"))?;
            validate_selector(graph, &field.body, rest, source)
        }
        (Segment::Index(index), SchemaType::Tuple { elements, .. }) => {
            let element = elements.get(*index).ok_or_else(|| {
                format!("selector `{source}` tuple index {index} is out of bounds")
            })?;
            validate_selector(graph, element, rest, source)
        }
        (Segment::Index(_), SchemaType::List { element, .. })
        | (Segment::Index(_), SchemaType::FixedList { element, .. })
        | (Segment::Wildcard, SchemaType::List { element, .. })
        | (Segment::Wildcard, SchemaType::FixedList { element, .. }) => {
            validate_selector(graph, element, rest, source)
        }
        _ => Err(format!(
            "selector `{source}` does not match the declared result schema"
        )),
    }
}

fn redact_selected(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &mut SchemaValue,
    path: &mut Vec<Segment>,
    rules: &[CompiledStructuredRule],
) -> Result<(), String> {
    let ty = dereference(graph, ty)?;
    if let SchemaType::Option {
        inner: inner_ty, ..
    } = ty
    {
        let SchemaValue::Option { inner } = value else {
            return Err("redaction option schema does not match the runtime result".to_string());
        };
        return match inner {
            Some(inner) => redact_selected(graph, inner_ty, inner, path, rules),
            None => Ok(()),
        };
    }

    match (ty, value) {
        (SchemaType::String { .. }, SchemaValue::String(text)) => {
            *text = redact_text(text, path, rules);
        }
        (SchemaType::Text { .. }, SchemaValue::Text(payload)) => {
            payload.text = redact_text(&payload.text, path, rules);
        }
        (
            SchemaType::Record {
                fields: schema_fields,
                ..
            },
            SchemaValue::Record { fields },
        ) => {
            for (field, field_value) in schema_fields.iter().zip(fields) {
                path.push(Segment::Field(field.name.clone()));
                redact_selected(graph, &field.body, field_value, path, rules)?;
                path.pop();
            }
        }
        (
            SchemaType::Tuple {
                elements: element_types,
                ..
            },
            SchemaValue::Tuple { elements },
        ) => {
            for (index, (element_type, element)) in element_types.iter().zip(elements).enumerate() {
                path.push(Segment::Index(index));
                redact_selected(graph, element_type, element, path, rules)?;
                path.pop();
            }
        }
        (
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. },
            SchemaValue::List { elements } | SchemaValue::FixedList { elements },
        ) => {
            for (index, value) in elements.iter_mut().enumerate() {
                path.push(Segment::Index(index));
                redact_selected(graph, element, value, path, rules)?;
                path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

fn redact_text(text: &str, path: &[Segment], rules: &[CompiledStructuredRule]) -> String {
    let applicable = rules
        .iter()
        .filter(|rule| selector_matches(&rule.selector, path))
        .map(|rule| (rule.pattern.as_bytes(), rule.replacement.as_bytes()))
        .collect::<Vec<_>>();
    String::from_utf8(replace_literals(text.as_bytes(), &applicable))
        .expect("UTF-8 literals preserve UTF-8 text")
}

fn selector_matches(selector: &[Segment], path: &[Segment]) -> bool {
    selector.len() == path.len()
        && selector.iter().zip(path).all(|(expected, actual)| {
            expected == actual
                || matches!((expected, actual), (Segment::Wildcard, Segment::Index(_)))
        })
}

fn replace_literals(input: &[u8], rules: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut remaining = input;
    let mut output = Vec::with_capacity(input.len());
    while let Some((offset, rule_index)) = rules
        .iter()
        .enumerate()
        .filter_map(|(rule_index, (pattern, _))| {
            find_bytes(remaining, pattern).map(|offset| (offset, rule_index))
        })
        .min()
    {
        let (pattern, replacement) = rules[rule_index];
        output.extend_from_slice(&remaining[..offset]);
        output.extend_from_slice(replacement);
        remaining = &remaining[offset + pattern.len()..];
    }
    output.extend_from_slice(remaining);
    output
}

#[derive(Debug, Clone)]
pub struct StreamingRedactor {
    rules: Vec<(Vec<u8>, Vec<u8>)>,
    retained: Vec<u8>,
}

impl StreamingRedactor {
    pub fn new(rules: Vec<LiteralRule>) -> Result<Self, String> {
        validate_literal_rules(
            rules.iter().map(|rule| (&rule.pattern, &rule.replacement)),
            "stream",
        )?;
        Ok(Self {
            rules: rules
                .into_iter()
                .map(|rule| (rule.pattern.into_bytes(), rule.replacement.into_bytes()))
                .collect(),
            retained: Vec::new(),
        })
    }

    pub fn retained_len(&self) -> usize {
        self.retained.len()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.retained.extend_from_slice(chunk);
        self.drain(false)
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.drain(true)
    }

    fn drain(&mut self, final_chunk: bool) -> Vec<u8> {
        let mut output = Vec::new();
        'next_match: loop {
            for offset in 0..self.retained.len() {
                let remaining = &self.retained[offset..];
                let matched = self
                    .rules
                    .iter()
                    .position(|(pattern, _)| remaining.starts_with(pattern));
                let blocked = !final_chunk
                    && self
                        .rules
                        .iter()
                        .enumerate()
                        .any(|(rule_index, (pattern, _))| {
                            pattern.len() > remaining.len()
                                && pattern.starts_with(remaining)
                                && matched.is_none_or(|matched| rule_index < matched)
                        });
                if blocked {
                    output.extend(self.retained.drain(..offset));
                    return output;
                }
                if let Some(rule_index) = matched {
                    let (pattern, replacement) = &self.rules[rule_index];
                    output.extend(self.retained.drain(..offset));
                    self.retained.drain(..pattern.len());
                    output.extend_from_slice(replacement);
                    continue 'next_match;
                }
            }
            output.append(&mut self.retained);
            return output;
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|candidate| candidate == needle)
}

async fn relay_redacted_stdout(
    mut input: Option<InputStream>,
    mut output: Option<OutputStream>,
    mut redactor: StreamingRedactor,
) -> Result<(), String> {
    if let Some(input) = &mut input {
        while let Some(item) = input.next().await {
            match item {
                Ok(chunk) => {
                    let redacted = redactor.push(&chunk);
                    if let Some(output) = &mut output {
                        write_output("stdout", output.write(redacted).await)?;
                    }
                }
                Err(reason) => {
                    if let Some(mut output) = output {
                        write_output("stdout", output.write(redactor.finish()).await)?;
                        write_output("stdout", output.fail(reason).await)?;
                    }
                    return Ok(());
                }
            }
        }
    }
    if let Some(mut output) = output {
        write_output("stdout", output.write(redactor.finish()).await)?;
        write_output("stdout", output.finish().await)?;
    }
    Ok(())
}

async fn relay_output(
    channel: &str,
    mut input: Option<InputStream>,
    mut output: Option<OutputStream>,
) -> Result<(), String> {
    if let Some(input) = &mut input {
        while let Some(item) = input.next().await {
            match item {
                Ok(chunk) => {
                    if let Some(output) = &mut output {
                        write_output(channel, output.write(chunk).await)?;
                    }
                }
                Err(reason) => {
                    if let Some(output) = output {
                        write_output(channel, output.fail(reason).await)?;
                    }
                    return Ok(());
                }
            }
        }
    }
    if let Some(output) = output {
        write_output(channel, output.finish().await)?;
    }
    Ok(())
}

fn write_output(channel: &str, result: Result<(), StreamWriteError>) -> Result<(), String> {
    match result {
        Ok(()) | Err(StreamWriteError::Closed(ByteStreamCloseCause::ConsumerCancelled)) => Ok(()),
        Err(error) => Err(format!("failed to relay {channel}: {error:?}")),
    }
}

/// Redacts configured structured result fields and stdout literals while preserving every other
/// value, stderr, declared error, and output terminal.
#[universal_tool_middleware(
    name = "output-redaction",
    version = "0.1.0",
    parameters = RedactionParameters
)]
async fn output_redaction(
    parameters: RedactionParameters,
    _tool_name: String,
    tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let policy = Policy::compile(parameters, &tool_metadata, &command_path)
        .map_err(ToolInvokeError::InvalidInput)?;
    let redactor = policy
        .stdout_redactor()
        .map_err(ToolInvokeError::InvalidInput)?;
    let mut invocation = underlying.start(command_path, input, stdin).await?;
    let stdout_relay = relay_redacted_stdout(invocation.stdout.take(), stdout, redactor);
    let stderr_relay = relay_output("stderr", invocation.stderr.take(), stderr);
    let (result, stdout_result, stderr_result) =
        (invocation.get(), stdout_relay, stderr_relay).join().await;
    let mut result = result?;
    stdout_result.map_err(ToolInvokeError::InvalidResult)?;
    stderr_result.map_err(ToolInvokeError::InvalidResult)?;
    if let Some(value) = result.take() {
        result = Some(
            policy
                .redact_result(value)
                .map_err(ToolInvokeError::InvalidResult)?,
        );
    }
    Ok(InvocationResult {
        result,
        stdout: None,
        stderr: None,
    })
}

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod tests {
    use super::*;
    use golem_rust::schema::TextRestrictions;
    use golem_rust::schema::metadata::MetadataEnvelope;
    use golem_rust::schema::schema_value::TextValuePayload;
    use golem_rust::schema::tool::{
        CommandBody, CommandNode, CommandTree, Doc, Globals, Positionals, ResultSpec,
    };
    use test_r::test;

    fn tool(result: SchemaType) -> Tool {
        Tool {
            version: "1.0.0".to_string(),
            requires_filesystem: false,
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "fixture".to_string(),
                    aliases: vec![],
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: Some(CommandBody {
                        positionals: Positionals::default(),
                        options: vec![],
                        flags: vec![],
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        stderr: None,
                        result: Some(ResultSpec {
                            type_: result,
                            doc: Doc::default(),
                            formatters: vec![],
                            default_formatter: String::new(),
                        }),
                        errors: vec![],
                        annotations: None,
                    }),
                }],
            },
            schema: SchemaGraph::empty(),
        }
    }

    fn structured(selector: &str, pattern: &str, replacement: &str) -> RedactionParameters {
        RedactionParameters {
            structured: vec![StructuredRule {
                selector: selector.to_string(),
                pattern: pattern.to_string(),
                replacement: replacement.to_string(),
            }],
            stdout: vec![],
        }
    }

    #[test]
    fn redacts_only_selected_structured_siblings() {
        let result_type = SchemaType::record_from_fields([
            ("selected", SchemaType::string()),
            ("sibling", SchemaType::string()),
        ]);
        let policy = Policy::compile(
            structured("$.selected", "secret", "[redacted]"),
            &tool(result_type.clone()),
            &[],
        )
        .unwrap();
        let result = TypedSchemaValue::new(
            SchemaGraph::anonymous(result_type),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("a-secret-z".to_string()),
                    SchemaValue::String("a-secret-z".to_string()),
                ],
            },
        );
        let result = policy.redact_result(result).unwrap();
        assert_eq!(
            result.value(),
            &SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("a-[redacted]-z".to_string()),
                    SchemaValue::String("a-secret-z".to_string()),
                ]
            }
        );
    }

    #[test]
    fn structured_redaction_does_not_rescan_replacement_text() {
        let policy = Policy::compile(
            RedactionParameters {
                structured: vec![
                    StructuredRule {
                        selector: "$".to_string(),
                        pattern: "secret".to_string(),
                        replacement: "token".to_string(),
                    },
                    StructuredRule {
                        selector: "$".to_string(),
                        pattern: "token".to_string(),
                        replacement: "[redacted]".to_string(),
                    },
                ],
                stdout: vec![],
            },
            &tool(SchemaType::string()),
            &[],
        )
        .unwrap();

        let result = policy
            .redact_result(TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::string()),
                SchemaValue::String("secret".to_string()),
            ))
            .unwrap();
        assert_eq!(result.value(), &SchemaValue::String("token".to_string()));
    }

    #[test]
    fn rejects_schema_unsafe_and_opaque_selectors() {
        let constrained = SchemaType::Text {
            restrictions: TextRestrictions {
                languages: None,
                min_length: Some(8),
                max_length: Some(12),
                regex: Some("^[a-z]+$".to_string()),
            },
            metadata: MetadataEnvelope::default(),
        };
        let error =
            Policy::compile(structured("$", "secret", "X"), &tool(constrained), &[]).unwrap_err();
        assert!(error.contains("regex, and length constraints"));

        let enum_error = Policy::compile(
            structured("$", "secret", "X"),
            &tool(SchemaType::r#enum(vec!["safe".to_string()])),
            &[],
        )
        .unwrap_err();
        assert!(enum_error.contains("enum"));

        let opaque_error = Policy::compile(
            structured("$", "secret", "X"),
            &tool(SchemaType::secret(Default::default())),
            &[],
        )
        .unwrap_err();
        assert!(opaque_error.contains("opaque resource"));
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_literals() {
        let duplicate = RedactionParameters {
            structured: vec![],
            stdout: vec![
                LiteralRule {
                    pattern: "secret".to_string(),
                    replacement: "x".to_string(),
                },
                LiteralRule {
                    pattern: "secret".to_string(),
                    replacement: "y".to_string(),
                },
            ],
        };
        assert!(
            Policy::compile(duplicate, &tool(SchemaType::string()), &[])
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(
            StreamingRedactor::new(vec![LiteralRule {
                pattern: String::new(),
                replacement: "x".to_string(),
            }])
            .unwrap_err()
            .contains("must not be empty")
        );
    }

    #[test]
    fn streaming_redaction_is_exact_at_every_chunk_boundary_and_bounded() {
        let input = b"before sk_live_123 after";
        let expected = b"before [redacted]123 after";
        for split in 0..=input.len() {
            let mut redactor = StreamingRedactor::new(vec![LiteralRule {
                pattern: "sk_live_".to_string(),
                replacement: "[redacted]".to_string(),
            }])
            .unwrap();
            let mut output = redactor.push(&input[..split]);
            assert!(redactor.retained_len() <= "sk_live_".len() - 1);
            output.extend(redactor.push(&input[split..]));
            assert!(redactor.retained_len() <= "sk_live_".len() - 1);
            output.extend(redactor.finish());
            assert_eq!(output, expected, "split at byte {split}");
        }
    }

    #[test]
    fn streaming_redaction_handles_overlaps_without_rescanning_replacements() {
        let mut redactor = StreamingRedactor::new(vec![
            LiteralRule {
                pattern: "abc".to_string(),
                replacement: "abc-safe".to_string(),
            },
            LiteralRule {
                pattern: "bc".to_string(),
                replacement: "second".to_string(),
            },
        ])
        .unwrap();
        let mut output = redactor.push(b"zab");
        output.extend(redactor.push(b"cz"));
        output.extend(redactor.finish());
        assert_eq!(output, b"zabc-safez");
    }

    #[test]
    fn streaming_redaction_does_not_preempt_partial_earlier_match() {
        let rules = vec![
            LiteralRule {
                pattern: "abcd".to_string(),
                replacement: "long".to_string(),
            },
            LiteralRule {
                pattern: "bc".to_string(),
                replacement: "short".to_string(),
            },
        ];

        let mut unsplit = StreamingRedactor::new(rules.clone()).unwrap();
        let mut expected = unsplit.push(b"abcd");
        expected.extend(unsplit.finish());
        assert_eq!(expected, b"long");

        let mut split = StreamingRedactor::new(rules).unwrap();
        let mut actual = split.push(b"abc");
        actual.extend(split.push(b"d"));
        actual.extend(split.finish());
        assert_eq!(actual, expected, "redaction must not depend on chunking");
    }

    #[test]
    fn structured_text_value_retains_metadata() {
        let result_type = SchemaType::text(TextRestrictions::default());
        let policy = Policy::compile(
            structured("$", "secret", "safe"),
            &tool(result_type.clone()),
            &[],
        )
        .unwrap();
        let result = TypedSchemaValue::new(
            SchemaGraph::anonymous(result_type),
            SchemaValue::Text(TextValuePayload {
                text: "secret".to_string(),
                language: Some("en".to_string()),
            }),
        );
        let result = policy.redact_result(result).unwrap();
        assert_eq!(
            result.value(),
            &SchemaValue::Text(TextValuePayload {
                text: "safe".to_string(),
                language: Some("en".to_string()),
            })
        );
    }
}
