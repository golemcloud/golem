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

use super::wire;
use crate::schema::SchemaGraph;
use crate::schema::tool as native;
use crate::schema::wit::{
    DecodeError, EncodeError, GraphDecoder, GraphEncoder, decode_value, encode_value,
};
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub(crate) enum ToolMetadataWireError {
    Encode(EncodeError),
    Decode(DecodeError),
}

impl Display for ToolMetadataWireError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => write!(formatter, "tool schema encode error: {error}"),
            Self::Decode(error) => write!(formatter, "tool schema decode error: {error}"),
        }
    }
}

impl From<EncodeError> for ToolMetadataWireError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

impl From<DecodeError> for ToolMetadataWireError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

pub(crate) fn encode_tool(tool: &native::Tool) -> Result<wire::Tool, ToolMetadataWireError> {
    let mut encoder = GraphEncoder::new(&tool.schema.defs)?;
    let commands = encode_command_tree(&mut encoder, &tool.commands)?;
    Ok(wire::Tool {
        version: tool.version.clone(),
        commands,
        schema: encoder.finish(),
    })
}

pub(crate) fn decode_tool(tool: wire::Tool) -> Result<native::Tool, ToolMetadataWireError> {
    let wire::Tool {
        version,
        commands,
        schema: wire_schema,
    } = tool;
    let decoder = GraphDecoder::new(&wire_schema)?;
    let mut schema = SchemaGraph::empty();
    schema.defs = decoder.decode_defs()?;
    Ok(native::Tool {
        version,
        commands: decode_command_tree(&decoder, commands)?,
        schema,
    })
}

fn encode_command_tree(
    encoder: &mut GraphEncoder,
    tree: &native::CommandTree,
) -> Result<wire::CommandTree, ToolMetadataWireError> {
    Ok(wire::CommandTree {
        nodes: tree
            .nodes
            .iter()
            .map(|node| encode_command_node(encoder, node))
            .collect::<Result<_, _>>()?,
    })
}

fn decode_command_tree(
    decoder: &GraphDecoder,
    tree: wire::CommandTree,
) -> Result<native::CommandTree, ToolMetadataWireError> {
    Ok(native::CommandTree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| decode_command_node(decoder, node))
            .collect::<Result<_, _>>()?,
    })
}

fn encode_command_node(
    encoder: &mut GraphEncoder,
    node: &native::CommandNode,
) -> Result<wire::CommandNode, ToolMetadataWireError> {
    Ok(wire::CommandNode {
        name: node.name.clone(),
        aliases: node.aliases.clone(),
        doc: encode_doc(&node.doc),
        globals: encode_globals(encoder, &node.globals)?,
        subcommands: node.subcommands.iter().map(|index| index.0).collect(),
        body: node
            .body
            .as_ref()
            .map(|body| encode_command_body(encoder, body))
            .transpose()?,
    })
}

fn decode_command_node(
    decoder: &GraphDecoder,
    node: wire::CommandNode,
) -> Result<native::CommandNode, ToolMetadataWireError> {
    Ok(native::CommandNode {
        name: node.name,
        aliases: node.aliases,
        doc: decode_doc(node.doc),
        globals: decode_globals(decoder, node.globals)?,
        subcommands: node
            .subcommands
            .into_iter()
            .map(native::CommandIndex)
            .collect(),
        body: node
            .body
            .map(|body| decode_command_body(decoder, body))
            .transpose()?,
    })
}

fn encode_globals(
    encoder: &mut GraphEncoder,
    globals: &native::Globals,
) -> Result<wire::Globals, ToolMetadataWireError> {
    Ok(wire::Globals {
        options: globals
            .options
            .iter()
            .map(|option| encode_option(encoder, option))
            .collect::<Result<_, _>>()?,
        flags: globals.flags.iter().map(encode_flag).collect(),
    })
}

fn decode_globals(
    decoder: &GraphDecoder,
    globals: wire::Globals,
) -> Result<native::Globals, ToolMetadataWireError> {
    Ok(native::Globals {
        options: globals
            .options
            .into_iter()
            .map(|option| decode_option(decoder, option))
            .collect::<Result<_, _>>()?,
        flags: globals.flags.into_iter().map(decode_flag).collect(),
    })
}

