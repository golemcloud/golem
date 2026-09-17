// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Tool discovery and calls driven by a caller-visible metadata snapshot.

use super::InputStream;
use super::reflection::{GolemReflectError, SchemaRef};
use super::tool_client::{self, InvocationResult, ToolError, ToolInvocation};
use crate::golem_agentic::golem::tool::host::{self, ToolRpc};
use crate::schema::tool::canonical::CanonicalSurfaceRef;
use crate::schema::tool::validation::validate_tool;
use crate::schema::tool::wit::decode_tool;
use crate::schema::tool::{CommandBody, Constraint, Doc, Ref, Tool};
use crate::schema::{
    MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ToolReflectionError {
    NotFound(String),
    InvalidMetadata(String),
    InvalidInput(GolemReflectError),
    Tool(ToolError<ReflectedToolCustomError>),
}

impl Display for ToolReflectionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "tool `{name}` was not found"),
            Self::InvalidMetadata(message) => write!(f, "invalid tool metadata: {message}"),
            Self::InvalidInput(error) => write!(f, "invalid tool input: {error}"),
            Self::Tool(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ToolReflectionError {}

#[derive(Clone, Debug, PartialEq)]
pub struct ReflectedToolCustomError {
    pub name: String,
    pub payload: TypedSchemaValue,
}

impl Display for ReflectedToolCustomError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "tool error `{}`", self.name)
    }
}

