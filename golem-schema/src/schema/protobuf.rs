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

//! Conversions between the recursive in-memory schema model and its protobuf
//! mirror in the `golem.schema` package.

use crate::model::EnvironmentId;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType,
    SecretSpec, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{
    BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
    SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
};
use chrono::{DateTime, TimeZone, Utc};
use golem_api_grpc::proto::golem::common::Empty as ProtoEmpty;
use golem_api_grpc::proto::golem::schema as proto;
use golem_api_grpc::proto::golem::schema::result_value::Result as ResultBody;
use golem_api_grpc::proto::golem::schema::schema_type::Body;
use golem_api_grpc::proto::golem::schema::schema_value::Value as ValueBody;

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

// --- value-side helpers ------------------------------------------------------

fn value_to_boxed_proto(value: SchemaValue) -> Box<proto::SchemaValue> {
    Box::new(value.into())
}

fn opt_box_value_from_proto(
    value: Option<Box<proto::SchemaValue>>,
) -> Result<Option<Box<SchemaValue>>, String> {
    value
        .map(|b| Ok::<_, String>(Box::new((*b).try_into()?)))
        .transpose()
}

fn req_box_value_from_proto(
    value: Option<Box<proto::SchemaValue>>,
    ctx: &str,
) -> Result<Box<SchemaValue>, String> {
    let b = value.ok_or_else(|| format!("Missing field: {ctx}"))?;
    Ok(Box::new((*b).try_into()?))
}

fn datetime_to_proto(dt: DateTime<Utc>) -> proto::DatetimeValue {
    proto::DatetimeValue {
        seconds: dt.timestamp(),
        nanoseconds: dt.timestamp_subsec_nanos(),
    }
}

fn datetime_from_proto(d: proto::DatetimeValue) -> Result<DateTime<Utc>, String> {
    Utc.timestamp_opt(d.seconds, d.nanoseconds)
        .single()
        .ok_or_else(|| format!("Invalid datetime: {}s {}ns", d.seconds, d.nanoseconds))
}

fn map_entry_from_proto(e: proto::MapEntry) -> Result<(SchemaValue, SchemaValue), String> {
    let key = e
        .key
        .ok_or_else(|| "Missing field: MapEntry.key".to_string())?
        .try_into()?;
    let value = e
        .value
        .ok_or_else(|| "Missing field: MapEntry.value".to_string())?
        .try_into()?;
    Ok((key, value))
}

fn result_value_from_proto(rv: proto::ResultValue) -> Result<ResultValuePayload, String> {
    let r = rv
        .result
        .ok_or_else(|| "Missing field: ResultValue.result".to_string())?;
    Ok(match r {
        ResultBody::Ok(v) => ResultValuePayload::Ok {
            value: Some(Box::new((*v).try_into()?)),
        },
        ResultBody::Err(v) => ResultValuePayload::Err {
            value: Some(Box::new((*v).try_into()?)),
        },
        ResultBody::OkUnit(_) => ResultValuePayload::Ok { value: None },
        ResultBody::ErrUnit(_) => ResultValuePayload::Err { value: None },
    })
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
            Role::UnstructuredText => proto::role::Value::UnstructuredText(ProtoEmpty {}),
            Role::UnstructuredBinary => proto::role::Value::UnstructuredBinary(ProtoEmpty {}),
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
            Some(proto::role::Value::UnstructuredText(_)) => Ok(Role::UnstructuredText),
            Some(proto::role::Value::UnstructuredBinary(_)) => Ok(Role::UnstructuredBinary),
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
            SchemaType::Secret { spec, .. } => Body::SecretType(Box::new(spec.into())),
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
                spec: (*s).try_into()?,
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
            inner: Some(box_to_proto(value.inner)),
        }
    }
}

impl TryFrom<proto::SecretSpec> for SecretSpec {
    type Error = String;

    fn try_from(value: proto::SecretSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            category: value.category,
            inner: match value.inner {
                Some(inner) => box_from_proto(inner)?,
                None => Box::new(SchemaType::string()),
            },
        })
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