fn encode_command_body(
    encoder: &mut GraphEncoder,
    body: &native::CommandBody,
) -> Result<wire::CommandBody, ToolMetadataWireError> {
    Ok(wire::CommandBody {
        positionals: encode_positionals(encoder, &body.positionals)?,
        options: body
            .options
            .iter()
            .map(|option| encode_option(encoder, option))
            .collect::<Result<_, _>>()?,
        flags: body.flags.iter().map(encode_flag).collect(),
        constraints: body
            .constraints
            .iter()
            .map(encode_constraint)
            .collect::<Result<_, _>>()?,
        stdin: body.stdin.as_ref().map(encode_stream),
        stdout: body.stdout.as_ref().map(encode_stream),
        result: body
            .result
            .as_ref()
            .map(|result| encode_result(encoder, result))
            .transpose()?,
        errors: body
            .errors
            .iter()
            .map(|error| encode_error(encoder, error))
            .collect::<Result<_, _>>()?,
        annotations: body.annotations.as_ref().map(encode_annotations),
    })
}

fn decode_command_body(
    decoder: &GraphDecoder,
    body: wire::CommandBody,
) -> Result<native::CommandBody, ToolMetadataWireError> {
    Ok(native::CommandBody {
        positionals: decode_positionals(decoder, body.positionals)?,
        options: body
            .options
            .into_iter()
            .map(|option| decode_option(decoder, option))
            .collect::<Result<_, _>>()?,
        flags: body.flags.into_iter().map(decode_flag).collect(),
        constraints: body
            .constraints
            .into_iter()
            .map(decode_constraint)
            .collect::<Result<_, _>>()?,
        stdin: body.stdin.map(decode_stream),
        stdout: body.stdout.map(decode_stream),
        result: body
            .result
            .map(|result| decode_result(decoder, result))
            .transpose()?,
        errors: body
            .errors
            .into_iter()
            .map(|error| decode_error(decoder, error))
            .collect::<Result<_, _>>()?,
        annotations: body.annotations.map(decode_annotations),
    })
}

fn encode_positionals(
    encoder: &mut GraphEncoder,
    positionals: &native::Positionals,
) -> Result<wire::Positionals, ToolMetadataWireError> {
    Ok(wire::Positionals {
        fixed: positionals
            .fixed
            .iter()
            .map(|positional| encode_positional(encoder, positional))
            .collect::<Result<_, _>>()?,
        tail: positionals
            .tail
            .as_ref()
            .map(|tail| encode_tail(encoder, tail))
            .transpose()?,
    })
}

fn decode_positionals(
    decoder: &GraphDecoder,
    positionals: wire::Positionals,
) -> Result<native::Positionals, ToolMetadataWireError> {
    Ok(native::Positionals {
        fixed: positionals
            .fixed
            .into_iter()
            .map(|positional| decode_positional(decoder, positional))
            .collect::<Result<_, _>>()?,
        tail: positionals
            .tail
            .map(|tail| decode_tail(decoder, tail))
            .transpose()?,
    })
}

fn encode_positional(
    encoder: &mut GraphEncoder,
    positional: &native::Positional,
) -> Result<wire::Positional, ToolMetadataWireError> {
    Ok(wire::Positional {
        name: positional.name.clone(),
        doc: encode_doc(&positional.doc),
        value_name: positional.value_name.clone(),
        type_: encoder.encode_type(&positional.type_)?,
        default: positional.default.as_ref().map(encode_value).transpose()?,
        required: positional.required,
        accepts_stdio: positional.accepts_stdio,
    })
}

fn decode_positional(
    decoder: &GraphDecoder,
    positional: wire::Positional,
) -> Result<native::Positional, ToolMetadataWireError> {
    Ok(native::Positional {
        name: positional.name,
        doc: decode_doc(positional.doc),
        value_name: positional.value_name,
        type_: decoder.decode_type_at(positional.type_)?,
        default: positional.default.map(decode_value).transpose()?,
        required: positional.required,
        accepts_stdio: positional.accepts_stdio,
    })
}

