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
use crate::schema::tool::{CommandBody, Constraint, Doc, FlagShape, Ref, Tool};
use crate::schema::validation::subtyping::is_equivalent_cross_graph;
use crate::schema::{SchemaGraph, SchemaValue, TypedSchemaValue};
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::rc::Rc;
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

fn flag_default(shape: &FlagShape) -> SchemaValue {
    match shape {
        FlagShape::BoolFlag(shape) => SchemaValue::Bool(shape.default),
        FlagShape::CountFlag(_) => SchemaValue::U32(0),
    }
}

fn value_present(value: &SchemaValue, default: Option<&SchemaValue>, flag: bool) -> bool {
    if flag {
        return default.is_some_and(|default| default != value);
    }
    if default == Some(value) {
        return false;
    }
    match value {
        SchemaValue::Option { inner } => inner.is_some(),
        SchemaValue::List { elements } | SchemaValue::FixedList { elements } => {
            !elements.is_empty()
        }
        SchemaValue::Map { entries } => !entries.is_empty(),
        SchemaValue::Bool(value) => *value,
        SchemaValue::U32(value) => *value != 0,
        _ => true,
    }
}

fn value_matches(value: &SchemaValue, expected: &SchemaValue) -> bool {
    if value == expected {
        return true;
    }
    match value {
        SchemaValue::Option { inner } => inner
            .as_ref()
            .is_some_and(|value| value_matches(value, expected)),
        SchemaValue::List { elements } | SchemaValue::FixedList { elements } => {
            elements.iter().any(|value| value_matches(value, expected))
        }
        SchemaValue::Map { entries } => entries
            .iter()
            .any(|(_, value)| value_matches(value, expected)),
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub struct ToolType {
    lookup_name: String,
    definition: Arc<Tool>,
    schema: Arc<SchemaGraph>,
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
        let schema = Arc::new(definition.schema.clone());
        Ok(Self {
            lookup_name: registered.lookup_name,
            definition: Arc::new(definition),
            schema,
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

    /// Returns the root command node, including namespace-only metadata.
    pub fn root(&self) -> ToolNode {
        ToolNode {
            tool: self.clone(),
            index: 0,
            path: Vec::new(),
        }
    }

    /// Resolves names and aliases without requiring the selected node to be callable.
    pub fn node(&self, path: &[&str]) -> Option<ToolNode> {
        let mut index = 0usize;
        let mut canonical_path = Vec::with_capacity(path.len());
        for part in path {
            index = self
                .definition
                .commands
                .nodes
                .get(index)?
                .subcommands
                .iter()
                .find_map(|child| {
                    let child = child.as_usize()?;
                    let node = self.definition.commands.nodes.get(child)?;
                    (node.name == *part || node.aliases.iter().any(|alias| alias == part))
                        .then_some(child)
                })?;
            canonical_path.push(self.definition.commands.nodes[index].name.clone());
        }
        Some(ToolNode {
            tool: self.clone(),
            index,
            path: canonical_path,
        })
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
        let input_graph = Arc::new(input.record_schema.clone());
        let mut arguments = Vec::with_capacity(input.fields.len());
        let surfaces = self.definition.canonical_input_surfaces(index);
        input
            .fields
            .iter()
            .zip(surfaces)
            .for_each(|(field, surface)| {
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
                    CanonicalSurfaceRef::GlobalFlag { node, index } => (
                        false,
                        Some(flag_default(
                            &self.definition.commands.nodes[node].globals.flags[index].shape,
                        )),
                    ),
                    CanonicalSurfaceRef::BodyFlag { index } => {
                        (false, Some(flag_default(&body.flags[index].shape)))
                    }
                };
                let kind = match surface {
                    CanonicalSurfaceRef::GlobalOption { .. }
                    | CanonicalSurfaceRef::BodyOption { .. } => ToolArgumentKind::Option,
                    CanonicalSurfaceRef::GlobalFlag { .. }
                    | CanonicalSurfaceRef::BodyFlag { .. } => ToolArgumentKind::Flag,
                    CanonicalSurfaceRef::BodyPositional { .. } => ToolArgumentKind::Positional,
                    CanonicalSurfaceRef::BodyTail => ToolArgumentKind::Tail,
                };
                arguments.push(ToolArgument {
                    kind,
                    name: field.name.clone(),
                    aliases: field.aliases.clone(),
                    short: field.short,
                    schema: SchemaRef::with_root(input_graph.clone(), field.type_.clone()),
                    required,
                    default,
                });
            });
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
            input: SchemaRef::with_root(input_graph, input.record_schema.root.clone()),
            wire_input: input.record_schema,
            arguments,
        })
    }

    pub fn client(&self) -> ReflectedToolClient {
        ReflectedToolClient { tool: self.clone() }
    }
}

/// Metadata for one discovered command-tree node. Namespace nodes are discoverable but not
/// callable.
#[derive(Clone, Debug)]
pub struct ToolNode {
    tool: ToolType,
    index: usize,
    path: Vec<String>,
}

impl ToolNode {
    pub fn name(&self) -> &str {
        &self.tool.definition.commands.nodes[self.index].name
    }

    pub fn path(&self) -> &[String] {
        &self.path
    }

    pub fn aliases(&self) -> &[String] {
        &self.tool.definition.commands.nodes[self.index].aliases
    }

    pub fn doc(&self) -> &Doc {
        &self.tool.definition.commands.nodes[self.index].doc
    }

    pub fn is_callable(&self) -> bool {
        self.tool.definition.commands.nodes[self.index]
            .body
            .is_some()
    }

    pub fn command(&self) -> Result<ToolCommand, ToolReflectionError> {
        if !self.is_callable() {
            return Err(ToolReflectionError::InvalidMetadata(format!(
                "command namespace `{}` is not callable",
                self.path.join(" ")
            )));
        }
        let path = self.path.iter().map(String::as_str).collect::<Vec<_>>();
        self.tool.command(&path)
    }

    pub fn children(&self) -> Vec<ToolNode> {
        self.tool.definition.commands.nodes[self.index]
            .subcommands
            .iter()
            .filter_map(|child| child.as_usize())
            .map(|index| {
                let mut path = self.path.clone();
                path.push(self.tool.definition.commands.nodes[index].name.clone());
                ToolNode {
                    tool: self.tool.clone(),
                    index,
                    path,
                }
            })
            .collect()
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
        self.body()
            .result
            .as_ref()
            .map(|spec| SchemaRef::with_root(self.tool.schema.clone(), spec.type_.clone()))
    }
    /// Validates both the canonical input schema and effective command constraints.
    pub fn validate_value(&self, value: &SchemaValue) -> Result<(), ToolReflectionError> {
        self.input
            .validate_value(value)
            .map_err(ToolReflectionError::InvalidInput)?;
        self.check_constraints(value)
    }
    /// Packs canonical JSON and validates effective command constraints without invoking the tool.
    #[cfg(feature = "json")]
    pub fn pack_json(&self, input: &serde_json::Value) -> Result<SchemaValue, ToolReflectionError> {
        let value = self
            .input
            .pack_json(input)
            .map_err(ToolReflectionError::InvalidInput)?;
        self.check_constraints(&value)?;
        Ok(value)
    }
    fn checked_input(&self, value: SchemaValue) -> Result<TypedSchemaValue, ToolReflectionError> {
        self.validate_value(&value)?;
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
                Ref::Present(_) => value_present(
                    &fields[index],
                    self.arguments[index].default.as_ref(),
                    self.arguments[index].kind == ToolArgumentKind::Flag,
                ),
                Ref::ValueIs(item) => value_matches(&fields[index], &item.value),
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
            ToolReflectionError::Tool(tool_client::map_rpc_error(error, &|_, _| {
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
        let value = self.pack_json(input)?;
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
    pub async fn start_value(
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
            ToolReflectionError::Tool(tool_client::map_rpc_error(error, &|_, _| {
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
        .await
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
            ToolReflectionError::Tool(tool_client::map_rpc_error(error, &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            }))
        })?;
        rpc.invoke(&self.path, encoded, stdin.map(tool_client::pump_tool_stdin))
            .map_err(|error| {
                ToolReflectionError::Tool(tool_client::map_rpc_error(error, &self.error_decoder()))
            })
    }
    fn decode_result(
        &self,
        result: InvocationResult,
    ) -> Result<Option<SchemaValue>, ToolError<ReflectedToolCustomError>> {
        match (self.output_schema(), result.result) {
            (None, None) => Ok(None),
            (Some(schema), Some(value))
                if is_equivalent_cross_graph(
                    value.graph(),
                    &value.graph().root,
                    schema.graph(),
                    schema.root(),
                ) =>
            {
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
        let graph = self.tool.schema.clone();
        move |name, payload| {
            let Some(case) = body.errors.iter().find(|case| case.name == name) else {
                return Ok(None);
            };
            match &case.payload {
                Some(root) => {
                    if !is_equivalent_cross_graph(
                        payload.graph(),
                        &payload.graph().root,
                        &graph,
                        root,
                    ) {
                        return Err(format!("custom error `{name}` has the wrong schema"));
                    }
                    let schema = SchemaRef::with_root(graph.clone(), root.clone());
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

#[derive(Clone, Debug)]
pub struct ToolClientCommandDefinition {
    pub path: Arc<[String]>,
    pub input: SchemaRef,
    pub output: Option<SchemaRef>,
}

#[derive(Clone, Debug)]
pub struct ToolClientDefinitionBuilder {
    name: Option<String>,
    commands: Vec<ToolClientCommandDefinition>,
}

impl ToolClientDefinitionBuilder {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            commands: Vec::new(),
        }
    }

    pub fn unnamed() -> Self {
        Self {
            name: None,
            commands: Vec::new(),
        }
    }

    pub fn command<I, O, P, S>(mut self, path: P) -> Result<Self, ToolReflectionError>
    where
        I: crate::IntoSchema,
        O: crate::IntoSchema,
        P: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.push::<I, P, S>(path, Some(schema_for::<O>()?))?;
        Ok(self)
    }

    pub fn unit_command<I, P, S>(mut self, path: P) -> Result<Self, ToolReflectionError>
    where
        I: crate::IntoSchema,
        P: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.push::<I, P, S>(path, None)?;
        Ok(self)
    }

    fn push<I, P, S>(
        &mut self,
        path: P,
        output: Option<SchemaRef>,
    ) -> Result<(), ToolReflectionError>
    where
        I: crate::IntoSchema,
        P: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let path = path.into_iter().map(Into::into).collect::<Vec<_>>();
        if path.iter().any(String::is_empty) {
            return Err(ToolReflectionError::InvalidMetadata(
                "tool client command paths cannot contain empty segments".to_string(),
            ));
        }
        if self
            .commands
            .iter()
            .any(|command| command.path.as_ref() == path)
        {
            return Err(ToolReflectionError::InvalidMetadata(format!(
                "duplicate tool client command `{}`",
                path.join(" ")
            )));
        }
        self.commands.push(ToolClientCommandDefinition {
            path: path.into(),
            input: schema_for::<I>()?,
            output,
        });
        Ok(())
    }

    pub fn build(self) -> Result<ToolClientDefinition, ToolReflectionError> {
        if self.name.as_deref().is_some_and(str::is_empty) {
            return Err(ToolReflectionError::InvalidMetadata(
                "tool client name cannot be empty".to_string(),
            ));
        }
        Ok(ToolClientDefinition {
            name: self.name,
            commands: self.commands.into(),
        })
    }
}

fn schema_for<T: crate::IntoSchema>() -> Result<SchemaRef, ToolReflectionError> {
    crate::schema::try_into_schema_graph::<T>()
        .map(SchemaRef::new)
        .map_err(|error| ToolReflectionError::InvalidMetadata(error.to_string()))
}

/// A caller-owned typed subset of a remote tool. It binds optimistically and does not perform
/// discovery; the remote host remains authoritative for command availability.
#[derive(Clone, Debug)]
pub struct ToolClientDefinition {
    name: Option<String>,
    commands: Arc<[ToolClientCommandDefinition]>,
}

impl ToolClientDefinition {
    pub fn named(name: impl Into<String>) -> ToolClientDefinitionBuilder {
        ToolClientDefinitionBuilder::named(name)
    }

    pub fn unnamed() -> ToolClientDefinitionBuilder {
        ToolClientDefinitionBuilder::unnamed()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn commands(&self) -> &[ToolClientCommandDefinition] {
        &self.commands
    }

    pub fn client(&self) -> Result<TypedToolClient, ToolReflectionError> {
        let name = self.name.as_deref().ok_or_else(|| {
            ToolReflectionError::InvalidMetadata(
                "a nameless tool client definition requires an explicit target".to_string(),
            )
        })?;
        self.client_for(name)
    }

    pub fn client_for(&self, name: &str) -> Result<TypedToolClient, ToolReflectionError> {
        if name.is_empty() {
            return Err(ToolReflectionError::InvalidMetadata(
                "tool client target name cannot be empty".to_string(),
            ));
        }
        let rpc = ToolRpc::create(name).map_err(|error| {
            ToolReflectionError::Tool(tool_client::map_rpc_error(error, &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            }))
        })?;
        Ok(TypedToolClient {
            definition: self.clone(),
            rpc: Rc::new(rpc),
        })
    }
}

#[derive(Clone)]
pub struct TypedToolClient {
    definition: ToolClientDefinition,
    rpc: Rc<ToolRpc>,
}

impl TypedToolClient {
    pub fn command<I, O>(
        &self,
        path: &[&str],
    ) -> Result<TypedToolCommand<I, O>, ToolReflectionError>
    where
        I: crate::IntoSchema,
        O: crate::IntoSchema + crate::FromSchema,
    {
        let definition = self.definition_for(path)?;
        let output = definition.output.as_ref().ok_or_else(|| {
            ToolReflectionError::InvalidMetadata(format!(
                "tool client command `{}` declares no result",
                path.join(" ")
            ))
        })?;
        ensure_schema::<I>(&definition.input, "input")?;
        ensure_schema::<O>(output, "output")?;
        Ok(TypedToolCommand {
            definition,
            rpc: self.rpc.clone(),
            marker: PhantomData,
        })
    }

    pub fn unit_command<I>(
        &self,
        path: &[&str],
    ) -> Result<TypedUnitToolCommand<I>, ToolReflectionError>
    where
        I: crate::IntoSchema,
    {
        let definition = self.definition_for(path)?;
        if definition.output.is_some() {
            return Err(ToolReflectionError::InvalidMetadata(format!(
                "tool client command `{}` declares a result",
                path.join(" ")
            )));
        }
        ensure_schema::<I>(&definition.input, "input")?;
        Ok(TypedUnitToolCommand {
            definition,
            rpc: self.rpc.clone(),
            marker: PhantomData,
        })
    }

    fn definition_for(
        &self,
        path: &[&str],
    ) -> Result<ToolClientCommandDefinition, ToolReflectionError> {
        self.definition
            .commands
            .iter()
            .find(|command| {
                command.path.len() == path.len()
                    && command
                        .path
                        .iter()
                        .zip(path)
                        .all(|(left, right)| left == right)
            })
            .cloned()
            .ok_or_else(|| ToolReflectionError::NotFound(path.join(" ")))
    }
}

fn ensure_schema<T: crate::IntoSchema>(
    declared: &SchemaRef,
    label: &str,
) -> Result<(), ToolReflectionError> {
    let requested = schema_for::<T>()?;
    if !is_equivalent_cross_graph(
        declared.graph(),
        declared.root(),
        requested.graph(),
        requested.root(),
    ) {
        return Err(ToolReflectionError::InvalidMetadata(format!(
            "requested {label} type does not match the caller-owned tool definition"
        )));
    }
    Ok(())
}

pub struct TypedToolCommand<I, O> {
    definition: ToolClientCommandDefinition,
    rpc: Rc<ToolRpc>,
    marker: PhantomData<fn(I) -> O>,
}

impl<I, O> TypedToolCommand<I, O>
where
    I: crate::IntoSchema,
    O: crate::FromSchema,
{
    pub async fn invoke(&self, input: &I) -> Result<O, ToolReflectionError> {
        let value = input.to_value();
        self.definition
            .input
            .validate_value(&value)
            .map_err(ToolReflectionError::InvalidInput)?;
        let typed = TypedSchemaValue::new(self.definition.input.graph().clone(), value);
        let result = tool_client::invoke_and_await(
            self.rpc.as_ref(),
            &self.definition.path,
            &typed,
            None,
            None,
            |name, payload| Ok(Some(ReflectedToolCustomError { name, payload })),
        )
        .await
        .map_err(ToolReflectionError::Tool)?;
        let value = decode_caller_owned_result(&self.definition, result)?;
        O::from_value(&value).map_err(|error| {
            ToolReflectionError::Tool(ToolError::MalformedRemoteOutput(error.to_string()))
        })
    }
}

pub struct TypedUnitToolCommand<I> {
    definition: ToolClientCommandDefinition,
    rpc: Rc<ToolRpc>,
    marker: PhantomData<fn(I)>,
}

impl<I: crate::IntoSchema> TypedUnitToolCommand<I> {
    pub async fn invoke(&self, input: &I) -> Result<(), ToolReflectionError> {
        let value = input.to_value();
        self.definition
            .input
            .validate_value(&value)
            .map_err(ToolReflectionError::InvalidInput)?;
        let typed = TypedSchemaValue::new(self.definition.input.graph().clone(), value);
        let result = tool_client::invoke_and_await(
            self.rpc.as_ref(),
            &self.definition.path,
            &typed,
            None,
            None,
            |name, payload| Ok(Some(ReflectedToolCustomError { name, payload })),
        )
        .await
        .map_err(ToolReflectionError::Tool)?;
        if result.result.is_some() {
            return Err(ToolReflectionError::Tool(ToolError::MalformedRemoteOutput(
                "tool returned a value instead of unit".to_string(),
            )));
        }
        Ok(())
    }
}

fn decode_caller_owned_result(
    definition: &ToolClientCommandDefinition,
    result: InvocationResult,
) -> Result<SchemaValue, ToolReflectionError> {
    let expected = definition.output.as_ref().expect("value command");
    let value = result.result.ok_or_else(|| {
        ToolReflectionError::Tool(ToolError::MalformedRemoteOutput(
            "tool returned unit instead of a value".to_string(),
        ))
    })?;
    if !is_equivalent_cross_graph(
        value.graph(),
        &value.graph().root,
        expected.graph(),
        expected.root(),
    ) {
        return Err(ToolReflectionError::Tool(ToolError::MalformedRemoteOutput(
            "tool result schema does not match the caller-owned definition".to_string(),
        )));
    }
    expected.validate_value(value.value()).map_err(|error| {
        ToolReflectionError::Tool(ToolError::MalformedRemoteOutput(error.to_string()))
    })?;
    Ok(value.into_parts().1)
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
        self.invoke_with_stdin(path, input, None).await
    }

    pub async fn invoke_with_stdin(
        &self,
        path: &[String],
        input: &TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<InvocationResult, ToolError<ReflectedToolCustomError>> {
        let rpc = ToolRpc::create(&self.name).map_err(|error| {
            tool_client::map_rpc_error(error, &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            })
        })?;
        tool_client::invoke_and_await(
            &rpc,
            path,
            input,
            stdin.map(tool_client::pump_tool_stdin),
            None,
            |name, payload| Ok(Some(ReflectedToolCustomError { name, payload })),
        )
        .await
    }

    pub async fn start(
        &self,
        path: &[String],
        input: &TypedSchemaValue,
        stdin: Option<InputStream>,
        attach_stdout: bool,
    ) -> Result<
        ToolInvocation<InvocationResult, ReflectedToolCustomError>,
        ToolError<ReflectedToolCustomError>,
    > {
        let rpc = ToolRpc::create(&self.name).map_err(|error| {
            tool_client::map_rpc_error(error, &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            })
        })?;
        tool_client::start_tool_invocation_with_stdout(
            &rpc,
            path,
            input,
            stdin,
            attach_stdout,
            Ok,
            |name, payload| Ok(Some(ReflectedToolCustomError { name, payload })),
        )
        .await
    }

    pub fn trigger(
        &self,
        path: &[String],
        input: &TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<(), ToolError<ReflectedToolCustomError>> {
        let encoded = crate::encode_typed_schema_value(input).map_err(|error| {
            ToolError::Rpc(tool_client::RpcError::Protocol(format!(
                "failed to encode tool input: {error}"
            )))
        })?;
        let rpc = ToolRpc::create(&self.name).map_err(|error| {
            tool_client::map_rpc_error(error, &|_, _| {
                Ok::<Option<ReflectedToolCustomError>, String>(None)
            })
        })?;
        rpc.invoke(path, encoded, stdin.map(tool_client::pump_tool_stdin))
            .map_err(|error| {
                tool_client::map_rpc_error(error, &|name, payload| {
                    Ok(Some(ReflectedToolCustomError { name, payload }))
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IntoSchema;
    use crate::schema::SchemaType;
    use crate::schema::tool::{
        BoolFlagShape, CommandBody, CommandIndex, CommandNode, CommandTree, ErrorCase, ErrorKind,
        FlagSpec, Globals, OptionShape, OptionSpec, Positional, Positionals, ResultSpec,
        ValueIsRef,
    };
    use crate::schema::{NamedFieldType, SchemaTypeDef, TypeId};
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
        let schema = Arc::new(SchemaGraph::empty());
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
                schema: (*schema).clone(),
            }),
            schema,
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
    fn namespace_nodes_are_discoverable_but_not_callable() {
        let tool = sample();
        let root = tool.root();
        assert_eq!(root.name(), "sample");
        assert!(!root.is_callable());
        assert_eq!(root.children()[0].path(), &["run"]);
        assert!(root.command().is_err());

        let run = tool.node(&["r"]).expect("alias resolves to a node");
        assert!(run.is_callable());
        assert_eq!(run.path(), &["run"]);
        assert_eq!(run.command().unwrap().path(), &["run"]);
    }

    #[derive(crate::IntoSchema)]
    struct CallerResult {
        value: String,
    }

    #[derive(crate::IntoSchema)]
    struct RemoteResult {
        value: String,
    }

    #[test]
    fn caller_owned_results_compare_resolved_graphs() {
        let definition = ToolClientCommandDefinition {
            path: vec!["run".to_string()].into(),
            input: SchemaRef::new(SchemaGraph::empty()),
            output: Some(schema_for::<CallerResult>().unwrap()),
        };
        let graph = crate::schema::try_into_schema_graph::<RemoteResult>().unwrap();
        let result = InvocationResult {
            result: Some(TypedSchemaValue::new(
                graph,
                RemoteResult {
                    value: "ok".to_string(),
                }
                .to_value(),
            )),
        };
        assert_eq!(
            decode_caller_owned_result(&definition, result).unwrap(),
            CallerResult {
                value: "ok".to_string(),
            }
            .to_value()
        );
    }

    #[test]
    fn caller_owned_definition_rejects_duplicate_paths() {
        let builder = ToolClientDefinition::unnamed()
            .unit_command::<String, _, _>(Vec::<String>::new())
            .unwrap()
            .unit_command::<String, _, _>(["run"])
            .unwrap();
        assert!(
            builder
                .clone()
                .unit_command::<String, _, _>(Vec::<String>::new())
                .is_err()
        );
        assert!(builder.unit_command::<String, _, _>(["run"]).is_err());
        assert!(
            ToolClientDefinition::unnamed()
                .unit_command::<String, _, _>([""])
                .is_err()
        );
    }

    #[test]
    fn optional_inputs_use_the_canonical_wire_schema() {
        let mut tool = sample();
        let definition = Arc::make_mut(&mut tool.definition);
        let body = definition.commands.nodes[1].body.as_mut().unwrap();
        body.positionals.fixed[0].required = false;
        body.options.push(OptionSpec {
            long: "mode".to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            value_name: None,
            shape: OptionShape::Scalar(SchemaType::string()),
            default: None,
            required: false,
            env_var: None,
        });
        let command = tool.command(&["run"]).unwrap();
        assert_eq!(command.input_schema().graph(), &command.wire_input);
        for fields in [
            vec![
                SchemaValue::Option { inner: None },
                SchemaValue::Option { inner: None },
            ],
            vec![
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("hello".to_string()))),
                },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("fast".to_string()))),
                },
            ],
        ] {
            let value = SchemaValue::Record { fields };
            assert!(command.checked_input(value).is_ok());
        }
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

    fn named_record_graph(id: &str, field: &str) -> SchemaGraph {
        SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: TypeId::new(id),
                name: None,
                body: SchemaType::record(vec![NamedFieldType {
                    name: field.to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
            }],
            root: SchemaType::ref_to(TypeId::new(id)),
        }
    }

    #[test]
    fn reflected_result_checks_definitions_behind_identical_reference_ids() {
        let mut tool = sample();
        let expected = named_record_graph("example.Result", "old_field");
        let definition = Arc::make_mut(&mut tool.definition);
        definition.schema = expected.clone();
        definition.commands.nodes[1]
            .body
            .as_mut()
            .unwrap()
            .result
            .as_mut()
            .unwrap()
            .type_ = expected.root.clone();
        tool.schema = Arc::new(expected);
        let command = tool.command(&["run"]).unwrap();
        let remote_value = SchemaValue::Record {
            fields: vec![SchemaValue::String("value".to_string())],
        };
        assert!(matches!(
            command.decode_result(InvocationResult {
                result: Some(TypedSchemaValue::new(
                    named_record_graph("example.Result", "new_field"),
                    remote_value.clone(),
                )),
            }),
            Err(ToolError::MalformedRemoteOutput(_))
        ));
        assert_eq!(
            command
                .decode_result(InvocationResult {
                    result: Some(TypedSchemaValue::new(
                        named_record_graph("remote.Result", "old_field"),
                        remote_value.clone(),
                    )),
                })
                .unwrap(),
            Some(remote_value)
        );
    }

    #[test]
    fn reflected_custom_error_checks_definitions_behind_identical_reference_ids() {
        let mut tool = sample();
        let expected = named_record_graph("example.Error", "old_field");
        let definition = Arc::make_mut(&mut tool.definition);
        definition.schema = expected.clone();
        definition.commands.nodes[1]
            .body
            .as_mut()
            .unwrap()
            .errors
            .push(ErrorCase {
                name: "bad".to_string(),
                doc: Doc::default(),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: Some(expected.root.clone()),
            });
        tool.schema = Arc::new(expected);
        let command = tool.command(&["run"]).unwrap();
        let remote_value = SchemaValue::Record {
            fields: vec![SchemaValue::String("value".to_string())],
        };
        let decode = command.error_decoder();
        assert!(
            decode(
                "bad".to_string(),
                TypedSchemaValue::new(
                    named_record_graph("example.Error", "new_field"),
                    remote_value.clone(),
                ),
            )
            .is_err()
        );
        assert!(
            decode(
                "bad".to_string(),
                TypedSchemaValue::new(
                    named_record_graph("remote.Error", "old_field"),
                    remote_value
                ),
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn constraints_use_declared_flag_default_and_nested_value_is() {
        let mut tool = sample();
        let definition = Arc::make_mut(&mut tool.definition);
        let body = definition.commands.nodes[1].body.as_mut().unwrap();
        body.options.push(OptionSpec {
            long: "mode".to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            value_name: None,
            shape: OptionShape::Scalar(SchemaType::string()),
            default: None,
            required: false,
            env_var: None,
        });
        body.flags.push(FlagSpec {
            long: "enabled".to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            shape: FlagShape::BoolFlag(BoolFlagShape {
                default: true,
                negatable: true,
            }),
            env_var: None,
        });
        body.constraints.push(Constraint::RequiresAll(vec![
            Ref::Present("enabled".to_string()),
            Ref::ValueIs(ValueIsRef {
                name: "mode".to_string(),
                value: SchemaValue::String("fast".to_string()),
            }),
        ]));
        let command = tool.command(&["run"]).unwrap();
        assert_eq!(
            command.arguments()[2].default,
            Some(SchemaValue::Bool(true))
        );
        let input = |enabled| SchemaValue::Record {
            fields: vec![
                SchemaValue::String("hello".to_string()),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("fast".to_string()))),
                },
                SchemaValue::Bool(enabled),
            ],
        };
        assert!(command.validate_value(&input(false)).is_ok());
        assert!(command.validate_value(&input(true)).is_err());
        #[cfg(feature = "json")]
        {
            assert!(
                command
                    .pack_json(&serde_json::json!({
                        "message": "hello",
                        "mode": "fast",
                        "enabled": false,
                    }))
                    .is_ok()
            );
            assert!(
                command
                    .pack_json(&serde_json::json!({
                        "message": "hello",
                        "mode": "fast",
                        "enabled": true,
                    }))
                    .is_err()
            );
        }
    }
}
