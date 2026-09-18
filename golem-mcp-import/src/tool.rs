//! Import filtering, tool metadata assembly, and complete response projection.

mod headers;
pub(crate) use headers::encode as encode_header_value;

use crate::{content, schema};
use golem_schema::schema::tool::{self, *};
use golem_schema::schema::{
    MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaValue,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub schema: schema::Limits,
    pub content: content::ProjectionLimits,
    pub max_tools: usize,
    pub max_listing_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            schema: Default::default(),
            content: Default::default(),
            max_tools: 1024,
            max_listing_bytes: 8 << 20,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectedTool {
    pub definition: Tool,
    pub upstream_name: String,
    pub digest: String,
    input: schema::Projection,
    output: Option<schema::Projection>,
    parameter_headers: Vec<headers::Mapping>,
    limits: Limits,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CallError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid result: {0}")]
    InvalidResult(String),
    #[error("mcp-tool-error: {0}")]
    ToolError(String),
}

#[derive(Debug, PartialEq)]
pub struct ProjectedResult {
    pub value: SchemaValue,
    pub stdout: Option<Vec<u8>>,
}

impl ProjectedTool {
    /// Decodes an internal admission snapshot, not an untrusted upstream definition.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        stacker::maybe_grow(2 << 20, 64 << 20, || {
            let mut decoder = serde_json::Deserializer::from_slice(bytes);
            decoder.disable_recursion_limit();
            let tool = Self::deserialize(&mut decoder)?;
            decoder.end()?;
            Ok(tool)
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        stacker::maybe_grow(2 << 20, 64 << 20, || serde_json::to_vec(self))
    }

    pub fn new(upstream: &Value, name: &str, limits: Limits) -> Result<Self, String> {
        crate::limits::check(
            upstream,
            limits.schema.schema_bytes,
            limits.schema.schema_nodes,
            limits.schema.schema_depth,
        )
        .map_err(|e| format!("tool definition exceeds {e:?} limit"))?;
        if !tool::validation::is_valid_identifier(name) {
            return Err("invalid projected tool name".into());
        }
        let upstream_name = upstream
            .get("name")
            .and_then(Value::as_str)
            .ok_or("missing tool name")?
            .to_owned();
        let input = schema::Projection::new(
            upstream
                .get("inputSchema")
                .ok_or("missing inputSchema")?
                .clone(),
            limits.schema,
        )
        .map_err(|e| e.to_string())?;
        let parameter_headers = headers::collect(input.source())?;
        if !input.is_object() {
            return Err("inputSchema must project to an object".into());
        }
        let root = input
            .graph()
            .resolve_ref(input.root())
            .map_err(|e| e.to_string())?;
        let wrapped = !matches!(root, SchemaType::Record { .. });
        let fields = match root {
            SchemaType::Record { fields, .. } => fields.clone(),
            _ => vec![field("arguments", input.root().clone())],
        };
        let mut names = BTreeSet::new();
        let options = fields
            .iter()
            .map(|field| {
                let long = sanitize_name(&field.name);
                if !tool::validation::is_valid_identifier(&long) || !names.insert(long.clone()) {
                    return Err(format!(
                        "ambiguous or invalid input member name {:?}",
                        field.name
                    ));
                }
                let required = wrapped
                    || input
                        .root_fields()
                        .iter()
                        .find(|f| f.schema_name == field.name)
                        .is_some_and(|f| f.required);
                Ok(OptionSpec {
                    long,
                    short: None,
                    aliases: Vec::new(),
                    doc: Doc {
                        summary: field.metadata.doc.clone().unwrap_or_default(),
                        ..Default::default()
                    },
                    value_name: None,
                    shape: OptionShape::Scalar(field.body.clone()),
                    default: None,
                    required,
                    env_var: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let output = upstream
            .get("outputSchema")
            .map(|value| schema::Projection::new(value.clone(), limits.schema))
            .transpose()
            .map_err(|e| e.to_string())?;
        let structured = output
            .as_ref()
            .map(|p| p.root().clone())
            .unwrap_or_else(|| SchemaType::option(SchemaType::string()));
        let result_type = SchemaType::record(vec![
            field("structured", structured),
            field("content", content::schema()),
        ]);
        let mut graph = SchemaGraph::empty();
        for definition in input
            .graph()
            .defs
            .iter()
            .chain(output.iter().flat_map(|p| p.graph().defs.iter()))
        {
            if !graph.defs.iter().any(|d| d.id == definition.id) {
                graph.defs.push(definition.clone());
            }
        }
        let annotations = upstream
            .get("annotations")
            .map(|value| -> Result<_, String> {
                let object = value.as_object().ok_or("annotations must be an object")?;
                let hint = |name: &str, default| match object.get(name) {
                    None => Ok(default),
                    Some(Value::Bool(value)) => Ok(*value),
                    _ => Err(format!("{name} must be boolean")),
                };
                let read_only = hint("readOnlyHint", false)?;
                Ok(CommandAnnotations {
                    read_only,
                    destructive: hint("destructiveHint", !read_only)?,
                    idempotent: hint("idempotentHint", false)?,
                    open_world: hint("openWorldHint", true)?,
                })
            })
            .transpose()?;
        let definition = Tool {
            version: "0.0.0".into(),
            schema: graph,
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.into(),
                    aliases: Vec::new(),
                    doc: Doc {
                        summary: optional_string(upstream, "title")?
                            .unwrap_or(&upstream_name)
                            .into(),
                        description: optional_string(upstream, "description")?
                            .unwrap_or_default()
                            .into(),
                        examples: Vec::new(),
                    },
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: Some(CommandBody {
                        positionals: Positionals::default(),
                        options,
                        flags: Vec::new(),
                        constraints: Vec::new(),
                        stdin: None,
                        stdout: Some(StreamSpec {
                            doc: Doc::default(),
                            mime: vec!["*/*".into()],
                            required: false,
                        }),
                        result: Some(ResultSpec {
                            type_: result_type,
                            doc: Doc::default(),
                            formatters: vec![Formatter {
                                name: "json".into(),
                                doc: Doc::default(),
                            }],
                            default_formatter: "json".into(),
                        }),
                        errors: vec![ErrorCase {
                            name: "mcp-tool-error".into(),
                            doc: Doc::default(),
                            kind: ErrorKind::RuntimeError,
                            exit_code: 1,
                            payload: Some(SchemaType::string()),
                        }],
                        annotations,
                    }),
                }],
            },
        };
        tool::validation::validate_tool(&definition)
            .map_err(|e| format!("unrepresentable tool metadata: {e:?}"))?;
        definition
            .canonical_input_model(0)
            .map_err(|e| format!("invalid canonical tool input: {e:?}"))?;
        let digest = blake3::hash(
            &serde_json::to_vec(&(
                &upstream_name,
                &definition,
                input.source(),
                output.as_ref().map(schema::Projection::source),
            ))
            .map_err(|e| e.to_string())?,
        )
        .to_hex()
        .to_string();
        Ok(Self {
            definition,
            upstream_name,
            digest,
            input,
            output,
            parameter_headers,
            limits,
        })
    }

    pub fn arguments(&self, value: &SchemaValue) -> Result<Value, CallError> {
        let root = self
            .input
            .graph()
            .resolve_ref(self.input.root())
            .map_err(|e| CallError::InvalidInput(e.to_string()))?;
        let value = if matches!(root, SchemaType::Record { .. }) {
            value
        } else {
            match value {
                SchemaValue::Record { fields } if fields.len() == 1 => &fields[0],
                _ => {
                    return Err(CallError::InvalidInput(
                        "expected one structured arguments option".into(),
                    ));
                }
            }
        };
        self.input
            .to_json(value)
            .map_err(|e| CallError::InvalidInput(e.to_string()))
    }

    /// Builds the custom HTTP headers declared by the upstream input schema.
    pub fn parameter_headers(&self, arguments: &Value) -> Result<Vec<(String, String)>, CallError> {
        headers::extract(&self.parameter_headers, arguments).map_err(CallError::InvalidInput)
    }

    /// Project an already transport-bounded response, applying each payload's own budget.
    pub fn response(&self, response: &Value) -> Result<ProjectedResult, CallError> {
        let invalid = |message: String| CallError::InvalidResult(message);
        let content = response
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("missing content array".into()))?;
        match response.get("isError") {
            Some(Value::Bool(true)) => {
                let payload = content::error_payload(content, self.limits.content)
                    .map_err(|e| invalid(e.to_string()))?;
                return Err(CallError::ToolError(payload));
            }
            None | Some(Value::Bool(false)) => {}
            _ => return Err(invalid("isError must be boolean".into())),
        }
        let structured = match (&self.output, response.get("structuredContent")) {
            (Some(projection), Some(value)) => projection
                .from_json(value)
                .map_err(|e| invalid(e.to_string()))?,
            (Some(_), None) => return Err(invalid("missing declared structuredContent".into())),
            (None, value) => {
                if let Some(value) = value {
                    crate::limits::check(
                        value,
                        self.limits.schema.instance_bytes,
                        self.limits.schema.instance_bytes,
                        self.limits.schema.instance_depth,
                    )
                    .map_err(|e| invalid(format!("structuredContent exceeds {e:?} limit")))?;
                }
                SchemaValue::Option {
                    inner: value.map(|v| Box::new(SchemaValue::String(v.to_string()))),
                }
            }
        };
        let content =
            content::project(content, self.limits.content).map_err(|e| invalid(e.to_string()))?;
        Ok(ProjectedResult {
            value: SchemaValue::Record {
                fields: vec![structured, content.value],
            },
            stdout: content.stdout,
        })
    }
}