fn encode_tail(
    encoder: &mut GraphEncoder,
    tail: &native::TailPositional,
) -> Result<wire::TailPositional, ToolMetadataWireError> {
    Ok(wire::TailPositional {
        name: tail.name.clone(),
        doc: encode_doc(&tail.doc),
        value_name: tail.value_name.clone(),
        item_type: encoder.encode_type(&tail.item_type)?,
        min: tail.min,
        max: tail.max,
        separator: tail.separator.clone(),
        verbatim: tail.verbatim,
        accepts_stdio: tail.accepts_stdio,
    })
}

fn decode_tail(
    decoder: &GraphDecoder,
    tail: wire::TailPositional,
) -> Result<native::TailPositional, ToolMetadataWireError> {
    Ok(native::TailPositional {
        name: tail.name,
        doc: decode_doc(tail.doc),
        value_name: tail.value_name,
        item_type: decoder.decode_type_at(tail.item_type)?,
        min: tail.min,
        max: tail.max,
        separator: tail.separator,
        verbatim: tail.verbatim,
        accepts_stdio: tail.accepts_stdio,
    })
}

fn encode_option(
    encoder: &mut GraphEncoder,
    option: &native::OptionSpec,
) -> Result<wire::OptionSpec, ToolMetadataWireError> {
    Ok(wire::OptionSpec {
        long: option.long.clone(),
        short: option.short,
        aliases: option.aliases.clone(),
        doc: encode_doc(&option.doc),
        value_name: option.value_name.clone(),
        shape: encode_option_shape(encoder, &option.shape)?,
        default: option.default.as_ref().map(encode_value).transpose()?,
        required: option.required,
        env_var: option.env_var.clone(),
    })
}

fn decode_option(
    decoder: &GraphDecoder,
    option: wire::OptionSpec,
) -> Result<native::OptionSpec, ToolMetadataWireError> {
    Ok(native::OptionSpec {
        long: option.long,
        short: option.short,
        aliases: option.aliases,
        doc: decode_doc(option.doc),
        value_name: option.value_name,
        shape: decode_option_shape(decoder, option.shape)?,
        default: option.default.map(decode_value).transpose()?,
        required: option.required,
        env_var: option.env_var,
    })
}

fn encode_option_shape(
    encoder: &mut GraphEncoder,
    shape: &native::OptionShape,
) -> Result<wire::OptionShape, ToolMetadataWireError> {
    Ok(match shape {
        native::OptionShape::Scalar(value_type) => {
            wire::OptionShape::Scalar(encoder.encode_type(value_type)?)
        }
        native::OptionShape::OptionalScalar(value_type) => {
            wire::OptionShape::OptionalScalar(encoder.encode_type(value_type)?)
        }
        native::OptionShape::RepeatableList(list) => {
            wire::OptionShape::RepeatableList(wire::RepeatableListShape {
                repetition: encode_repetition(&list.repetition),
                item_type: encoder.encode_type(&list.item_type)?,
            })
        }
        native::OptionShape::RepeatableMap(map) => {
            wire::OptionShape::RepeatableMap(wire::RepeatableMapShape {
                repetition: encode_repetition(&map.repetition),
                map_type: encoder.encode_type(&map.map_type)?,
                duplicate_key_policy: encode_duplicate_key_policy(map.duplicate_key_policy),
            })
        }
    })
}

fn decode_option_shape(
    decoder: &GraphDecoder,
    shape: wire::OptionShape,
) -> Result<native::OptionShape, ToolMetadataWireError> {
    Ok(match shape {
        wire::OptionShape::Scalar(value_type) => {
            native::OptionShape::Scalar(decoder.decode_type_at(value_type)?)
        }
        wire::OptionShape::OptionalScalar(value_type) => {
            native::OptionShape::OptionalScalar(decoder.decode_type_at(value_type)?)
        }
        wire::OptionShape::RepeatableList(list) => {
            native::OptionShape::RepeatableList(native::RepeatableListShape {
                repetition: decode_repetition(list.repetition),
                item_type: decoder.decode_type_at(list.item_type)?,
            })
        }
        wire::OptionShape::RepeatableMap(map) => {
            native::OptionShape::RepeatableMap(native::RepeatableMapShape {
                repetition: decode_repetition(map.repetition),
                map_type: decoder.decode_type_at(map.map_type)?,
                duplicate_key_policy: decode_duplicate_key_policy(map.duplicate_key_policy),
            })
        }
    })
}