// --- SchemaValue / TypedSchemaValue ------------------------------------------

impl From<SchemaValue> for proto::SchemaValue {
    fn from(value: SchemaValue) -> Self {
        let body = match value {
            SchemaValue::Bool(b) => ValueBody::BoolValue(b),
            SchemaValue::S8(v) => ValueBody::S8Value(v as i32),
            SchemaValue::S16(v) => ValueBody::S16Value(v as i32),
            SchemaValue::S32(v) => ValueBody::S32Value(v),
            SchemaValue::S64(v) => ValueBody::S64Value(v),
            SchemaValue::U8(v) => ValueBody::U8Value(v as u32),
            SchemaValue::U16(v) => ValueBody::U16Value(v as u32),
            SchemaValue::U32(v) => ValueBody::U32Value(v),
            SchemaValue::U64(v) => ValueBody::U64Value(v),
            SchemaValue::F32(v) => ValueBody::F32Value(v),
            SchemaValue::F64(v) => ValueBody::F64Value(v),
            SchemaValue::Char(c) => ValueBody::CharValue(c as u32),
            SchemaValue::String(s) => ValueBody::StringValue(s),
            SchemaValue::Record { fields } => ValueBody::RecordValue(proto::RecordValue {
                fields: fields.into_iter().map(Into::into).collect(),
            }),
            SchemaValue::Variant(p) => ValueBody::VariantValue(Box::new(proto::VariantValue {
                case: p.case,
                payload: p.payload.map(|value| value_to_boxed_proto(*value)),
            })),
            SchemaValue::Enum { case } => ValueBody::EnumValue(case),
            SchemaValue::Flags { bits } => ValueBody::FlagsValue(proto::FlagsValue { bits }),
            SchemaValue::Tuple { elements } => ValueBody::TupleValue(proto::TupleValue {
                elements: elements.into_iter().map(Into::into).collect(),
            }),
            SchemaValue::List { elements } => ValueBody::ListValue(proto::ListValue {
                elements: elements.into_iter().map(Into::into).collect(),
            }),
            SchemaValue::FixedList { elements } => {
                ValueBody::FixedListValue(proto::FixedListValue {
                    elements: elements.into_iter().map(Into::into).collect(),
                })
            }
            SchemaValue::Map { entries } => ValueBody::MapValue(proto::MapValue {
                entries: entries
                    .into_iter()
                    .map(|(k, v)| proto::MapEntry {
                        key: Some(k.into()),
                        value: Some(v.into()),
                    })
                    .collect(),
            }),
            SchemaValue::Option { inner } => ValueBody::OptionValue(Box::new(proto::OptionValue {
                inner: inner.map(|value| value_to_boxed_proto(*value)),
            })),
            SchemaValue::Result(r) => ValueBody::ResultValue(Box::new(proto::ResultValue {
                result: Some(match r {
                    ResultValuePayload::Ok { value } => match value {
                        Some(v) => ResultBody::Ok(value_to_boxed_proto(*v)),
                        None => ResultBody::OkUnit(ProtoEmpty {}),
                    },
                    ResultValuePayload::Err { value } => match value {
                        Some(v) => ResultBody::Err(value_to_boxed_proto(*v)),
                        None => ResultBody::ErrUnit(ProtoEmpty {}),
                    },
                }),
            })),
            SchemaValue::Text(t) => ValueBody::TextValue(proto::TextValue {
                text: t.text,
                language: t.language,
            }),
            SchemaValue::Binary(b) => ValueBody::BinaryValue(proto::BinaryValue {
                bytes: b.bytes,
                mime_type: b.mime_type,
            }),
            SchemaValue::Path { path } => ValueBody::PathValue(path),
            SchemaValue::Url { url } => ValueBody::UrlValue(url),
            SchemaValue::Datetime { value } => ValueBody::DatetimeValue(datetime_to_proto(value)),
            SchemaValue::Duration(d) => ValueBody::DurationValue(proto::DurationValue {
                nanoseconds: d.nanoseconds,
            }),
            SchemaValue::Quantity(q) => ValueBody::QuantityValue(q.into()),
            SchemaValue::Union(u) => ValueBody::UnionValue(Box::new(proto::UnionValue {
                tag: u.tag,
                body: Some(value_to_boxed_proto(*u.body)),
            })),
            SchemaValue::Secret(s) => ValueBody::SecretValue(proto::SecretValue {
                secret_id: Some(s.secret_id.into()),
                config_key: s.config_key.map(|items| proto::StringList { items }),
                version: s.version,
                resolved_at: Some(datetime_to_proto(s.resolved_at)),
                category: s.category,
            }),
            SchemaValue::QuotaToken(q) => ValueBody::QuotaTokenValue(proto::QuotaTokenValue {
                environment_id: Some(q.environment_id.uuid.into()),
                resource_name: q.resource_name,
                expected_use: q.expected_use,
                last_credit: q.last_credit,
                last_credit_at: Some(datetime_to_proto(q.last_credit_at)),
            }),
        };
        Self { value: Some(body) }
    }
}

