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

use super::*;
use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::tool as proto;

fn required<T>(value: Option<T>, field: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("Missing field: {field}"))
}

fn encode_char(value: char) -> u32 {
    value.into()
}

fn decode_char(value: u32, field: &str) -> Result<char, String> {
    char::from_u32(value).ok_or_else(|| format!("Invalid Unicode scalar in {field}: {value}"))
}

impl From<Tool> for proto::Tool {
    fn from(value: Tool) -> Self {
        Self {
            version: value.version,
            commands: Some(value.commands.into()),
            schema: Some(value.schema.into()),
        }
    }
}

impl TryFrom<proto::Tool> for Tool {
    type Error = String;

    fn try_from(value: proto::Tool) -> Result<Self, Self::Error> {
        Ok(Self {
            version: value.version,
            commands: required(value.commands, "Tool.commands")?.try_into()?,
            schema: required(value.schema, "Tool.schema")?.try_into()?,
        })
    }
}

impl From<CommandTree> for proto::CommandTree {
    fn from(value: CommandTree) -> Self {
        Self {
            nodes: value.nodes.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::CommandTree> for CommandTree {
    type Error = String;

    fn try_from(value: proto::CommandTree) -> Result<Self, Self::Error> {
        Ok(Self {
            nodes: value
                .nodes
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<CommandNode> for proto::CommandNode {
    fn from(value: CommandNode) -> Self {
        Self {
            name: value.name,
            aliases: value.aliases,
            doc: Some(value.doc.into()),
            globals: Some(value.globals.into()),
            subcommands: value.subcommands.into_iter().map(|index| index.0).collect(),
            body: value.body.map(Into::into),
        }
    }
}

impl TryFrom<proto::CommandNode> for CommandNode {
    type Error = String;

    fn try_from(value: proto::CommandNode) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            aliases: value.aliases,
            doc: required(value.doc, "CommandNode.doc")?.into(),
            globals: required(value.globals, "CommandNode.globals")?.try_into()?,
            subcommands: value.subcommands.into_iter().map(CommandIndex).collect(),
            body: value.body.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<Globals> for proto::Globals {
    fn from(value: Globals) -> Self {
        Self {
            options: value.options.into_iter().map(Into::into).collect(),
            flags: value.flags.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::Globals> for Globals {
    type Error = String;

    fn try_from(value: proto::Globals) -> Result<Self, Self::Error> {
        Ok(Self {
            options: value
                .options
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            flags: value
                .flags
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<CommandBody> for proto::CommandBody {
    fn from(value: CommandBody) -> Self {
        Self {
            positionals: Some(value.positionals.into()),
            options: value.options.into_iter().map(Into::into).collect(),
            flags: value.flags.into_iter().map(Into::into).collect(),
            constraints: value.constraints.into_iter().map(Into::into).collect(),
            stdin: value.stdin.map(Into::into),
            stdout: value.stdout.map(Into::into),
            result: value.result.map(Into::into),
            errors: value.errors.into_iter().map(Into::into).collect(),
            annotations: value.annotations.map(Into::into),
        }
    }
}

impl TryFrom<proto::CommandBody> for CommandBody {
    type Error = String;

    fn try_from(value: proto::CommandBody) -> Result<Self, Self::Error> {
        Ok(Self {
            positionals: required(value.positionals, "CommandBody.positionals")?.try_into()?,
            options: value
                .options
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            flags: value
                .flags
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            constraints: value
                .constraints
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            stdin: value.stdin.map(TryInto::try_into).transpose()?,
            stdout: value.stdout.map(TryInto::try_into).transpose()?,
            result: value.result.map(TryInto::try_into).transpose()?,
            errors: value
                .errors
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            annotations: value.annotations.map(Into::into),
        })
    }
}

impl From<CommandAnnotations> for proto::CommandAnnotations {
    fn from(value: CommandAnnotations) -> Self {
        Self {
            read_only: value.read_only,
            destructive: value.destructive,
            idempotent: value.idempotent,
            open_world: value.open_world,
        }
    }
}

impl From<proto::CommandAnnotations> for CommandAnnotations {
    fn from(value: proto::CommandAnnotations) -> Self {
        Self {
            read_only: value.read_only,
            destructive: value.destructive,
            idempotent: value.idempotent,
            open_world: value.open_world,
        }
    }
}

impl From<Positionals> for proto::Positionals {
    fn from(value: Positionals) -> Self {
        Self {
            fixed: value.fixed.into_iter().map(Into::into).collect(),
            tail: value.tail.map(Into::into),
        }
    }
}

impl TryFrom<proto::Positionals> for Positionals {
    type Error = String;

    fn try_from(value: proto::Positionals) -> Result<Self, Self::Error> {
        Ok(Self {
            fixed: value
                .fixed
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            tail: value.tail.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<Positional> for proto::Positional {
    fn from(value: Positional) -> Self {
        Self {
            name: value.name,
            doc: Some(value.doc.into()),
            value_name: value.value_name,
            r#type: Some(value.type_.into()),
            default: value.default.map(Into::into),
            required: value.required,
            accepts_stdio: value.accepts_stdio,
        }
    }
}

impl TryFrom<proto::Positional> for Positional {
    type Error = String;

    fn try_from(value: proto::Positional) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            doc: required(value.doc, "Positional.doc")?.into(),
            value_name: value.value_name,
            type_: required(value.r#type, "Positional.type")?.try_into()?,
            default: value.default.map(TryInto::try_into).transpose()?,
            required: value.required,
            accepts_stdio: value.accepts_stdio,
        })
    }
}

impl From<TailPositional> for proto::TailPositional {
    fn from(value: TailPositional) -> Self {
        Self {
            name: value.name,
            doc: Some(value.doc.into()),
            value_name: value.value_name,
            item_type: Some(value.item_type.into()),
            min: value.min,
            max: value.max,
            separator: value.separator,
            verbatim: value.verbatim,
            accepts_stdio: value.accepts_stdio,
        }
    }
}

impl TryFrom<proto::TailPositional> for TailPositional {
    type Error = String;

    fn try_from(value: proto::TailPositional) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            doc: required(value.doc, "TailPositional.doc")?.into(),
            value_name: value.value_name,
            item_type: required(value.item_type, "TailPositional.item_type")?.try_into()?,
            min: value.min,
            max: value.max,
            separator: value.separator,
            verbatim: value.verbatim,
            accepts_stdio: value.accepts_stdio,
        })
    }
}

impl From<OptionSpec> for proto::OptionSpec {
    fn from(value: OptionSpec) -> Self {
        Self {
            long: value.long,
            short: value.short.map(encode_char),
            aliases: value.aliases,
            doc: Some(value.doc.into()),
            value_name: value.value_name,
            shape: Some(value.shape.into()),
            default: value.default.map(Into::into),
            required: value.required,
            env_var: value.env_var,
        }
    }
}

impl TryFrom<proto::OptionSpec> for OptionSpec {
    type Error = String;

    fn try_from(value: proto::OptionSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            long: value.long,
            short: value
                .short
                .map(|value| decode_char(value, "OptionSpec.short"))
                .transpose()?,
            aliases: value.aliases,
            doc: required(value.doc, "OptionSpec.doc")?.into(),
            value_name: value.value_name,
            shape: required(value.shape, "OptionSpec.shape")?.try_into()?,
            default: value.default.map(TryInto::try_into).transpose()?,
            required: value.required,
            env_var: value.env_var,
        })
    }
}

impl From<OptionShape> for proto::OptionShape {
    fn from(value: OptionShape) -> Self {
        use proto::option_shape::Value;
        let value = match value {
            OptionShape::Scalar(value) => Value::Scalar(value.into()),
            OptionShape::OptionalScalar(value) => Value::OptionalScalar(value.into()),
            OptionShape::RepeatableList(value) => Value::RepeatableList(value.into()),
            OptionShape::RepeatableMap(value) => Value::RepeatableMap(value.into()),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<proto::OptionShape> for OptionShape {
    type Error = String;

    fn try_from(value: proto::OptionShape) -> Result<Self, Self::Error> {
        use proto::option_shape::Value;
        match required(value.value, "OptionShape.value")? {
            Value::Scalar(value) => Ok(Self::Scalar(value.try_into()?)),
            Value::OptionalScalar(value) => Ok(Self::OptionalScalar(value.try_into()?)),
            Value::RepeatableList(value) => Ok(Self::RepeatableList(value.try_into()?)),
            Value::RepeatableMap(value) => Ok(Self::RepeatableMap(value.try_into()?)),
        }
    }
}

impl From<RepeatableListShape> for proto::RepeatableListShape {
    fn from(value: RepeatableListShape) -> Self {
        Self {
            repetition: Some(value.repetition.into()),
            item_type: Some(value.item_type.into()),
        }
    }
}

impl TryFrom<proto::RepeatableListShape> for RepeatableListShape {
    type Error = String;

    fn try_from(value: proto::RepeatableListShape) -> Result<Self, Self::Error> {
        Ok(Self {
            repetition: required(value.repetition, "RepeatableListShape.repetition")?.try_into()?,
            item_type: required(value.item_type, "RepeatableListShape.item_type")?.try_into()?,
        })
    }
}

impl From<RepeatableMapShape> for proto::RepeatableMapShape {
    fn from(value: RepeatableMapShape) -> Self {
        Self {
            repetition: Some(value.repetition.into()),
            map_type: Some(value.map_type.into()),
            duplicate_key_policy: match value.duplicate_key_policy {
                DuplicateKeyPolicy::Reject => proto::DuplicateKeyPolicy::Reject as i32,
                DuplicateKeyPolicy::LastWins => proto::DuplicateKeyPolicy::LastWins as i32,
            },
        }
    }
}

impl TryFrom<proto::RepeatableMapShape> for RepeatableMapShape {
    type Error = String;

    fn try_from(value: proto::RepeatableMapShape) -> Result<Self, Self::Error> {
        let duplicate_key_policy =
            match proto::DuplicateKeyPolicy::try_from(value.duplicate_key_policy).map_err(|_| {
                format!(
                    "Invalid RepeatableMapShape.duplicate_key_policy: {}",
                    value.duplicate_key_policy
                )
            })? {
                proto::DuplicateKeyPolicy::Reject => DuplicateKeyPolicy::Reject,
                proto::DuplicateKeyPolicy::LastWins => DuplicateKeyPolicy::LastWins,
                proto::DuplicateKeyPolicy::Unspecified => {
                    return Err("Missing RepeatableMapShape.duplicate_key_policy".to_string());
                }
            };
        Ok(Self {
            repetition: required(value.repetition, "RepeatableMapShape.repetition")?.try_into()?,
            map_type: required(value.map_type, "RepeatableMapShape.map_type")?.try_into()?,
            duplicate_key_policy,
        })
    }
}

impl From<Repetition> for proto::Repetition {
    fn from(value: Repetition) -> Self {
        use proto::repetition::Value;
        let value = match value {
            Repetition::Repeated => Value::Repeated(Empty {}),
            Repetition::Delimited(value) => Value::Delimited(encode_char(value)),
            Repetition::Either(value) => Value::Either(encode_char(value)),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<proto::Repetition> for Repetition {
    type Error = String;

    fn try_from(value: proto::Repetition) -> Result<Self, Self::Error> {
        use proto::repetition::Value;
        match required(value.value, "Repetition.value")? {
            Value::Repeated(_) => Ok(Self::Repeated),
            Value::Delimited(value) => {
                Ok(Self::Delimited(decode_char(value, "Repetition.delimited")?))
            }
            Value::Either(value) => Ok(Self::Either(decode_char(value, "Repetition.either")?)),
        }
    }
}

impl From<FlagSpec> for proto::FlagSpec {
    fn from(value: FlagSpec) -> Self {
        Self {
            long: value.long,
            short: value.short.map(encode_char),
            aliases: value.aliases,
            doc: Some(value.doc.into()),
            shape: Some(value.shape.into()),
            env_var: value.env_var,
        }
    }
}

impl TryFrom<proto::FlagSpec> for FlagSpec {
    type Error = String;

    fn try_from(value: proto::FlagSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            long: value.long,
            short: value
                .short
                .map(|value| decode_char(value, "FlagSpec.short"))
                .transpose()?,
            aliases: value.aliases,
            doc: required(value.doc, "FlagSpec.doc")?.into(),
            shape: required(value.shape, "FlagSpec.shape")?.try_into()?,
            env_var: value.env_var,
        })
    }
}

impl From<FlagShape> for proto::FlagShape {
    fn from(value: FlagShape) -> Self {
        use proto::flag_shape::Value;
        let value = match value {
            FlagShape::BoolFlag(value) => Value::BoolFlag(value.into()),
            FlagShape::CountFlag(max) => Value::CountFlag(proto::CountFlagShape { max }),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<proto::FlagShape> for FlagShape {
    type Error = String;

    fn try_from(value: proto::FlagShape) -> Result<Self, Self::Error> {
        use proto::flag_shape::Value;
        match required(value.value, "FlagShape.value")? {
            Value::BoolFlag(value) => Ok(Self::BoolFlag(value.into())),
            Value::CountFlag(value) => Ok(Self::CountFlag(value.max)),
        }
    }
}

impl From<BoolFlagShape> for proto::BoolFlagShape {
    fn from(value: BoolFlagShape) -> Self {
        Self {
            default: value.default,
            negatable: value.negatable,
        }
    }
}

impl From<proto::BoolFlagShape> for BoolFlagShape {
    fn from(value: proto::BoolFlagShape) -> Self {
        Self {
            default: value.default,
            negatable: value.negatable,
        }
    }
}

impl From<Ref> for proto::Ref {
    fn from(value: Ref) -> Self {
        use proto::r#ref::Value;
        let value = match value {
            Ref::Present(value) => Value::Present(value),
            Ref::ValueIs(value) => Value::ValueIs(value.into()),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<proto::Ref> for Ref {
    type Error = String;

    fn try_from(value: proto::Ref) -> Result<Self, Self::Error> {
        use proto::r#ref::Value;
        match required(value.value, "Ref.value")? {
            Value::Present(value) => Ok(Self::Present(value)),
            Value::ValueIs(value) => Ok(Self::ValueIs(value.try_into()?)),
        }
    }
}

impl From<ValueIsRef> for proto::ValueIsRef {
    fn from(value: ValueIsRef) -> Self {
        Self {
            name: value.name,
            value: Some(value.value.into()),
        }
    }
}

impl TryFrom<proto::ValueIsRef> for ValueIsRef {
    type Error = String;

    fn try_from(value: proto::ValueIsRef) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            value: required(value.value, "ValueIsRef.value")?.try_into()?,
        })
    }
}

impl From<Constraint> for proto::Constraint {
    fn from(value: Constraint) -> Self {
        use proto::constraint::Value;
        fn refs(values: Vec<Ref>) -> proto::Refs {
            proto::Refs {
                refs: values.into_iter().map(Into::into).collect(),
            }
        }
        let value = match value {
            Constraint::RequiresAll(values) => Value::RequiresAll(refs(values)),
            Constraint::AllOrNone(values) => Value::AllOrNone(refs(values)),
            Constraint::RequiresAny(values) => Value::RequiresAny(refs(values)),
            Constraint::MutexGroups(groups) => Value::MutexGroups(proto::RefGroups {
                groups: groups.into_iter().map(Into::into).collect(),
            }),
            Constraint::Implies(value) => Value::Implies(value.into()),
            Constraint::Forbids(value) => Value::Forbids(value.into()),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<proto::Constraint> for Constraint {
    type Error = String;

    fn try_from(value: proto::Constraint) -> Result<Self, Self::Error> {
        use proto::constraint::Value;
        fn refs(value: proto::Refs) -> Result<Vec<Ref>, String> {
            value.refs.into_iter().map(TryInto::try_into).collect()
        }
        match required(value.value, "Constraint.value")? {
            Value::RequiresAll(value) => Ok(Self::RequiresAll(refs(value)?)),
            Value::AllOrNone(value) => Ok(Self::AllOrNone(refs(value)?)),
            Value::RequiresAny(value) => Ok(Self::RequiresAny(refs(value)?)),
            Value::MutexGroups(value) => Ok(Self::MutexGroups(
                value
                    .groups
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )),
            Value::Implies(value) => Ok(Self::Implies(value.try_into()?)),
            Value::Forbids(value) => Ok(Self::Forbids(value.try_into()?)),
        }
    }
}

impl From<RefGroup> for proto::RefGroup {
    fn from(value: RefGroup) -> Self {
        Self {
            refs: value.refs.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::RefGroup> for RefGroup {
    type Error = String;

    fn try_from(value: proto::RefGroup) -> Result<Self, Self::Error> {
        Ok(Self {
            refs: value
                .refs
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

fn encode_quantifier(value: Quantifier) -> i32 {
    match value {
        Quantifier::All => proto::Quantifier::All as i32,
        Quantifier::Any => proto::Quantifier::Any as i32,
    }
}

fn decode_quantifier(value: i32, field: &str) -> Result<Quantifier, String> {
    match proto::Quantifier::try_from(value).map_err(|_| format!("Invalid {field}: {value}"))? {
        proto::Quantifier::All => Ok(Quantifier::All),
        proto::Quantifier::Any => Ok(Quantifier::Any),
        proto::Quantifier::Unspecified => Err(format!("Missing {field}")),
    }
}

impl From<ImpliesC> for proto::Implies {
    fn from(value: ImpliesC) -> Self {
        Self {
            lhs_quant: encode_quantifier(value.lhs_quant),
            lhs: value.lhs.into_iter().map(Into::into).collect(),
            rhs_quant: encode_quantifier(value.rhs_quant),
            rhs: value.rhs.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::Implies> for ImpliesC {
    type Error = String;

    fn try_from(value: proto::Implies) -> Result<Self, Self::Error> {
        Ok(Self {
            lhs_quant: decode_quantifier(value.lhs_quant, "Implies.lhs_quant")?,
            lhs: value
                .lhs
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            rhs_quant: decode_quantifier(value.rhs_quant, "Implies.rhs_quant")?,
            rhs: value
                .rhs
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ForbidsC> for proto::Forbids {
    fn from(value: ForbidsC) -> Self {
        Self {
            lhs_quant: encode_quantifier(value.lhs_quant),
            lhs: value.lhs.into_iter().map(Into::into).collect(),
            rhs: value.rhs.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::Forbids> for ForbidsC {
    type Error = String;

    fn try_from(value: proto::Forbids) -> Result<Self, Self::Error> {
        Ok(Self {
            lhs_quant: decode_quantifier(value.lhs_quant, "Forbids.lhs_quant")?,
            lhs: value
                .lhs
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            rhs: value
                .rhs
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<StreamSpec> for proto::StreamSpec {
    fn from(value: StreamSpec) -> Self {
        Self {
            doc: Some(value.doc.into()),
            mime: value.mime,
            required: value.required,
        }
    }
}

impl TryFrom<proto::StreamSpec> for StreamSpec {
    type Error = String;

    fn try_from(value: proto::StreamSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            doc: required(value.doc, "StreamSpec.doc")?.into(),
            mime: value.mime,
            required: value.required,
        })
    }
}

impl From<ResultSpec> for proto::ResultSpec {
    fn from(value: ResultSpec) -> Self {
        Self {
            r#type: Some(value.type_.into()),
            doc: Some(value.doc.into()),
            formatters: value.formatters.into_iter().map(Into::into).collect(),
            default_formatter: value.default_formatter,
        }
    }
}

impl TryFrom<proto::ResultSpec> for ResultSpec {
    type Error = String;

    fn try_from(value: proto::ResultSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            type_: required(value.r#type, "ResultSpec.type")?.try_into()?,
            doc: required(value.doc, "ResultSpec.doc")?.into(),
            formatters: value
                .formatters
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            default_formatter: value.default_formatter,
        })
    }
}

impl From<Formatter> for proto::Formatter {
    fn from(value: Formatter) -> Self {
        Self {
            name: value.name,
            doc: Some(value.doc.into()),
        }
    }
}

impl TryFrom<proto::Formatter> for Formatter {
    type Error = String;

    fn try_from(value: proto::Formatter) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            doc: required(value.doc, "Formatter.doc")?.into(),
        })
    }
}

impl From<ErrorCase> for proto::ErrorCase {
    fn from(value: ErrorCase) -> Self {
        Self {
            name: value.name,
            doc: Some(value.doc.into()),
            kind: match value.kind {
                ErrorKind::UsageError => proto::ErrorKind::UsageError as i32,
                ErrorKind::RuntimeError => proto::ErrorKind::RuntimeError as i32,
            },
            exit_code: value.exit_code.into(),
            payload: value.payload.map(Into::into),
        }
    }
}

impl TryFrom<proto::ErrorCase> for ErrorCase {
    type Error = String;

    fn try_from(value: proto::ErrorCase) -> Result<Self, Self::Error> {
        let kind = match proto::ErrorKind::try_from(value.kind)
            .map_err(|_| format!("Invalid ErrorCase.kind: {}", value.kind))?
        {
            proto::ErrorKind::UsageError => ErrorKind::UsageError,
            proto::ErrorKind::RuntimeError => ErrorKind::RuntimeError,
            proto::ErrorKind::Unspecified => return Err("Missing ErrorCase.kind".to_string()),
        };
        Ok(Self {
            name: value.name,
            doc: required(value.doc, "ErrorCase.doc")?.into(),
            kind,
            exit_code: value
                .exit_code
                .try_into()
                .map_err(|_| format!("ErrorCase.exit_code is out of range: {}", value.exit_code))?,
            payload: value.payload.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<Doc> for proto::Doc {
    fn from(value: Doc) -> Self {
        Self {
            summary: value.summary,
            description: value.description,
            examples: value.examples.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<proto::Doc> for Doc {
    fn from(value: proto::Doc) -> Self {
        Self {
            summary: value.summary,
            description: value.description,
            examples: value.examples.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<Example> for proto::Example {
    fn from(value: Example) -> Self {
        Self {
            title: value.title,
            body: value.body,
        }
    }
}

impl From<proto::Example> for Example {
    fn from(value: proto::Example) -> Self {
        Self {
            title: value.title,
            body: value.body,
        }
    }
}