fn encode_constraint(
    constraint: &native::Constraint,
) -> Result<wire::Constraint, ToolMetadataWireError> {
    Ok(match constraint {
        native::Constraint::RequiresAll(references) => {
            wire::Constraint::RequiresAll(encode_refs(references)?)
        }
        native::Constraint::AllOrNone(references) => {
            wire::Constraint::AllOrNone(encode_refs(references)?)
        }
        native::Constraint::RequiresAny(references) => {
            wire::Constraint::RequiresAny(encode_refs(references)?)
        }
        native::Constraint::MutexGroups(groups) => wire::Constraint::MutexGroups(
            groups
                .iter()
                .map(|group| {
                    Ok(wire::RefGroup {
                        refs: encode_refs(&group.refs)?,
                    })
                })
                .collect::<Result<_, ToolMetadataWireError>>()?,
        ),
        native::Constraint::Implies(implies) => wire::Constraint::Implies(wire::ImpliesC {
            lhs_quant: encode_quantifier(implies.lhs_quant),
            lhs: encode_refs(&implies.lhs)?,
            rhs_quant: encode_quantifier(implies.rhs_quant),
            rhs: encode_refs(&implies.rhs)?,
        }),
        native::Constraint::Forbids(forbids) => wire::Constraint::Forbids(wire::ForbidsC {
            lhs_quant: encode_quantifier(forbids.lhs_quant),
            lhs: encode_refs(&forbids.lhs)?,
            rhs: encode_refs(&forbids.rhs)?,
        }),
    })
}

fn decode_constraint(
    constraint: wire::Constraint,
) -> Result<native::Constraint, ToolMetadataWireError> {
    Ok(match constraint {
        wire::Constraint::RequiresAll(references) => {
            native::Constraint::RequiresAll(decode_refs(references)?)
        }
        wire::Constraint::AllOrNone(references) => {
            native::Constraint::AllOrNone(decode_refs(references)?)
        }
        wire::Constraint::RequiresAny(references) => {
            native::Constraint::RequiresAny(decode_refs(references)?)
        }
        wire::Constraint::MutexGroups(groups) => native::Constraint::MutexGroups(
            groups
                .into_iter()
                .map(|group| {
                    Ok(native::RefGroup {
                        refs: decode_refs(group.refs)?,
                    })
                })
                .collect::<Result<_, ToolMetadataWireError>>()?,
        ),
        wire::Constraint::Implies(implies) => native::Constraint::Implies(native::ImpliesC {
            lhs_quant: decode_quantifier(implies.lhs_quant),
            lhs: decode_refs(implies.lhs)?,
            rhs_quant: decode_quantifier(implies.rhs_quant),
            rhs: decode_refs(implies.rhs)?,
        }),
        wire::Constraint::Forbids(forbids) => native::Constraint::Forbids(native::ForbidsC {
            lhs_quant: decode_quantifier(forbids.lhs_quant),
            lhs: decode_refs(forbids.lhs)?,
            rhs: decode_refs(forbids.rhs)?,
        }),
    })
}

fn encode_refs(references: &[native::Ref]) -> Result<Vec<wire::Ref>, ToolMetadataWireError> {
    references
        .iter()
        .map(|reference| {
            Ok(match reference {
                native::Ref::Present(name) => wire::Ref::Present(name.clone()),
                native::Ref::ValueIs(value) => wire::Ref::ValueIs(wire::ValueIsRef {
                    name: value.name.clone(),
                    value: encode_value(&value.value)?,
                }),
            })
        })
        .collect()
}