impl TryFrom<proto::SchemaValue> for SchemaValue {
    type Error = String;

    fn try_from(value: proto::SchemaValue) -> Result<Self, Self::Error> {
        let body = value
            .value
            .ok_or_else(|| "Missing field: SchemaValue.value".to_string())?;
        let result = match body {
            ValueBody::BoolValue(b) => SchemaValue::Bool(b),
            ValueBody::S8Value(v) => {
                SchemaValue::S8(i8::try_from(v).map_err(|_| format!("s8 out of range: {v}"))?)
            }
            ValueBody::S16Value(v) => {
                SchemaValue::S16(i16::try_from(v).map_err(|_| format!("s16 out of range: {v}"))?)
            }
            ValueBody::S32Value(v) => SchemaValue::S32(v),
            ValueBody::S64Value(v) => SchemaValue::S64(v),
            ValueBody::U8Value(v) => {
                SchemaValue::U8(u8::try_from(v).map_err(|_| format!("u8 out of range: {v}"))?)
            }
            ValueBody::U16Value(v) => {
                SchemaValue::U16(u16::try_from(v).map_err(|_| format!("u16 out of range: {v}"))?)
            }
            ValueBody::U32Value(v) => SchemaValue::U32(v),
            ValueBody::U64Value(v) => SchemaValue::U64(v),
            ValueBody::F32Value(v) => SchemaValue::F32(v),
            ValueBody::F64Value(v) => SchemaValue::F64(v),
            ValueBody::CharValue(v) => {
                SchemaValue::Char(char::from_u32(v).ok_or_else(|| format!("invalid char: {v}"))?)
            }
            ValueBody::StringValue(s) => SchemaValue::String(s),
            ValueBody::RecordValue(rv) => SchemaValue::Record {
                fields: rv
                    .fields
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            },
            ValueBody::VariantValue(vv) => {
                let vv = *vv;
                SchemaValue::Variant(VariantValuePayload {
                    case: vv.case,
                    payload: opt_box_value_from_proto(vv.payload)?,
                })
            }
            ValueBody::EnumValue(case) => SchemaValue::Enum { case },
            ValueBody::FlagsValue(fv) => SchemaValue::Flags { bits: fv.bits },
            ValueBody::TupleValue(tv) => SchemaValue::Tuple {
                elements: tv
                    .elements
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            },
            ValueBody::ListValue(lv) => SchemaValue::List {
                elements: lv
                    .elements
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            },
            ValueBody::FixedListValue(fv) => SchemaValue::FixedList {
                elements: fv
                    .elements
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            },
            ValueBody::MapValue(mv) => SchemaValue::Map {
                entries: mv
                    .entries
                    .into_iter()
                    .map(map_entry_from_proto)
                    .collect::<Result<_, _>>()?,
            },
            ValueBody::OptionValue(ov) => SchemaValue::Option {
                inner: opt_box_value_from_proto(ov.inner)?,
            },
            ValueBody::ResultValue(rv) => SchemaValue::Result(result_value_from_proto(*rv)?),
            ValueBody::TextValue(t) => SchemaValue::Text(TextValuePayload {
                text: t.text,
                language: t.language,
            }),
            ValueBody::BinaryValue(b) => SchemaValue::Binary(BinaryValuePayload {
                bytes: b.bytes,
                mime_type: b.mime_type,
            }),
            ValueBody::PathValue(p) => SchemaValue::Path { path: p },
            ValueBody::UrlValue(u) => SchemaValue::Url { url: u },
            ValueBody::DatetimeValue(d) => SchemaValue::Datetime {
                value: datetime_from_proto(d)?,
            },
            ValueBody::DurationValue(d) => SchemaValue::Duration(DurationValuePayload {
                nanoseconds: d.nanoseconds,
            }),
            ValueBody::QuantityValue(q) => SchemaValue::Quantity(q.into()),
            ValueBody::UnionValue(uv) => {
                let uv = *uv;
                SchemaValue::Union(UnionValuePayload {
                    tag: uv.tag,
                    body: req_box_value_from_proto(uv.body, "UnionValue.body")?,
                })
            }
            ValueBody::SecretValue(s) => SchemaValue::Secret(SecretValuePayload {
                secret_id: s
                    .secret_id
                    .ok_or_else(|| "Missing field: SecretValue.secret_id".to_string())?
                    .into(),
                config_key: s.config_key.map(|sl| sl.items),
                version: s.version,
                resolved_at: datetime_from_proto(
                    s.resolved_at
                        .ok_or_else(|| "Missing field: SecretValue.resolved_at".to_string())?,
                )?,
                category: s.category,
            }),
            ValueBody::QuotaTokenValue(q) => SchemaValue::QuotaToken(QuotaTokenValuePayload {
                environment_id: EnvironmentId::new(
                    q.environment_id
                        .ok_or_else(|| "Missing field: QuotaTokenValue.environment_id".to_string())?
                        .into(),
                ),
                resource_name: q.resource_name,
                expected_use: q.expected_use,
                last_credit: q.last_credit,
                last_credit_at: datetime_from_proto(
                    q.last_credit_at.ok_or_else(|| {
                        "Missing field: QuotaTokenValue.last_credit_at".to_string()
                    })?,
                )?,
            }),
        };
        Ok(result)
    }
}

