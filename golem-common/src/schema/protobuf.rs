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

//! Conversions between the recursive in-memory schema model
//! (`golem-common/src/schema`) and its protobuf mirror in the
//! `golem.schema` / `golem.registry` packages.
//!
//! The protobuf mirror follows the recursive Rust shape directly (protobuf,
//! unlike WIT, can express recursion), so the conversions are mechanical.
//! Forward (`From`) is infallible; reverse (`TryFrom`) returns `String` errors
//! for missing required fields, matching the convention in
//! `golem-common/src/model/agent/protobuf.rs`.
//!
//! Non-schema agent fields (`mode`, http mount/endpoint, read-only,
//! snapshotting, config, implementer) reuse the existing legacy conversions
//! between `base_model::agent` and the `golem.component` / `golem.registry`
//! proto packages.

use crate::base_model::agent::{AgentTypeName, Snapshotting};
use crate::model::Empty as ModelEmpty;
use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
    AutoInjectedKind, FieldSource, InputSchema, NamedField, OutputSchema,
    RegisteredAgentTypeSchema,
};
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType,
    SecretSpec, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
use golem_api_grpc::proto::golem::common::Empty as ProtoEmpty;
use golem_api_grpc::proto::golem::schema as proto;
use golem_api_grpc::proto::golem::schema::schema_type::Body;

// --- small helpers -----------------------------------------------------------

fn optional_meta(meta: MetadataEnvelope) -> Option<proto::MetadataEnvelope> {
    if meta.is_empty() {
        None
    } else {
        Some(meta.into())
    }
}

fn meta_from_proto(meta: Option<proto::MetadataEnvelope>) -> Result<MetadataEnvelope, String> {
    match meta {
        None => Ok(MetadataEnvelope::default()),
        Some(m) => m.try_into(),
    }
}

fn string_list_to_proto(value: Option<Vec<String>>) -> Option<proto::StringList> {
    value.map(|items| proto::StringList { items })
}

fn string_list_from_proto(value: Option<proto::StringList>) -> Option<Vec<String>> {
    value.map(|sl| sl.items)
}

fn box_to_proto(value: Box<SchemaType>) -> Box<proto::SchemaType> {
    Box::new((*value).into())
}

fn box_from_proto(value: Box<proto::SchemaType>) -> Result<Box<SchemaType>, String> {
    Ok(Box::new((*value).try_into()?))
}

fn opt_box_to_proto(value: Option<Box<SchemaType>>) -> Option<Box<proto::SchemaType>> {
    value.map(box_to_proto)
}

fn opt_box_from_proto(
    value: Option<Box<proto::SchemaType>>,
) -> Result<Option<Box<SchemaType>>, String> {
    value.map(box_from_proto).transpose()
}

fn req_box_from_proto(
    value: Option<Box<proto::SchemaType>>,
    ctx: &str,
) -> Result<Box<SchemaType>, String> {
    box_from_proto(value.ok_or_else(|| format!("Missing field: {ctx}"))?)
}

// --- MetadataEnvelope / Role -------------------------------------------------

impl From<MetadataEnvelope> for proto::MetadataEnvelope {
    fn from(value: MetadataEnvelope) -> Self {
        Self {
            doc: value.doc,
            aliases: value.aliases,
            examples: value.examples,
            deprecated: value.deprecated,
            role: value.role.map(Into::into),
        }
    }
}

impl TryFrom<proto::MetadataEnvelope> for MetadataEnvelope {
    type Error = String;

