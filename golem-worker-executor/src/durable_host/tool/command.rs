// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::{
    CanonicalSurfaceRef, EntityCallMode, FlagShape, ModelTypedSchemaValue, OptionShape, OptionSpec,
    ResolvedToolCommand, SchemaType, SchemaValue, SerializableToolError, SerializableToolRpcError,
    Tool, ToolArgPattern, ToolIdentifier, ToolInvocationValue, is_equivalent_cross_graph,
    validate_value, value_to_cli_text_unredacted,
};

pub(super) fn render_tool_value(
    tool: &Tool,
    type_: &SchemaType,
    value: &SchemaValue,
) -> Result<ToolInvocationValue, String> {
    if crate::durable_host::schema_value_stream::contains_stream(value) {
        // A stream's contents are unknown at admission. Preserve its argument position without
        // allowing a literal-value permission to authorize those contents.
        Ok(ToolInvocationValue::Opaque)
    } else if let SchemaValue::String(value) = value {
        Ok(ToolInvocationValue::Literal(value.clone()))
    } else {
        value_to_cli_text_unredacted(&tool.schema, type_, value)
            .map(ToolInvocationValue::Literal)
            .map_err(|error| error.to_string())
    }
}

pub(super) fn join_tool_values(
    values: Vec<ToolInvocationValue>,
    separator: &str,
) -> ToolInvocationValue {
    let literals = values
        .into_iter()
        .map(|value| match value {
            ToolInvocationValue::Literal(value) => Some(value),
            ToolInvocationValue::Opaque => None,
        })
        .collect::<Option<Vec<_>>>();
    match literals {
        Some(values) => ToolInvocationValue::Literal(values.join(separator)),
        None => ToolInvocationValue::Opaque,
    }
}