impl From<TypedSchemaValue> for proto::TypedSchemaValue {
    fn from(value: TypedSchemaValue) -> Self {
        let (graph, val) = value.into_parts();
        Self {
            graph: Some(graph.into()),
            value: Some(val.into()),
        }
    }
}

impl TryFrom<proto::TypedSchemaValue> for TypedSchemaValue {
    type Error = String;

    fn try_from(value: proto::TypedSchemaValue) -> Result<Self, Self::Error> {
        let graph = value
            .graph
            .ok_or_else(|| "Missing field: TypedSchemaValue.graph".to_string())?
            .try_into()?;
        let val = value
            .value
            .ok_or_else(|| "Missing field: TypedSchemaValue.value".to_string())?
            .try_into()?;
        Ok(TypedSchemaValue::new(graph, val))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn secret_ref_value_round_trip_preserves_opaque_identifier_legacy_proto_secret_spec_defaults_inner()
     {
        let proto = proto::SchemaType {
            metadata: None,
            body: Some(Body::SecretType(Box::new(proto::SecretSpec {
                category: Some("api-key".to_string()),
                inner: None,
            }))),
        };

        let decoded = SchemaType::try_from(proto).expect("legacy SecretSpec without inner");

        assert_eq!(
            decoded,
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("api-key".to_string()),
            })
        );
    }
}

// --- agent input/output layer ------------------------------------------------