    fn try_from(value: proto::MetadataEnvelope) -> Result<Self, Self::Error> {
        Ok(Self {
            doc: value.doc,
            aliases: value.aliases,
            examples: value.examples,
            deprecated: value.deprecated,
            role: value.role.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<Role> for proto::Role {
    fn from(value: Role) -> Self {
        let inner = match value {
            Role::Multimodal => proto::role::Value::Multimodal(ProtoEmpty {}),
            Role::Other(s) => proto::role::Value::Other(s),
        };
        Self { value: Some(inner) }
    }
}

impl TryFrom<proto::Role> for Role {
    type Error = String;

    fn try_from(value: proto::Role) -> Result<Self, Self::Error> {
        match value.value {
            Some(proto::role::Value::Multimodal(_)) => Ok(Role::Multimodal),
            Some(proto::role::Value::Other(s)) => Ok(Role::Other(s)),
            None => Err("Missing field: Role.value".to_string()),
        }
    }
}

// --- SchemaType --------------------------------------------------------------

impl From<SchemaType> for proto::SchemaType {
    fn from(value: SchemaType) -> Self {
        let metadata = optional_meta(value.metadata().clone());
        let body = match value {
            SchemaType::Ref { id, .. } => Body::RefType(id.0),
            SchemaType::Bool { .. } => Body::BoolType(ProtoEmpty {}),
            SchemaType::S8 { .. } => Body::S8Type(ProtoEmpty {}),
            SchemaType::S16 { .. } => Body::S16Type(ProtoEmpty {}),
            SchemaType::S32 { .. } => Body::S32Type(ProtoEmpty {}),
            SchemaType::S64 { .. } => Body::S64Type(ProtoEmpty {}),
            SchemaType::U8 { .. } => Body::U8Type(ProtoEmpty {}),
            SchemaType::U16 { .. } => Body::U16Type(ProtoEmpty {}),
            SchemaType::U32 { .. } => Body::U32Type(ProtoEmpty {}),
            SchemaType::U64 { .. } => Body::U64Type(ProtoEmpty {}),
            SchemaType::F32 { .. } => Body::F32Type(ProtoEmpty {}),
            SchemaType::F64 { .. } => Body::F64Type(ProtoEmpty {}),
            SchemaType::Char { .. } => Body::CharType(ProtoEmpty {}),
            SchemaType::String { .. } => Body::StringType(ProtoEmpty {}),
            SchemaType::Record { fields, .. } => Body::RecordType(proto::RecordType {
                fields: fields.into_iter().map(Into::into).collect(),
            }),
            SchemaType::Variant { cases, .. } => Body::VariantType(proto::VariantType {
                cases: cases.into_iter().map(Into::into).collect(),
            }),
            SchemaType::Enum { cases, .. } => Body::EnumType(proto::EnumType { cases }),
            SchemaType::Flags { flags, .. } => Body::FlagsType(proto::FlagsType { flags }),
            SchemaType::Tuple { elements, .. } => Body::TupleType(proto::TupleType {
                elements: elements.into_iter().map(Into::into).collect(),
            }),
            SchemaType::List { element, .. } => Body::ListType(Box::new(proto::ListType {
                element: Some(box_to_proto(element)),
            })),
            SchemaType::FixedList {
                element, length, ..
            } => Body::FixedListType(Box::new(proto::FixedListType {
                element: Some(box_to_proto(element)),
                length,
            })),
            SchemaType::Map { key, value, .. } => Body::MapType(Box::new(proto::MapType {
                key: Some(box_to_proto(key)),
                value: Some(box_to_proto(value)),
            })),
            SchemaType::Option { inner, .. } => Body::OptionType(Box::new(proto::OptionType {
                inner: Some(box_to_proto(inner)),
            })),
            SchemaType::Result { spec, .. } => Body::ResultType(Box::new(proto::ResultType {
                ok: opt_box_to_proto(spec.ok),
                err: opt_box_to_proto(spec.err),
            })),
            SchemaType::Text { restrictions, .. } => Body::TextType(restrictions.into()),
            SchemaType::Binary { restrictions, .. } => Body::BinaryType(restrictions.into()),
            SchemaType::Path { spec, .. } => Body::PathType(spec.into()),
            SchemaType::Url { restrictions, .. } => Body::UrlType(restrictions.into()),
            SchemaType::Datetime { .. } => Body::DatetimeType(ProtoEmpty {}),
            SchemaType::Duration { .. } => Body::DurationType(ProtoEmpty {}),
            SchemaType::Quantity { spec, .. } => Body::QuantityType(spec.into()),
            SchemaType::Union { spec, .. } => Body::UnionType(spec.into()),
            SchemaType::Secret { spec, .. } => Body::SecretType(spec.into()),
            SchemaType::QuotaToken { spec, .. } => Body::QuotaTokenType(spec.into()),
            SchemaType::Future { inner, .. } => Body::FutureType(Box::new(proto::WasiStubType {
                element: opt_box_to_proto(inner),
            })),
            SchemaType::Stream { inner, .. } => Body::StreamType(Box::new(proto::WasiStubType {
                element: opt_box_to_proto(inner),
            })),
        };
        Self {
            metadata,
            body: Some(body),
        }
    }
}

impl TryFrom<proto::SchemaType> for SchemaType {
    type Error = String;

    fn try_from(value: proto::SchemaType) -> Result<Self, Self::Error> {
        let metadata = meta_from_proto(value.metadata)?;
        let body = value
            .body
            .ok_or_else(|| "Missing field: SchemaType.body".to_string())?;
        let result = match body {
            Body::RefType(id) => SchemaType::Ref {
                id: TypeId(id),
                metadata,
            },
            Body::BoolType(_) => SchemaType::Bool { metadata },
            Body::S8Type(_) => SchemaType::S8 { metadata },
            Body::S16Type(_) => SchemaType::S16 { metadata },
            Body::S32Type(_) => SchemaType::S32 { metadata },
            Body::S64Type(_) => SchemaType::S64 { metadata },
            Body::U8Type(_) => SchemaType::U8 { metadata },
            Body::U16Type(_) => SchemaType::U16 { metadata },
            Body::U32Type(_) => SchemaType::U32 { metadata },
            Body::U64Type(_) => SchemaType::U64 { metadata },
            Body::F32Type(_) => SchemaType::F32 { metadata },
            Body::F64Type(_) => SchemaType::F64 { metadata },
            Body::CharType(_) => SchemaType::Char { metadata },
            Body::StringType(_) => SchemaType::String { metadata },
            Body::RecordType(rt) => SchemaType::Record {
                fields: rt
                    .fields
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                metadata,
            },
            Body::VariantType(vt) => SchemaType::Variant {
                cases: vt
                    .cases
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                metadata,
            },
            Body::EnumType(et) => SchemaType::Enum {
                cases: et.cases,
                metadata,
            },
            Body::FlagsType(ft) => SchemaType::Flags {
                flags: ft.flags,
                metadata,
            },
            Body::TupleType(tt) => SchemaType::Tuple {
                elements: tt
                    .elements
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                metadata,
            },
            Body::ListType(lt) => SchemaType::List {
                element: req_box_from_proto(lt.element, "ListType.element")?,
                metadata,
            },
            Body::FixedListType(ft) => SchemaType::FixedList {
                element: req_box_from_proto(ft.element, "FixedListType.element")?,
                length: ft.length,
                metadata,
            },
            Body::MapType(mt) => SchemaType::Map {
                key: req_box_from_proto(mt.key, "MapType.key")?,
                value: req_box_from_proto(mt.value, "MapType.value")?,
                metadata,
            },
            Body::OptionType(ot) => SchemaType::Option {
                inner: req_box_from_proto(ot.inner, "OptionType.inner")?,
                metadata,
            },
            Body::ResultType(rt) => SchemaType::Result {
                spec: ResultSpec {
                    ok: opt_box_from_proto(rt.ok)?,
                    err: opt_box_from_proto(rt.err)?,
                },
                metadata,
            },
            Body::TextType(t) => SchemaType::Text {
                restrictions: t.into(),
                metadata,
            },
            Body::BinaryType(b) => SchemaType::Binary {
                restrictions: b.into(),
                metadata,
            },
            Body::PathType(p) => SchemaType::Path {
                spec: p.try_into()?,
                metadata,
            },
            Body::UrlType(u) => SchemaType::Url {
                restrictions: u.into(),
                metadata,
            },
            Body::DatetimeType(_) => SchemaType::Datetime { metadata },
            Body::DurationType(_) => SchemaType::Duration { metadata },
            Body::QuantityType(q) => SchemaType::Quantity {
                spec: q.into(),
                metadata,
            },
            Body::UnionType(u) => SchemaType::Union {
                spec: u.try_into()?,
                metadata,
            },
            Body::SecretType(s) => SchemaType::Secret {
                spec: s.into(),
                metadata,
            },
            Body::QuotaTokenType(q) => SchemaType::QuotaToken {
                spec: q.into(),
                metadata,
            },
            Body::FutureType(w) => SchemaType::Future {
                inner: opt_box_from_proto(w.element)?,
                metadata,
            },
            Body::StreamType(w) => SchemaType::Stream {
                inner: opt_box_from_proto(w.element)?,
                metadata,
            },
        };
        Ok(result)
    }
}

// --- structural composite helpers -------------------------------------------

impl From<NamedFieldType> for proto::NamedFieldType {
    fn from(value: NamedFieldType) -> Self {
        Self {
            name: value.name,
            body: Some(value.body.into()),
            metadata: optional_meta(value.metadata),
        }
    }
}

impl TryFrom<proto::NamedFieldType> for NamedFieldType {
    type Error = String;

    fn try_from(value: proto::NamedFieldType) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            body: value
                .body
                .ok_or_else(|| "Missing field: NamedFieldType.body".to_string())?
                .try_into()?,
            metadata: meta_from_proto(value.metadata)?,
        })
    }
}

impl From<VariantCaseType> for proto::VariantCaseType {
    fn from(value: VariantCaseType) -> Self {
        Self {
            name: value.name,
            payload: value.payload.map(Into::into),
            metadata: optional_meta(value.metadata),
        }
    }
}

impl TryFrom<proto::VariantCaseType> for VariantCaseType {
    type Error = String;