#[derive(Clone, Debug)]
pub struct ToolArgument {
    pub kind: ToolArgumentKind,
    pub name: String,
    pub aliases: Vec<String>,
    pub short: Option<char>,
    pub schema: SchemaRef,
    pub required: bool,
    pub default: Option<SchemaValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolArgumentKind {
    Positional,
    Tail,
    Option,
    Flag,
}

#[derive(Clone, Debug)]
pub struct ToolType {
    lookup_name: String,
    definition: Arc<Tool>,
    implemented_by: crate::ComponentId,
}

impl ToolType {
    fn from_registered(registered: host::RegisteredTool) -> Result<Self, ToolReflectionError> {
        let definition = decode_tool(registered.definition)
            .map_err(|error| ToolReflectionError::InvalidMetadata(error.to_string()))?;
        validate_tool(&definition).map_err(|errors| {
            ToolReflectionError::InvalidMetadata(
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        Ok(Self {
            lookup_name: registered.lookup_name,
            definition: Arc::new(definition),
            implemented_by: registered.implemented_by.into(),
        })
    }

    pub fn lookup_name(&self) -> &str {
        &self.lookup_name
    }
    pub fn definition(&self) -> &Tool {
        &self.definition
    }
    pub fn implemented_by(&self) -> &crate::ComponentId {
        &self.implemented_by
    }

    pub fn command(&self, path: &[&str]) -> Result<ToolCommand, ToolReflectionError> {
        let path = path
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>();
        let index = self
            .definition
            .command_index_by_path(&path)
            .ok_or_else(|| {
                ToolReflectionError::InvalidMetadata(format!(
                    "command `{}` has no body",
                    path.join(" ")
                ))
            })?;
        let input = self
            .definition
            .canonical_input_model(index)
            .map_err(|error| ToolReflectionError::InvalidMetadata(error.to_string()))?;
        let body = self.definition.commands.nodes[index]
            .body
            .as_ref()
            .expect("resolved body");
        let mut local_graph = input.record_schema.clone();
        let mut arguments = Vec::with_capacity(input.fields.len());
        let surfaces = self.definition.canonical_input_surfaces(index);
        let local_types = input
            .fields
            .iter()
            .zip(surfaces)
            .map(|(field, surface)| {
                let (required, default) = match surface {
                    CanonicalSurfaceRef::GlobalOption { node, index } => {
                        let spec = &self.definition.commands.nodes[node].globals.options[index];
                        (spec.required, spec.default.clone())
                    }
                    CanonicalSurfaceRef::BodyOption { index } => {
                        let spec = &body.options[index];
                        (spec.required, spec.default.clone())
                    }
                    CanonicalSurfaceRef::BodyPositional { index } => {
                        let spec = &body.positionals.fixed[index];
                        (spec.required, spec.default.clone())
                    }
                    CanonicalSurfaceRef::BodyTail => (
                        body.positionals.tail.as_ref().is_some_and(|s| s.min > 0),
                        None,
                    ),
                    _ => (false, None),
                };
                let optional_carrier = !required
                    && default.is_none()
                    && matches!(
                        surface,
                        CanonicalSurfaceRef::GlobalOption { .. }
                            | CanonicalSurfaceRef::BodyOption { .. }
                            | CanonicalSurfaceRef::BodyPositional { .. }
                    );
                let kind = match surface {
                    CanonicalSurfaceRef::GlobalOption { .. }
                    | CanonicalSurfaceRef::BodyOption { .. } => ToolArgumentKind::Option,
                    CanonicalSurfaceRef::GlobalFlag { .. }
                    | CanonicalSurfaceRef::BodyFlag { .. } => ToolArgumentKind::Flag,
                    CanonicalSurfaceRef::BodyPositional { .. } => ToolArgumentKind::Positional,
                    CanonicalSurfaceRef::BodyTail => ToolArgumentKind::Tail,
                };
                let schema_type = if optional_carrier {
                    SchemaType::option(field.type_.clone())
                } else {
                    field.type_.clone()
                };
                arguments.push(ToolArgument {
                    kind,
                    name: field.name.clone(),
                    aliases: field.aliases.clone(),
                    short: field.short,
                    schema: SchemaRef::new(SchemaGraph {
                        defs: local_graph.defs.clone(),
                        root: schema_type.clone(),
                    }),
                    required,
                    default,
                });
                NamedFieldType {
                    name: field.name.clone(),
                    body: schema_type,
                    metadata: MetadataEnvelope::default(),
                }
            })
            .collect::<Vec<_>>();
        local_graph.root = SchemaType::record(local_types);
        let mut node_index = 0;
        let canonical_path = path
            .iter()
            .map(|part| {
                node_index = self.definition.commands.nodes[node_index]
                    .subcommands
                    .iter()
                    .filter_map(|child| child.as_usize())
                    .find(|child| {
                        let node = &self.definition.commands.nodes[*child];
                        node.name == *part || node.aliases.contains(part)
                    })
                    .expect("resolved command path");
                self.definition.commands.nodes[node_index].name.clone()
            })
            .collect();
        Ok(ToolCommand {
            tool: self.clone(),
            index,
            path: canonical_path,
            input: SchemaRef::new(local_graph),
            wire_input: input.record_schema,
            arguments,
        })
    }

    pub fn client(&self) -> ReflectedToolClient {
        ReflectedToolClient { tool: self.clone() }
    }
}

pub fn get_all_tool_types() -> Result<Vec<ToolType>, ToolReflectionError> {
    host::get_all_tools()
        .into_iter()
        .map(ToolType::from_registered)
        .collect()
}

pub fn get_tool_type(name: &str) -> Result<ToolType, ToolReflectionError> {
    ToolType::from_registered(
        host::get_tool(name).ok_or_else(|| ToolReflectionError::NotFound(name.to_string()))?,
    )
}

#[derive(Clone, Debug)]
pub struct ReflectedToolClient {
    tool: ToolType,
}

impl ReflectedToolClient {
    pub fn command(&self, path: &[&str]) -> Result<ToolCommand, ToolReflectionError> {
        self.tool.command(path)
    }
    pub fn definition(&self) -> &ToolType {
        &self.tool
    }
}

#[derive(Clone, Debug)]
pub struct ToolCommand {
    tool: ToolType,
    index: usize,
    path: Vec<String>,
    input: SchemaRef,
    wire_input: SchemaGraph,
    arguments: Vec<ToolArgument>,
}

impl ToolCommand {
    pub fn path(&self) -> &[String] {
        &self.path
    }
    pub fn input_schema(&self) -> &SchemaRef {
        &self.input
    }
    pub fn arguments(&self) -> &[ToolArgument] {
        &self.arguments
    }
    pub fn body(&self) -> &CommandBody {
        self.tool.definition.commands.nodes[self.index]
            .body
            .as_ref()
            .expect("resolved body")
    }
    pub fn doc(&self) -> &Doc {
        &self.tool.definition.commands.nodes[self.index].doc
    }
    pub fn output_schema(&self) -> Option<SchemaRef> {
        self.body().result.as_ref().map(|spec| {
            SchemaRef::new(SchemaGraph {
                defs: self.tool.definition.schema.defs.clone(),
                root: spec.type_.clone(),
            })
        })
    }
    fn checked_input(&self, value: SchemaValue) -> Result<TypedSchemaValue, ToolReflectionError> {
        self.input
            .validate_value(&value)
            .map_err(ToolReflectionError::InvalidInput)?;
        self.check_constraints(&value)?;
        Ok(TypedSchemaValue::new(self.wire_input.clone(), value))
    }
    fn check_constraints(&self, value: &SchemaValue) -> Result<(), ToolReflectionError> {
        let SchemaValue::Record { fields } = value else {
            unreachable!("validated record")
        };
        let present = |reference: &Ref| -> bool {
            let name = match reference {
                Ref::Present(name) => name,
                Ref::ValueIs(item) => &item.name,
            };
            let index = self
                .arguments
                .iter()
                .position(|argument| argument.name == *name || argument.aliases.contains(name))
                .expect("validated constraint reference");
            match reference {
                Ref::Present(_) => match &fields[index] {
                    SchemaValue::Option { inner } => inner.is_some(),
                    other => self.arguments[index].default.as_ref() != Some(other),
                },
                Ref::ValueIs(item) => fields[index] == item.value,
            }
        };
        let quant = |refs: &[Ref], all: bool| -> bool {
            if all {
                refs.iter().all(&present)
            } else {
                refs.iter().any(&present)
            }
        };
        for (index, constraint) in self.body().constraints.iter().enumerate() {
            let okay = match constraint {
                Constraint::RequiresAll(refs) => quant(refs, true),
                Constraint::RequiresAny(refs) => quant(refs, false),
                Constraint::AllOrNone(refs) => {
                    let count = refs.iter().filter(|r| present(r)).count();
                    count == 0 || count == refs.len()
                }
                Constraint::MutexGroups(groups) => {
                    groups.iter().filter(|g| quant(&g.refs, true)).count() <= 1
                }
                Constraint::Implies(c) => {
                    !quant(
                        &c.lhs,
                        matches!(c.lhs_quant, crate::schema::tool::Quantifier::All),
                    ) || quant(
                        &c.rhs,
                        matches!(c.rhs_quant, crate::schema::tool::Quantifier::All),
                    )
                }
                Constraint::Forbids(c) => {
                    !quant(
                        &c.lhs,
                        matches!(c.lhs_quant, crate::schema::tool::Quantifier::All),
                    ) || !quant(&c.rhs, false)
                }
            };
            if !okay {
                return Err(ToolReflectionError::InvalidInput(
                    GolemReflectError::InvalidInput(format!("command constraint {index} failed")),
                ));
            }
        }
        Ok(())
    }
    pub async fn invoke_value(
        &self,
        value: SchemaValue,
    ) -> Result<Option<SchemaValue>, ToolReflectionError> {
        self.invoke_value_with_stdin(value, None).await
    }
    pub async fn invoke_value_with_stdin(
        &self,
        value: SchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<Option<SchemaValue>, ToolReflectionError> {
        if self
            .body()
            .stdout
            .as_ref()
            .is_some_and(|spec| spec.required)
        {
            return Err(ToolReflectionError::InvalidInput(
                GolemReflectError::InvalidInput(
                    "command requires caller-readable stdout".to_string(),
                ),
            ));
        }
        if self.body().stdin.as_ref().is_some_and(|spec| spec.required) && stdin.is_none() {
            return Err(ToolReflectionError::InvalidInput(
                GolemReflectError::InvalidInput("command requires stdin".to_string()),
            ));
        }
        let input = self.checked_input(value)?;
        let rpc = ToolRpc::create(self.tool.lookup_name()).map_err(|error| {
            ToolReflectionError::Tool(tool_client::map_rpc_error(error.into(), &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            }))
        })?;
        let result = tool_client::invoke_and_await(
            &rpc,
            &self.path,
            &input,
            stdin.map(tool_client::pump_tool_stdin),
            None,
            self.error_decoder(),
        )
        .await
        .map_err(ToolReflectionError::Tool)?;
        self.decode_result(result)
            .map_err(ToolReflectionError::Tool)
    }
    #[cfg(feature = "json")]
    pub async fn invoke_json(
        &self,
        input: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, ToolReflectionError> {
        let value = self
            .input
            .pack_json(input)
            .map_err(ToolReflectionError::InvalidInput)?;
        let result = self.invoke_value(value).await?;
        result
            .map(|value| {
                self.output_schema()
                    .expect("declared result")
                    .unpack_json(&value)
                    .map_err(|error| {
                        ToolReflectionError::Tool(ToolError::MalformedRemoteOutput(
                            error.to_string(),
                        ))
                    })
            })
            .transpose()
    }
    pub fn start_value(
        &self,
        value: SchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<ToolInvocation<Option<SchemaValue>, ReflectedToolCustomError>, ToolReflectionError>
    {
        if self.body().stdin.as_ref().is_some_and(|spec| spec.required) && stdin.is_none() {
            return Err(ToolReflectionError::InvalidInput(
                GolemReflectError::InvalidInput("command requires stdin".to_string()),
            ));
        }
        let input = self.checked_input(value)?;
        let rpc = ToolRpc::create(self.tool.lookup_name()).map_err(|error| {
            ToolReflectionError::Tool(tool_client::map_rpc_error(error.into(), &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            }))
        })?;
        let command = self.clone();
        tool_client::start_tool_invocation_with_stdout(
            &rpc,
            &self.path,
            &input,
            stdin,
            self.body().stdout.is_some(),
            move |result| command.decode_result(result),
            self.error_decoder(),
        )
        .map_err(ToolReflectionError::Tool)
    }
    pub fn trigger_value(
        &self,
        value: SchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<(), ToolReflectionError> {
        if self
            .body()
            .stdout
            .as_ref()
            .is_some_and(|spec| spec.required)
        {
            return Err(ToolReflectionError::InvalidInput(
                GolemReflectError::InvalidInput(
                    "command requires caller-readable stdout".to_string(),
                ),
            ));
        }
        if self.body().stdin.as_ref().is_some_and(|spec| spec.required) && stdin.is_none() {
            return Err(ToolReflectionError::InvalidInput(
                GolemReflectError::InvalidInput("command requires stdin".to_string()),
            ));
        }
        let input = self.checked_input(value)?;
        let encoded = crate::encode_typed_schema_value(&input).map_err(|error| {
            ToolReflectionError::InvalidInput(GolemReflectError::SchemaEncode(error.to_string()))
        })?;
        let rpc = ToolRpc::create(self.tool.lookup_name()).map_err(|error| {
            ToolReflectionError::Tool(tool_client::map_rpc_error(error.into(), &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            }))
        })?;
        rpc.invoke(&self.path, encoded, stdin.map(tool_client::pump_tool_stdin))
            .map_err(|error| {
                ToolReflectionError::Tool(tool_client::map_rpc_error(
                    error.into(),
                    &self.error_decoder(),
                ))
            })
    }
    fn decode_result(
        &self,
        result: InvocationResult,
    ) -> Result<Option<SchemaValue>, ToolError<ReflectedToolCustomError>> {
        match (self.output_schema(), result.result) {
            (None, None) => Ok(None),
            (Some(schema), Some(value)) if value.graph().root == *schema.root() => {
                schema
                    .validate_value(value.value())
                    .map_err(|error| ToolError::MalformedRemoteOutput(error.to_string()))?;
                Ok(Some(value.into_parts().1))
            }
            _ => Err(ToolError::MalformedRemoteOutput(
                "missing, unexpected, or mismatched structured result".to_string(),
            )),
        }
    }
    fn error_decoder(
        &self,
    ) -> impl Fn(String, TypedSchemaValue) -> Result<Option<ReflectedToolCustomError>, String> + 'static
    {
        let body = self.body().clone();
        let graph = self.tool.definition.schema.clone();
        move |name, payload| {
            let Some(case) = body.errors.iter().find(|case| case.name == name) else {
                return Ok(None);
            };
            match &case.payload {
                Some(root) => {
                    if payload.graph().root != *root {
                        return Err(format!("custom error `{name}` has the wrong schema"));
                    }
                    let schema = SchemaRef::new(SchemaGraph {
                        defs: graph.defs.clone(),
                        root: root.clone(),
                    });
                    schema
                        .validate_value(payload.value())
                        .map_err(|error| error.to_string())?;
                }
                None if !matches!(payload.value(), SchemaValue::Tuple { elements } if elements.is_empty()) =>
                {
                    return Err(format!("custom error `{name}` has an unexpected payload"));
                }
                None => {}
            }
            Ok(Some(ReflectedToolCustomError { name, payload }))
        }
    }
}

pub struct DynamicToolClient {
    name: String,
}

impl DynamicToolClient {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
    pub async fn invoke(
        &self,
        path: &[String],
        input: &TypedSchemaValue,
    ) -> Result<InvocationResult, ToolError<ReflectedToolCustomError>> {
        let rpc = ToolRpc::create(&self.name).map_err(|error| {
            tool_client::map_rpc_error(error.into(), &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            })
        })?;
        tool_client::invoke_and_await(&rpc, path, input, None, None, |name, payload| {
            Ok(Some(ReflectedToolCustomError { name, payload }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tool::{
        CommandBody, CommandIndex, CommandNode, CommandTree, Globals, Positional, Positionals,
        ResultSpec,
    };
    use test_r::test;

    fn sample() -> ToolType {
        let doc = Doc {
            summary: String::new(),
            description: String::new(),
            examples: Vec::new(),
        };
        let body = CommandBody {
            positionals: Positionals {
                fixed: vec![Positional {
                    name: "message".to_string(),
                    doc: doc.clone(),
                    value_name: None,
                    type_: SchemaType::string(),
                    default: None,
                    required: true,
                    accepts_stdio: false,
                }],
                tail: None,
            },
            options: Vec::new(),
            flags: Vec::new(),
            constraints: Vec::new(),
            stdin: None,
            stdout: None,
            result: Some(ResultSpec {
                type_: SchemaType::s32(),
                doc: doc.clone(),
                formatters: Vec::new(),
                default_formatter: String::new(),
            }),
            errors: Vec::new(),
            annotations: None,
        };
        ToolType {
            lookup_name: "sample".to_string(),
            definition: Arc::new(Tool {
                version: "1".to_string(),
                commands: CommandTree {
                    nodes: vec![
                        CommandNode {
                            name: "sample".to_string(),
                            aliases: Vec::new(),
                            doc: doc.clone(),
                            globals: Globals::default(),
                            subcommands: vec![CommandIndex(1)],
                            body: None,
                        },
                        CommandNode {
                            name: "run".to_string(),
                            aliases: vec!["r".to_string()],
                            doc,
                            globals: Globals::default(),
                            subcommands: Vec::new(),
                            body: Some(body),
                        },
                    ],
                },
                schema: SchemaGraph::empty(),
            }),
            implemented_by: crate::ComponentId::new(crate::Uuid::nil()),
        }
    }

    #[test]
    fn alias_resolves_to_canonical_path_and_bad_input_is_local() {
        let command = sample().command(&["r"]).expect("alias");
        assert_eq!(command.path(), &["run"]);
        assert_eq!(command.arguments()[0].name, "message");
        assert!(
            command
                .checked_input(SchemaValue::Record {
                    fields: vec![SchemaValue::S32(1)]
                })
                .is_err()
        );
    }

    #[test]
    fn malformed_remote_output_is_distinct() {
        let command = sample().command(&["run"]).expect("command");
        assert!(matches!(
            command.decode_result(InvocationResult { result: None }),
            Err(ToolError::MalformedRemoteOutput(_))
        ));
        assert!(matches!(
            command.decode_result(InvocationResult {
                result: Some(TypedSchemaValue::new(
                    SchemaGraph {
                        defs: Vec::new(),
                        root: SchemaType::string()
                    },
                    SchemaValue::String("wrong".to_string()),
                )),
            }),
            Err(ToolError::MalformedRemoteOutput(_))
        ));
    }
}