fn decode_refs(references: Vec<wire::Ref>) -> Result<Vec<native::Ref>, ToolMetadataWireError> {
    references
        .into_iter()
        .map(|reference| {
            Ok(match reference {
                wire::Ref::Present(name) => native::Ref::Present(name),
                wire::Ref::ValueIs(value) => native::Ref::ValueIs(native::ValueIsRef {
                    name: value.name,
                    value: decode_value(value.value)?,
                }),
            })
        })
        .collect()
}

fn encode_result(
    encoder: &mut GraphEncoder,
    result: &native::ResultSpec,
) -> Result<wire::ResultSpec, ToolMetadataWireError> {
    Ok(wire::ResultSpec {
        type_: encoder.encode_type(&result.type_)?,
        doc: encode_doc(&result.doc),
        formatters: result.formatters.iter().map(encode_formatter).collect(),
        default_formatter: result.default_formatter.clone(),
    })
}

fn decode_result(
    decoder: &GraphDecoder,
    result: wire::ResultSpec,
) -> Result<native::ResultSpec, ToolMetadataWireError> {
    Ok(native::ResultSpec {
        type_: decoder.decode_type_at(result.type_)?,
        doc: decode_doc(result.doc),
        formatters: result
            .formatters
            .into_iter()
            .map(decode_formatter)
            .collect(),
        default_formatter: result.default_formatter,
    })
}

fn encode_error(
    encoder: &mut GraphEncoder,
    error: &native::ErrorCase,
) -> Result<wire::ErrorCase, ToolMetadataWireError> {
    Ok(wire::ErrorCase {
        name: error.name.clone(),
        doc: encode_doc(&error.doc),
        kind: encode_error_kind(error.kind),
        exit_code: error.exit_code,
        payload: error
            .payload
            .as_ref()
            .map(|payload| encoder.encode_type(payload))
            .transpose()?,
    })
}

fn decode_error(
    decoder: &GraphDecoder,
    error: wire::ErrorCase,
) -> Result<native::ErrorCase, ToolMetadataWireError> {
    Ok(native::ErrorCase {
        name: error.name,
        doc: decode_doc(error.doc),
        kind: decode_error_kind(error.kind),
        exit_code: error.exit_code,
        payload: error
            .payload
            .map(|payload| decoder.decode_type_at(payload))
            .transpose()?,
    })
}

fn encode_doc(doc: &native::Doc) -> wire::Doc {
    wire::Doc {
        summary: doc.summary.clone(),
        description: doc.description.clone(),
        examples: doc.examples.iter().map(encode_example).collect(),
    }
}

fn decode_doc(doc: wire::Doc) -> native::Doc {
    native::Doc {
        summary: doc.summary,
        description: doc.description,
        examples: doc.examples.into_iter().map(decode_example).collect(),
    }
}

fn encode_example(example: &native::Example) -> wire::Example {
    wire::Example {
        title: example.title.clone(),
        body: example.body.clone(),
    }
}

fn decode_example(example: wire::Example) -> native::Example {
    native::Example {
        title: example.title,
        body: example.body,
    }
}

fn encode_flag(flag: &native::FlagSpec) -> wire::FlagSpec {
    wire::FlagSpec {
        long: flag.long.clone(),
        short: flag.short,
        aliases: flag.aliases.clone(),
        doc: encode_doc(&flag.doc),
        shape: match &flag.shape {
            native::FlagShape::BoolFlag(shape) => wire::FlagShape::BoolFlag(wire::BoolFlagShape {
                default: shape.default,
                negatable: shape.negatable,
            }),
            native::FlagShape::CountFlag(max) => wire::FlagShape::CountFlag(*max),
        },
        env_var: flag.env_var.clone(),
    }
}

fn decode_flag(flag: wire::FlagSpec) -> native::FlagSpec {
    native::FlagSpec {
        long: flag.long,
        short: flag.short,
        aliases: flag.aliases,
        doc: decode_doc(flag.doc),
        shape: match flag.shape {
            wire::FlagShape::BoolFlag(shape) => {
                native::FlagShape::BoolFlag(native::BoolFlagShape {
                    default: shape.default,
                    negatable: shape.negatable,
                })
            }
            wire::FlagShape::CountFlag(max) => native::FlagShape::CountFlag(max),
        },
        env_var: flag.env_var,
    }
}