    fn try_from(value: proto::VariantCaseType) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            payload: value.payload.map(TryInto::try_into).transpose()?,
            metadata: meta_from_proto(value.metadata)?,
        })
    }
}

// --- rich scalar specs -------------------------------------------------------

impl From<TextRestrictions> for proto::TextRestrictions {
    fn from(value: TextRestrictions) -> Self {
        Self {
            languages: string_list_to_proto(value.languages),
            min_length: value.min_length,
            max_length: value.max_length,
            regex: value.regex,
        }
    }
}

impl From<proto::TextRestrictions> for TextRestrictions {
    fn from(value: proto::TextRestrictions) -> Self {
        Self {
            languages: string_list_from_proto(value.languages),
            min_length: value.min_length,
            max_length: value.max_length,
            regex: value.regex,
        }
    }
}

impl From<BinaryRestrictions> for proto::BinaryRestrictions {
    fn from(value: BinaryRestrictions) -> Self {
        Self {
            mime_types: string_list_to_proto(value.mime_types),
            min_bytes: value.min_bytes,
            max_bytes: value.max_bytes,
        }
    }
}

impl From<proto::BinaryRestrictions> for BinaryRestrictions {
    fn from(value: proto::BinaryRestrictions) -> Self {
        Self {
            mime_types: string_list_from_proto(value.mime_types),
            min_bytes: value.min_bytes,
            max_bytes: value.max_bytes,
        }
    }
}