pub(super) fn option_args(
    tool: &Tool,
    option: &OptionSpec,
    value: &SchemaValue,
) -> Result<Vec<ToolArgPattern<ToolInvocationValue>>, String> {
    let value = if !option.required && option.default.is_none() {
        match value {
            SchemaValue::Option { inner } => match inner.as_deref() {
                Some(value) => value,
                None => return Ok(Vec::new()),
            },
            _ => value,
        }
    } else {
        value
    };
    let name = ToolIdentifier::parse(&option.long)?;
    let argument = |value| ToolArgPattern::LongFlag {
        name: name.clone(),
        value: Some(value),
    };
    match &option.shape {
        OptionShape::Scalar(type_) | OptionShape::OptionalScalar(type_) => {
            let type_ = if !option.required && option.default.is_none() {
                match type_ {
                    SchemaType::Option { inner, .. } => inner,
                    _ => type_,
                }
            } else {
                type_
            };
            Ok(vec![argument(render_tool_value(tool, type_, value)?)])
        }
        OptionShape::RepeatableList(shape) => {
            let SchemaValue::List { elements } = value else {
                return Err(format!(
                    "canonical tool option '{}' must contain a list",
                    option.long
                ));
            };
            let rendered = elements
                .iter()
                .map(|value| render_tool_value(tool, &shape.item_type, value))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rendered.into_iter().map(argument).collect())
        }
        OptionShape::RepeatableMap(shape) => {
            let SchemaType::Map {
                key: key_type,
                value: value_type,
                ..
            } = tool
                .schema
                .resolve_ref(&shape.map_type)
                .map_err(|error| error.to_string())?
            else {
                return Err(format!(
                    "canonical tool option '{}' must declare a map type",
                    option.long
                ));
            };
            let SchemaValue::Map { entries } = value else {
                return Err(format!(
                    "canonical tool option '{}' must contain a map",
                    option.long
                ));
            };
            let rendered = entries
                .iter()
                .map(|(key, value)| {
                    Ok(join_tool_values(
                        vec![
                            render_tool_value(tool, key_type, key)?,
                            render_tool_value(tool, value_type, value)?,
                        ],
                        "=",
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(rendered.into_iter().map(argument).collect())
        }
    }
}

pub(super) fn flag_args(
    flag: &golem_common::schema::tool::FlagSpec,
    value: &SchemaValue,
) -> Result<Vec<ToolArgPattern<ToolInvocationValue>>, String> {
    let argument = |name: &str| {
        ToolIdentifier::parse(name).map(|name| ToolArgPattern::LongFlag { name, value: None })
    };
    match (flag.shape, value) {
        (FlagShape::BoolFlag(_), SchemaValue::Bool(true)) => Ok(vec![argument(&flag.long)?]),
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(false)) if shape.negatable => {
            Ok(vec![argument(&format!("no-{}", flag.long))?])
        }
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(false)) if !shape.default => Ok(Vec::new()),
        (FlagShape::BoolFlag(_), SchemaValue::Bool(false)) => Err(format!(
            "canonical tool flag '{}' cannot represent false",
            flag.long
        )),
        (FlagShape::CountFlag(max), SchemaValue::U32(count)) => {
            if max.is_some_and(|max| *count > max) {
                return Err(format!(
                    "canonical tool flag '{}' count {} exceeds its maximum",
                    flag.long, count
                ));
            }
            Ok(vec![argument(&flag.long)?; *count as usize])
        }
        (FlagShape::BoolFlag(_), _) => Err(format!(
            "canonical tool flag '{}' must contain a boolean",
            flag.long
        )),
        (FlagShape::CountFlag(_), _) => Err(format!(
            "canonical tool flag '{}' must contain a u32 count",
            flag.long
        )),
    }
}

pub(super) fn validate_stream_attachments(
    command: &ResolvedToolCommand,
    command_path: &[String],
    has_stdin: bool,
    has_stdout: bool,
    has_stderr: bool,
    call_mode: EntityCallMode,
) -> Result<(), SerializableToolRpcError> {
    if command.stdin_required.is_none() && has_stdin
        || command.stdin_required == Some(true) && !has_stdin
    {
        return Err(SerializableToolRpcError::ProtocolError(format!(
            "tool command {} stdin declaration does not match the supplied attachment",
            command_path.join(" ")
        )));
    }
    if call_mode != EntityCallMode::FireAndForget
        && (command.stdout_required.is_none() && has_stdout
            || command.stdout_required == Some(true) && !has_stdout)
    {
        return Err(SerializableToolRpcError::ProtocolError(format!(
            "tool command {} stdout declaration does not match the supplied attachment",
            command_path.join(" ")
        )));
    }
    if call_mode != EntityCallMode::FireAndForget
        && (command.stderr_required.is_none() && has_stderr
            || command.stderr_required == Some(true) && !has_stderr)
    {
        return Err(SerializableToolRpcError::ProtocolError(format!(
            "tool command {} stderr declaration does not match the supplied attachment",
            command_path.join(" ")
        )));
    }
    Ok(())
}

pub(super) fn resolve_tool_command(
    tool: &Tool,
    command_path: &[String],
    input: &ModelTypedSchemaValue,
) -> Result<ResolvedToolCommand, SerializableToolError> {
    let command_index = tool
        .command_index_by_path(command_path)
        .ok_or_else(|| SerializableToolError::InvalidCommandPath(command_path.to_vec()))?;
    let expected_input = tool
        .canonical_input_record_schema(command_index)
        .map_err(|error| SerializableToolError::InvalidInput(error.to_string()))?;
    if !is_equivalent_cross_graph(
        input.graph(),
        &input.graph().root,
        &expected_input,
        &expected_input.root,
    ) {
        return Err(SerializableToolError::InvalidInput(
            "tool input schema does not match the selected command".to_string(),
        ));
    }
    validate_value(input.graph(), &input.graph().root, input.value()).map_err(|errors| {
        SerializableToolError::InvalidInput(format!(
            "tool input value does not satisfy its schema: {}",
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    })?;
    let values = tool
        .decode_canonical_input_record(command_index, input.value().clone())
        .map_err(|error| SerializableToolError::InvalidInput(error.to_string()))?;
    let surfaces = tool.canonical_input_surfaces(command_index);
    let body = tool.commands.nodes[command_index]
        .body
        .as_ref()
        .expect("command_index_by_path only resolves commands with bodies");
    golem_schema::schema::tool::constraints::validate_tool_constraints(
        tool,
        command_index,
        &body.constraints,
        &surfaces,
        &values,
    )
    .map_err(SerializableToolError::ConstraintViolation)?;
    let mut args = Vec::new();

    for (surface, field) in surfaces.into_iter().zip(values) {
        match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => args.extend(
                option_args(
                    tool,
                    &tool.commands.nodes[node].globals.options[index],
                    &field.value,
                )
                .map_err(SerializableToolError::InvalidInput)?,
            ),
            CanonicalSurfaceRef::GlobalFlag { node, index } => args.extend(
                flag_args(
                    &tool.commands.nodes[node].globals.flags[index],
                    &field.value,
                )
                .map_err(SerializableToolError::InvalidInput)?,
            ),
            CanonicalSurfaceRef::BodyPositional { index } => {
                let positional = &body.positionals.fixed[index];
                let value = if !positional.required && positional.default.is_none() {
                    match &field.value {
                        SchemaValue::Option { inner } => inner.as_deref(),
                        value => Some(value),
                    }
                } else {
                    Some(&field.value)
                };
                if let Some(value) = value {
                    args.push(ToolArgPattern::Positional(
                        render_tool_value(tool, &positional.type_, value)
                            .map_err(SerializableToolError::InvalidInput)?,
                    ));
                }
            }
            CanonicalSurfaceRef::BodyTail => {
                let tail = body
                    .positionals
                    .tail
                    .as_ref()
                    .expect("BodyTail resolves an existing tail positional");
                let SchemaValue::List { elements } = &field.value else {
                    return Err(SerializableToolError::InvalidInput(format!(
                        "canonical tool tail positional '{}' must contain a list",
                        tail.name
                    )));
                };
                args.extend(
                    elements
                        .iter()
                        .map(|value| {
                            render_tool_value(tool, &tail.item_type, value)
                                .map(ToolArgPattern::Positional)
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(SerializableToolError::InvalidInput)?,
                );
            }
            CanonicalSurfaceRef::BodyOption { index } => args.extend(
                option_args(tool, &body.options[index], &field.value)
                    .map_err(SerializableToolError::InvalidInput)?,
            ),
            CanonicalSurfaceRef::BodyFlag { index } => args.extend(
                flag_args(&body.flags[index], &field.value)
                    .map_err(SerializableToolError::InvalidInput)?,
            ),
        }
    }

    Ok(ResolvedToolCommand {
        command_index,
        args,
        stdin_required: body.stdin.as_ref().map(|stream| stream.required),
        stdout_required: body.stdout.as_ref().map(|stream| stream.required),
        stderr_required: body.stderr.as_ref().map(|stream| stream.required),
    })
}