fn optional_string<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    value
        .get(key)
        .map(|v| v.as_str().ok_or_else(|| format!("{key} must be a string")))
        .transpose()
}

fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.into(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

pub fn sanitize_name(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            result.push(ch);
        } else if !result.is_empty() && !result.ends_with('-') {
            result.push('-');
        }
    }
    result.trim_end_matches('-').to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub upstream_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub tools: Vec<ProjectedTool>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn project_import(
    tools: &[Value],
    prefix: Option<&str>,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
    limits: Limits,
) -> Result<Batch, String> {
    if include.is_some() && exclude.is_some() {
        return Err("include and exclude are mutually exclusive".into());
    }
    if prefix.is_some_and(|p| !tool::validation::is_valid_identifier(p)) {
        return Err("invalid import prefix".into());
    }
    if tools.len() > limits.max_tools {
        return Err("tool count limit exceeded".into());
    }
    crate::limits::check_array_bytes(tools, limits.max_listing_bytes)
        .map_err(|_| "listing byte limit exceeded")?;
    let bounded: Vec<_> = tools
        .iter()
        .map(|value| {
            crate::limits::walk(value, limits.schema.schema_depth, &mut {
                limits.schema.schema_nodes
            })
            .map_err(|e| format!("tool definition exceeds {e:?} limit"))
        })
        .collect();
    let mut counts = BTreeMap::new();
    for tool in tools {
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            *counts.entry(sanitize_name(name)).or_insert(0usize) += 1;
        }
    }
    let mut batch = Batch {
        tools: Vec::new(),
        diagnostics: Vec::new(),
    };
    for (tool, bound) in tools.iter().zip(bounded) {
        let original = tool.get("name").and_then(Value::as_str).unwrap_or_default();
        let sanitized = sanitize_name(original);
        if include.is_some_and(|patterns| {
            !patterns
                .iter()
                .any(|p| glob_match::glob_match(p, &sanitized))
        }) || exclude.is_some_and(|patterns| {
            patterns
                .iter()
                .any(|p| glob_match::glob_match(p, &sanitized))
        }) {
            continue;
        }
        let name = prefix
            .map(|prefix| format!("{prefix}-{sanitized}"))
            .unwrap_or_else(|| sanitized.clone());
        let projected = if let Err(reason) = bound {
            Err(reason)
        } else if counts.get(&sanitized).is_some_and(|count| *count > 1) {
            Err("ambiguous sanitized upstream name".into())
        } else {
            ProjectedTool::new(tool, &name, limits)
        };
        match projected {
            Ok(tool) => batch.tools.push(tool),
            Err(reason) => batch.diagnostics.push(Diagnostic {
                upstream_name: original.into(),
                reason,
            }),
        }
    }
    Ok(batch)
}

pub struct MergedImports {
    pub tools: Vec<(usize, ProjectedTool)>,
    pub diagnostics: Vec<(usize, Diagnostic)>,
}

/// Each item in `imports` is a complete successful observation, in declaration order.
pub fn merge_imports(native_names: &BTreeSet<String>, imports: Vec<Batch>) -> MergedImports {
    let mut names = native_names.clone();
    let mut tools = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, batch) in imports.into_iter().enumerate() {
        diagnostics.extend(batch.diagnostics.into_iter().map(|d| (index, d)));
        for tool in batch.tools {
            if names.insert(tool.definition.name().unwrap().into()) {
                tools.push((index, tool));
            } else {
                diagnostics.push((
                    index,
                    Diagnostic {
                        upstream_name: tool.upstream_name,
                        reason: "name is shadowed by a higher-precedence tool".into(),
                    },
                ));
            }
        }
    }
    MergedImports { tools, diagnostics }
}

#[cfg(test)]
mod tests;