impl From<PathDirection> for proto::PathDirection {
    fn from(value: PathDirection) -> Self {
        match value {
            PathDirection::Input => proto::PathDirection::Input,
            PathDirection::Output => proto::PathDirection::Output,
            PathDirection::InOut => proto::PathDirection::InOut,
        }
    }
}

impl TryFrom<proto::PathDirection> for PathDirection {
    type Error = String;

    fn try_from(value: proto::PathDirection) -> Result<Self, Self::Error> {
        match value {
            proto::PathDirection::Input => Ok(PathDirection::Input),
            proto::PathDirection::Output => Ok(PathDirection::Output),
            proto::PathDirection::InOut => Ok(PathDirection::InOut),
            proto::PathDirection::Unspecified => Err("Unspecified PathDirection".to_string()),
        }
    }
}

impl From<PathKind> for proto::PathKind {
    fn from(value: PathKind) -> Self {
        match value {
            PathKind::File => proto::PathKind::File,
            PathKind::Directory => proto::PathKind::Directory,
            PathKind::Any => proto::PathKind::Any,
        }
    }
}

impl TryFrom<proto::PathKind> for PathKind {
    type Error = String;

    fn try_from(value: proto::PathKind) -> Result<Self, Self::Error> {
        match value {
            proto::PathKind::File => Ok(PathKind::File),
            proto::PathKind::Directory => Ok(PathKind::Directory),
            proto::PathKind::Any => Ok(PathKind::Any),
            proto::PathKind::Unspecified => Err("Unspecified PathKind".to_string()),
        }
    }
}