fn encode_repetition(repetition: &native::Repetition) -> wire::Repetition {
    match repetition {
        native::Repetition::Repeated => wire::Repetition::Repeated,
        native::Repetition::Delimited(delimiter) => wire::Repetition::Delimited(*delimiter),
        native::Repetition::Either(delimiter) => wire::Repetition::Either(*delimiter),
    }
}

fn decode_repetition(repetition: wire::Repetition) -> native::Repetition {
    match repetition {
        wire::Repetition::Repeated => native::Repetition::Repeated,
        wire::Repetition::Delimited(delimiter) => native::Repetition::Delimited(delimiter),
        wire::Repetition::Either(delimiter) => native::Repetition::Either(delimiter),
    }
}

fn encode_duplicate_key_policy(policy: native::DuplicateKeyPolicy) -> wire::DuplicateKeyPolicy {
    match policy {
        native::DuplicateKeyPolicy::Reject => wire::DuplicateKeyPolicy::Reject,
        native::DuplicateKeyPolicy::LastWins => wire::DuplicateKeyPolicy::LastWins,
    }
}

fn decode_duplicate_key_policy(policy: wire::DuplicateKeyPolicy) -> native::DuplicateKeyPolicy {
    match policy {
        wire::DuplicateKeyPolicy::Reject => native::DuplicateKeyPolicy::Reject,
        wire::DuplicateKeyPolicy::LastWins => native::DuplicateKeyPolicy::LastWins,
    }
}

fn encode_quantifier(quantifier: native::Quantifier) -> wire::Quantifier {
    match quantifier {
        native::Quantifier::All => wire::Quantifier::All,
        native::Quantifier::Any => wire::Quantifier::Any,
    }
}

fn decode_quantifier(quantifier: wire::Quantifier) -> native::Quantifier {
    match quantifier {
        wire::Quantifier::All => native::Quantifier::All,
        wire::Quantifier::Any => native::Quantifier::Any,
    }
}

fn encode_stream(stream: &native::StreamSpec) -> wire::StreamSpec {
    wire::StreamSpec {
        doc: encode_doc(&stream.doc),
        mime: stream.mime.clone(),
        required: stream.required,
    }
}

fn decode_stream(stream: wire::StreamSpec) -> native::StreamSpec {
    native::StreamSpec {
        doc: decode_doc(stream.doc),
        mime: stream.mime,
        required: stream.required,
    }
}

fn encode_formatter(formatter: &native::Formatter) -> wire::Formatter {
    wire::Formatter {
        name: formatter.name.clone(),
        doc: encode_doc(&formatter.doc),
    }
}

fn decode_formatter(formatter: wire::Formatter) -> native::Formatter {
    native::Formatter {
        name: formatter.name,
        doc: decode_doc(formatter.doc),
    }
}

fn encode_error_kind(kind: native::ErrorKind) -> wire::ErrorKind {
    match kind {
        native::ErrorKind::UsageError => wire::ErrorKind::UsageError,
        native::ErrorKind::RuntimeError => wire::ErrorKind::RuntimeError,
    }
}

fn decode_error_kind(kind: wire::ErrorKind) -> native::ErrorKind {
    match kind {
        wire::ErrorKind::UsageError => native::ErrorKind::UsageError,
        wire::ErrorKind::RuntimeError => native::ErrorKind::RuntimeError,
    }
}

fn encode_annotations(annotations: &native::CommandAnnotations) -> wire::CommandAnnotations {
    wire::CommandAnnotations {
        read_only: annotations.read_only,
        destructive: annotations.destructive,
        idempotent: annotations.idempotent,
        open_world: annotations.open_world,
    }
}

fn decode_annotations(annotations: wire::CommandAnnotations) -> native::CommandAnnotations {
    native::CommandAnnotations {
        read_only: annotations.read_only,
        destructive: annotations.destructive,
        idempotent: annotations.idempotent,
        open_world: annotations.open_world,
    }
}