impl From<PathSpec> for proto::PathSpec {
    fn from(value: PathSpec) -> Self {
        Self {
            direction: proto::PathDirection::from(value.direction) as i32,
            kind: proto::PathKind::from(value.kind) as i32,
            allowed_mime_types: string_list_to_proto(value.allowed_mime_types),
            allowed_extensions: string_list_to_proto(value.allowed_extensions),
        }
    }
}

impl TryFrom<proto::PathSpec> for PathSpec {
    type Error = String;

    fn try_from(value: proto::PathSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            direction: value.direction().try_into()?,
            kind: value.kind().try_into()?,
            allowed_mime_types: string_list_from_proto(value.allowed_mime_types),
            allowed_extensions: string_list_from_proto(value.allowed_extensions),
        })
    }
}

impl From<UrlRestrictions> for proto::UrlRestrictions {
    fn from(value: UrlRestrictions) -> Self {
        Self {
            allowed_schemes: string_list_to_proto(value.allowed_schemes),
            allowed_hosts: string_list_to_proto(value.allowed_hosts),
        }
    }
}

impl From<proto::UrlRestrictions> for UrlRestrictions {
    fn from(value: proto::UrlRestrictions) -> Self {
        Self {
            allowed_schemes: string_list_from_proto(value.allowed_schemes),
            allowed_hosts: string_list_from_proto(value.allowed_hosts),
        }
    }
}

impl From<QuantityValue> for proto::QuantityValue {
    fn from(value: QuantityValue) -> Self {
        Self {
            mantissa: value.mantissa,
            scale: value.scale,
            unit: value.unit,
        }
    }
}

impl From<proto::QuantityValue> for QuantityValue {
    fn from(value: proto::QuantityValue) -> Self {
        Self {
            mantissa: value.mantissa,
            scale: value.scale,
            unit: value.unit,
        }
    }
}

impl From<QuantitySpec> for proto::QuantitySpec {
    fn from(value: QuantitySpec) -> Self {
        Self {
            base_unit: value.base_unit,
            allowed_suffixes: value.allowed_suffixes,
            min: value.min.map(Into::into),
            max: value.max.map(Into::into),
        }
    }
}

impl From<proto::QuantitySpec> for QuantitySpec {
    fn from(value: proto::QuantitySpec) -> Self {
        Self {
            base_unit: value.base_unit,
            allowed_suffixes: value.allowed_suffixes,
            min: value.min.map(Into::into),
            max: value.max.map(Into::into),
        }
    }
}

impl From<SecretSpec> for proto::SecretSpec {
    fn from(value: SecretSpec) -> Self {
        Self {
            category: value.category,
        }
    }
}

impl From<proto::SecretSpec> for SecretSpec {
    fn from(value: proto::SecretSpec) -> Self {
        Self {
            category: value.category,
        }
    }
}

impl From<QuotaTokenSpec> for proto::QuotaTokenSpec {
    fn from(value: QuotaTokenSpec) -> Self {
        Self {
            resource_name: value.resource_name,
        }
    }
}

impl From<proto::QuotaTokenSpec> for QuotaTokenSpec {
    fn from(value: proto::QuotaTokenSpec) -> Self {
        Self {
            resource_name: value.resource_name,
        }
    }
}

// --- discriminated union -----------------------------------------------------

impl From<UnionSpec> for proto::UnionSpec {
    fn from(value: UnionSpec) -> Self {
        Self {
            branches: value.branches.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::UnionSpec> for UnionSpec {
    type Error = String;

    fn try_from(value: proto::UnionSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            branches: value
                .branches
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<UnionBranch> for proto::UnionBranch {
    fn from(value: UnionBranch) -> Self {
        Self {
            tag: value.tag,
            body: Some(value.body.into()),
            discriminator: Some(value.discriminator.into()),
            metadata: optional_meta(value.metadata),
        }
    }
}

impl TryFrom<proto::UnionBranch> for UnionBranch {
    type Error = String;

    fn try_from(value: proto::UnionBranch) -> Result<Self, Self::Error> {
        Ok(Self {
            tag: value.tag,
            body: value
                .body
                .ok_or_else(|| "Missing field: UnionBranch.body".to_string())?
                .try_into()?,
            discriminator: value
                .discriminator
                .ok_or_else(|| "Missing field: UnionBranch.discriminator".to_string())?
                .try_into()?,
            metadata: meta_from_proto(value.metadata)?,
        })
    }
}

impl From<DiscriminatorRule> for proto::DiscriminatorRule {
    fn from(value: DiscriminatorRule) -> Self {
        use proto::discriminator_rule::Rule;
        let rule = match value {
            DiscriminatorRule::Prefix { prefix } => Rule::Prefix(prefix),
            DiscriminatorRule::Suffix { suffix } => Rule::Suffix(suffix),
            DiscriminatorRule::Contains { substring } => Rule::Contains(substring),
            DiscriminatorRule::Regex { regex } => Rule::Regex(regex),
            DiscriminatorRule::FieldEquals(fd) => Rule::FieldEquals(fd.into()),
            DiscriminatorRule::FieldAbsent { field_name } => Rule::FieldAbsent(field_name),
        };
        Self { rule: Some(rule) }
    }
}

impl TryFrom<proto::DiscriminatorRule> for DiscriminatorRule {
    type Error = String;

    fn try_from(value: proto::DiscriminatorRule) -> Result<Self, Self::Error> {
        use proto::discriminator_rule::Rule;
        match value.rule {
            Some(Rule::Prefix(prefix)) => Ok(DiscriminatorRule::Prefix { prefix }),
            Some(Rule::Suffix(suffix)) => Ok(DiscriminatorRule::Suffix { suffix }),
            Some(Rule::Contains(substring)) => Ok(DiscriminatorRule::Contains { substring }),
            Some(Rule::Regex(regex)) => Ok(DiscriminatorRule::Regex { regex }),
            Some(Rule::FieldEquals(fd)) => Ok(DiscriminatorRule::FieldEquals(fd.into())),
            Some(Rule::FieldAbsent(field_name)) => {
                Ok(DiscriminatorRule::FieldAbsent { field_name })
            }
            None => Err("Missing field: DiscriminatorRule.rule".to_string()),
        }
    }
}

impl From<FieldDiscriminator> for proto::FieldDiscriminator {
    fn from(value: FieldDiscriminator) -> Self {
        Self {
            field_name: value.field_name,
            literal: value.literal,
        }
    }
}

impl From<proto::FieldDiscriminator> for FieldDiscriminator {
    fn from(value: proto::FieldDiscriminator) -> Self {
        Self {
            field_name: value.field_name,
            literal: value.literal,
        }
    }
}

// --- SchemaGraph -------------------------------------------------------------

impl From<SchemaGraph> for proto::SchemaGraph {
    fn from(value: SchemaGraph) -> Self {
        Self {
            defs: value.defs.into_iter().map(Into::into).collect(),
            root: Some(value.root.into()),
        }
    }
}

impl TryFrom<proto::SchemaGraph> for SchemaGraph {
    type Error = String;

    fn try_from(value: proto::SchemaGraph) -> Result<Self, Self::Error> {
        Ok(Self {
            defs: value
                .defs
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            root: value
                .root
                .ok_or_else(|| "Missing field: SchemaGraph.root".to_string())?
                .try_into()?,
        })
    }
}

impl From<SchemaTypeDef> for proto::SchemaTypeDef {
    fn from(value: SchemaTypeDef) -> Self {
        Self {
            id: value.id.0,
            name: value.name,
            body: Some(value.body.into()),
        }
    }
}

impl TryFrom<proto::SchemaTypeDef> for SchemaTypeDef {
    type Error = String;

    fn try_from(value: proto::SchemaTypeDef) -> Result<Self, Self::Error> {
        Ok(Self {
            id: TypeId(value.id),
            name: value.name,
            body: value
                .body
                .ok_or_else(|| "Missing field: SchemaTypeDef.body".to_string())?
                .try_into()?,
        })
    }
}

// --- agent input/output layer ------------------------------------------------

impl From<InputSchema> for proto::InputSchema {
    fn from(value: InputSchema) -> Self {
        match value {
            InputSchema::Parameters(fields) => Self {
                parameters: fields.into_iter().map(Into::into).collect(),
            },
        }
    }
}

impl TryFrom<proto::InputSchema> for InputSchema {
    type Error = String;

    fn try_from(value: proto::InputSchema) -> Result<Self, Self::Error> {
        Ok(InputSchema::Parameters(
            value
                .parameters
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        ))
    }
}

impl From<OutputSchema> for proto::OutputSchema {
    fn from(value: OutputSchema) -> Self {
        use proto::output_schema::Output;
        let output = match value {
            OutputSchema::Unit => Output::Unit(ProtoEmpty {}),
            OutputSchema::Single(ty) => Output::Single((*ty).into()),
        };
        Self {
            output: Some(output),
        }
    }
}

impl TryFrom<proto::OutputSchema> for OutputSchema {
    type Error = String;

    fn try_from(value: proto::OutputSchema) -> Result<Self, Self::Error> {
        use proto::output_schema::Output;
        match value.output {
            Some(Output::Unit(_)) => Ok(OutputSchema::Unit),
            Some(Output::Single(ty)) => Ok(OutputSchema::Single(Box::new(ty.try_into()?))),
            None => Err("Missing field: OutputSchema.output".to_string()),
        }
    }
}

impl From<NamedField> for proto::NamedField {
    fn from(value: NamedField) -> Self {
        Self {
            name: value.name,
            source: Some(value.source.into()),
            schema: Some(value.schema.into()),
            metadata: optional_meta(value.metadata),
        }
    }
}

impl TryFrom<proto::NamedField> for NamedField {
    type Error = String;

    fn try_from(value: proto::NamedField) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            source: value
                .source
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
            schema: value
                .schema
                .ok_or_else(|| "Missing field: NamedField.schema".to_string())?
                .try_into()?,
            metadata: meta_from_proto(value.metadata)?,
        })
    }
}

impl From<FieldSource> for proto::FieldSource {
    fn from(value: FieldSource) -> Self {
        use proto::field_source::Source;
        let source = match value {
            FieldSource::UserSupplied => Source::UserSupplied(ProtoEmpty {}),
            FieldSource::AutoInjected(kind) => {
                Source::AutoInjected(proto::AutoInjectedKind::from(kind) as i32)
            }
        };
        Self {
            source: Some(source),
        }
    }
}

impl TryFrom<proto::FieldSource> for FieldSource {
    type Error = String;

    fn try_from(value: proto::FieldSource) -> Result<Self, Self::Error> {
        use proto::field_source::Source;
        match value.source {
            Some(Source::UserSupplied(_)) => Ok(FieldSource::UserSupplied),
            Some(Source::AutoInjected(kind)) => {
                let kind = proto::AutoInjectedKind::try_from(kind)
                    .map_err(|_| format!("Invalid AutoInjectedKind: {kind}"))?;
                Ok(FieldSource::AutoInjected(kind.try_into()?))
            }
            None => Err("Missing field: FieldSource.source".to_string()),
        }
    }
}

impl From<AutoInjectedKind> for proto::AutoInjectedKind {
    fn from(value: AutoInjectedKind) -> Self {
        match value {
            AutoInjectedKind::Principal => proto::AutoInjectedKind::Principal,
        }
    }
}

impl TryFrom<proto::AutoInjectedKind> for AutoInjectedKind {
    type Error = String;

    fn try_from(value: proto::AutoInjectedKind) -> Result<Self, Self::Error> {
        match value {
            proto::AutoInjectedKind::Principal => Ok(AutoInjectedKind::Principal),
            proto::AutoInjectedKind::Unspecified => Err("Unspecified AutoInjectedKind".to_string()),
        }
    }
}

// --- agent type layer --------------------------------------------------------

impl From<AgentConstructorSchema> for proto::AgentConstructorSchema {
    fn from(value: AgentConstructorSchema) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: Some(value.input_schema.into()),
        }
    }
}

impl TryFrom<proto::AgentConstructorSchema> for AgentConstructorSchema {
    type Error = String;

    fn try_from(value: proto::AgentConstructorSchema) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value
                .input_schema
                .ok_or_else(|| "Missing field: AgentConstructorSchema.input_schema".to_string())?
                .try_into()?,
        })
    }
}

impl From<AgentMethodSchema> for proto::AgentMethodSchema {
    fn from(value: AgentMethodSchema) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: Some(value.input_schema.into()),
            output_schema: Some(value.output_schema.into()),
            http_endpoint: value.http_endpoint.into_iter().map(Into::into).collect(),
            read_only: value.read_only.map(Into::into),
        }
    }
}

impl TryFrom<proto::AgentMethodSchema> for AgentMethodSchema {
    type Error = String;

    fn try_from(value: proto::AgentMethodSchema) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value
                .input_schema
                .ok_or_else(|| "Missing field: AgentMethodSchema.input_schema".to_string())?
                .try_into()?,
            output_schema: value
                .output_schema
                .ok_or_else(|| "Missing field: AgentMethodSchema.output_schema".to_string())?
                .try_into()?,
            http_endpoint: value
                .http_endpoint
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            read_only: value.read_only.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<AgentDependencySchema> for proto::AgentDependencySchema {
    fn from(value: AgentDependencySchema) -> Self {
        Self {
            type_name: value.type_name,
            description: value.description,
            schema: Some(value.schema.into()),
            constructor: Some(value.constructor.into()),
            methods: value.methods.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::AgentDependencySchema> for AgentDependencySchema {
    type Error = String;

    fn try_from(value: proto::AgentDependencySchema) -> Result<Self, Self::Error> {
        Ok(Self {
            type_name: value.type_name,
            description: value.description,
            schema: value
                .schema
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_else(SchemaGraph::empty),
            constructor: value
                .constructor
                .ok_or_else(|| "Missing field: AgentDependencySchema.constructor".to_string())?
                .try_into()?,
            methods: value
                .methods
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<AgentTypeSchema> for proto::AgentTypeSchema {
    fn from(value: AgentTypeSchema) -> Self {
        Self {
            type_name: value.type_name.0,
            description: value.description,
            source_language: value.source_language,
            schema: Some(value.schema.into()),
            constructor: Some(value.constructor.into()),
            methods: value.methods.into_iter().map(Into::into).collect(),
            dependencies: value.dependencies.into_iter().map(Into::into).collect(),
            mode: golem_api_grpc::proto::golem::component::AgentMode::from(value.mode) as i32,
            http_mount: value.http_mount.map(Into::into),
            snapshotting: Some(value.snapshotting.into()),
            config: value.config.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::AgentTypeSchema> for AgentTypeSchema {
    type Error = String;

    fn try_from(value: proto::AgentTypeSchema) -> Result<Self, Self::Error> {
        let mode = value.mode().into();
        Ok(Self {
            type_name: AgentTypeName(value.type_name),
            description: value.description,
            source_language: value.source_language,
            schema: value
                .schema
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_else(SchemaGraph::empty),
            constructor: value
                .constructor
                .ok_or_else(|| "Missing field: AgentTypeSchema.constructor".to_string())?
                .try_into()?,
            methods: value
                .methods
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            dependencies: value
                .dependencies
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            mode,
            http_mount: value.http_mount.map(TryInto::try_into).transpose()?,
            snapshotting: value
                .snapshotting
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or(Snapshotting::Disabled(ModelEmpty {})),
            config: value
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

// --- registered agent type schema (golem.registry package) ------------------

impl From<RegisteredAgentTypeSchema>
    for golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema
{
    fn from(value: RegisteredAgentTypeSchema) -> Self {
        Self {
            agent_type: Some(value.agent_type.into()),
            implemented_by: Some(value.implemented_by.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema>
    for RegisteredAgentTypeSchema
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_type: value
                .agent_type
                .ok_or_else(|| "Missing field: RegisteredAgentTypeSchema.agent_type".to_string())?
                .try_into()?,
            implemented_by: value
                .implemented_by
                .ok_or_else(|| {
                    "Missing field: RegisteredAgentTypeSchema.implemented_by".to_string()
                })?
                .try_into()?,
        })
    }
}
