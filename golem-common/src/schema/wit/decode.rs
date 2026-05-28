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
use golem_wasm::golem_core_2_0_x::types as wire;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};

/// Errors that can occur while decoding the flat wire form into the
/// recursive in-memory representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TypeNodeIndexOutOfRange(wire::TypeNodeIndex),
    ValueNodeIndexOutOfRange(wire::ValueNodeIndex),
    DefIndexOutOfRange(wire::DefIndex),
    DuplicateTypeId(TypeId),
    InvalidDatetime {
        seconds: i64,
        nanoseconds: u32,
    },
    /// Cycle detected in the flat type-node list that does not pass through a
    /// `ref-type` (which is the only valid recursion form).
    CyclicTypeWithoutRef,
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::TypeNodeIndexOutOfRange(i) => {
                write!(f, "type-node index out of range: {i}")
            }
            DecodeError::ValueNodeIndexOutOfRange(i) => {
                write!(f, "value-node index out of range: {i}")
            }
            DecodeError::DefIndexOutOfRange(i) => write!(f, "def index out of range: {i}"),
            DecodeError::DuplicateTypeId(id) => write!(f, "duplicate type id: {id}"),
            DecodeError::InvalidDatetime {
                seconds,
                nanoseconds,
            } => write!(f, "invalid datetime: {seconds}s {nanoseconds}ns"),
            DecodeError::CyclicTypeWithoutRef => write!(
                f,
                "cyclic type detected that does not pass through a `ref-type` indirection"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn decode_graph(wire_graph: &wire::SchemaGraph) -> Result<SchemaGraph, DecodeError> {
    let ctx = GraphCtx::new(wire_graph)?;
    let defs = wire_graph
        .defs
        .iter()
        .map(|d| {
            let body = ctx.decode_type(d.body, &mut HashSet::new())?;
            Ok(SchemaTypeDef {
                id: TypeId(d.id.clone()),
                name: d.name.clone(),
                metadata: decode_metadata(&d.metadata),
                body,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = ctx.decode_type(wire_graph.root, &mut HashSet::new())?;
    Ok(SchemaGraph { defs, root })
}

pub fn decode_value(wire_tree: &wire::SchemaValueTree) -> Result<SchemaValue, DecodeError> {
    decode_value_at(wire_tree, wire_tree.root, &mut HashSet::new())
}

pub fn decode_typed(wire_typed: &wire::TypedSchemaValue) -> Result<TypedSchemaValue, DecodeError> {
    let graph = decode_graph(&wire_typed.graph)?;
    let value = decode_value(&wire_typed.value)?;
    Ok(TypedSchemaValue::new(graph, value))
}

struct GraphCtx<'a> {
    wire: &'a wire::SchemaGraph,
}

impl<'a> GraphCtx<'a> {
    fn new(wire_graph: &'a wire::SchemaGraph) -> Result<Self, DecodeError> {
        let mut seen: HashSet<&str> = HashSet::with_capacity(wire_graph.defs.len());
        for def in &wire_graph.defs {
            if !seen.insert(def.id.as_str()) {
                return Err(DecodeError::DuplicateTypeId(TypeId(def.id.clone())));
            }
        }
        Ok(GraphCtx { wire: wire_graph })
    }

    fn decode_type(
        &self,
        idx: wire::TypeNodeIndex,
        visiting: &mut HashSet<wire::TypeNodeIndex>,
    ) -> Result<SchemaType, DecodeError> {
        let node = self
            .wire
            .type_nodes
            .get(usize_index(idx)?)
            .ok_or(DecodeError::TypeNodeIndexOutOfRange(idx))?;
        if !visiting.insert(idx) {
            // Re-entered this node without going through a `ref-type` — invalid.
            return Err(DecodeError::CyclicTypeWithoutRef);
        }
        let result = self.decode_node(node, visiting);
        visiting.remove(&idx);
        result
    }

    fn decode_node(
        &self,
        node: &wire::SchemaTypeNode,
        visiting: &mut HashSet<wire::TypeNodeIndex>,
    ) -> Result<SchemaType, DecodeError> {
        let out = match node {
            wire::SchemaTypeNode::RefType(def_idx) => {
                let def = self
                    .wire
                    .defs
                    .get(usize_index(*def_idx)?)
                    .ok_or(DecodeError::DefIndexOutOfRange(*def_idx))?;
                SchemaType::Ref(TypeId(def.id.clone()))
            }
            wire::SchemaTypeNode::BoolType => SchemaType::Bool,
            wire::SchemaTypeNode::S8Type => SchemaType::S8,
            wire::SchemaTypeNode::S16Type => SchemaType::S16,
            wire::SchemaTypeNode::S32Type => SchemaType::S32,
            wire::SchemaTypeNode::S64Type => SchemaType::S64,
            wire::SchemaTypeNode::U8Type => SchemaType::U8,
            wire::SchemaTypeNode::U16Type => SchemaType::U16,
            wire::SchemaTypeNode::U32Type => SchemaType::U32,
            wire::SchemaTypeNode::U64Type => SchemaType::U64,
            wire::SchemaTypeNode::F32Type => SchemaType::F32,
            wire::SchemaTypeNode::F64Type => SchemaType::F64,
            wire::SchemaTypeNode::CharType => SchemaType::Char,
            wire::SchemaTypeNode::StringType => SchemaType::String,
            wire::SchemaTypeNode::RecordType(fields) => {
                let decoded = fields
                    .iter()
                    .map(|f| {
                        Ok(NamedFieldType {
                            name: f.name.clone(),
                            body: self.decode_type(f.body, visiting)?,
                            metadata: decode_metadata(&f.metadata),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                SchemaType::Record { fields: decoded }
            }
            wire::SchemaTypeNode::VariantType(cases) => {
                let decoded = cases
                    .iter()
                    .map(|c| {
                        let payload = match c.payload {
                            Some(p) => Some(self.decode_type(p, visiting)?),
                            None => None,
                        };
                        Ok(VariantCaseType {
                            name: c.name.clone(),
                            payload,
                            metadata: decode_metadata(&c.metadata),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                SchemaType::Variant { cases: decoded }
            }
            wire::SchemaTypeNode::EnumType(cases) => SchemaType::Enum {
                cases: cases.clone(),
            },
            wire::SchemaTypeNode::FlagsType(flags) => SchemaType::Flags {
                flags: flags.clone(),
            },
            wire::SchemaTypeNode::TupleType(elements) => {
                let decoded = elements
                    .iter()
                    .map(|e| self.decode_type(*e, visiting))
                    .collect::<Result<Vec<_>, _>>()?;
                SchemaType::Tuple { elements: decoded }
            }
            wire::SchemaTypeNode::ListType(element) => SchemaType::List {
                element: Box::new(self.decode_type(*element, visiting)?),
            },
            wire::SchemaTypeNode::FixedListType(spec) => SchemaType::FixedList {
                element: Box::new(self.decode_type(spec.element, visiting)?),
                length: spec.length,
            },
            wire::SchemaTypeNode::MapType(spec) => SchemaType::Map {
                key: Box::new(self.decode_type(spec.key, visiting)?),
                value: Box::new(self.decode_type(spec.value, visiting)?),
            },
            wire::SchemaTypeNode::OptionType(inner) => SchemaType::Option {
                inner: Box::new(self.decode_type(*inner, visiting)?),
            },
            wire::SchemaTypeNode::ResultType(spec) => {
                let ok = match spec.ok {
                    Some(idx) => Some(Box::new(self.decode_type(idx, visiting)?)),
                    None => None,
                };
                let err = match spec.err {
                    Some(idx) => Some(Box::new(self.decode_type(idx, visiting)?)),
                    None => None,
                };
                SchemaType::Result(ResultSpec { ok, err })
            }
            wire::SchemaTypeNode::TextType(r) => SchemaType::Text(TextRestrictions {
                languages: r.languages.clone(),
                min_length: r.min_length,
                max_length: r.max_length,
                regex: r.regex.clone(),
            }),
            wire::SchemaTypeNode::BinaryType(r) => SchemaType::Binary(BinaryRestrictions {
                mime_types: r.mime_types.clone(),
                min_bytes: r.min_bytes,
                max_bytes: r.max_bytes,
            }),
            wire::SchemaTypeNode::PathType(p) => SchemaType::Path(PathSpec {
                direction: match p.direction {
                    wire::PathDirection::Input => PathDirection::Input,
                    wire::PathDirection::Output => PathDirection::Output,
                    wire::PathDirection::InOut => PathDirection::InOut,
                },
                kind: match p.kind {
                    wire::PathKind::File => PathKind::File,
                    wire::PathKind::Directory => PathKind::Directory,
                    wire::PathKind::Any => PathKind::Any,
                },
                allowed_mime_types: p.allowed_mime_types.clone(),
                allowed_extensions: p.allowed_extensions.clone(),
            }),
            wire::SchemaTypeNode::UrlType(r) => SchemaType::Url(UrlRestrictions {
                allowed_schemes: r.allowed_schemes.clone(),
                allowed_hosts: r.allowed_hosts.clone(),
            }),
            wire::SchemaTypeNode::DatetimeType => SchemaType::Datetime,
            wire::SchemaTypeNode::DurationType => SchemaType::Duration,
            wire::SchemaTypeNode::QuantityType(q) => SchemaType::Quantity(QuantitySpec {
                base_unit: q.base_unit.clone(),
                allowed_suffixes: q.allowed_suffixes.clone(),
                min: q.min.as_ref().map(decode_quantity_value),
                max: q.max.as_ref().map(decode_quantity_value),
            }),
            wire::SchemaTypeNode::UnionType(u) => {
                let branches = u
                    .branches
                    .iter()
                    .map(|b| {
                        Ok(UnionBranch {
                            tag: b.tag.clone(),
                            body: self.decode_type(b.body, visiting)?,
                            discriminator: decode_discriminator(&b.discriminator),
                            metadata: decode_metadata(&b.metadata),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                SchemaType::Union(UnionSpec { branches })
            }
            wire::SchemaTypeNode::SecretType(s) => SchemaType::Secret(SecretSpec {
                category: s.category.clone(),
                metadata: decode_metadata(&s.metadata),
            }),
            wire::SchemaTypeNode::QuotaTokenType(q) => SchemaType::QuotaToken(QuotaTokenSpec {
                resource_name: q.resource_name.clone(),
                metadata: decode_metadata(&q.metadata),
            }),
            wire::SchemaTypeNode::FutureType(inner) => {
                let inner = match inner {
                    Some(i) => Some(Box::new(self.decode_type(*i, visiting)?)),
                    None => None,
                };
                SchemaType::Future { inner }
            }
            wire::SchemaTypeNode::StreamType(inner) => {
                let inner = match inner {
                    Some(i) => Some(Box::new(self.decode_type(*i, visiting)?)),
                    None => None,
                };
                SchemaType::Stream { inner }
            }
        };
        Ok(out)
    }
}

fn decode_value_at(
    wire_tree: &wire::SchemaValueTree,
    idx: wire::ValueNodeIndex,
    visiting: &mut HashSet<wire::ValueNodeIndex>,
) -> Result<SchemaValue, DecodeError> {
    let node = wire_tree
        .value_nodes
        .get(usize_index_v(idx)?)
        .ok_or(DecodeError::ValueNodeIndexOutOfRange(idx))?;
    if !visiting.insert(idx) {
        return Err(DecodeError::CyclicTypeWithoutRef);
    }
    let result = decode_value_node(wire_tree, node, visiting);
    visiting.remove(&idx);
    result
}

fn decode_value_node(
    wire_tree: &wire::SchemaValueTree,
    node: &wire::SchemaValueNode,
    visiting: &mut HashSet<wire::ValueNodeIndex>,
) -> Result<SchemaValue, DecodeError> {
    let out = match node {
        wire::SchemaValueNode::BoolValue(b) => SchemaValue::Bool(*b),
        wire::SchemaValueNode::S8Value(v) => SchemaValue::S8(*v),
        wire::SchemaValueNode::S16Value(v) => SchemaValue::S16(*v),
        wire::SchemaValueNode::S32Value(v) => SchemaValue::S32(*v),
        wire::SchemaValueNode::S64Value(v) => SchemaValue::S64(*v),
        wire::SchemaValueNode::U8Value(v) => SchemaValue::U8(*v),
        wire::SchemaValueNode::U16Value(v) => SchemaValue::U16(*v),
        wire::SchemaValueNode::U32Value(v) => SchemaValue::U32(*v),
        wire::SchemaValueNode::U64Value(v) => SchemaValue::U64(*v),
        wire::SchemaValueNode::F32Value(v) => SchemaValue::F32(*v),
        wire::SchemaValueNode::F64Value(v) => SchemaValue::F64(*v),
        wire::SchemaValueNode::CharValue(c) => SchemaValue::Char(*c),
        wire::SchemaValueNode::StringValue(s) => SchemaValue::String(s.clone()),
        wire::SchemaValueNode::RecordValue(fields) => {
            let decoded = fields
                .iter()
                .map(|i| decode_value_at(wire_tree, *i, visiting))
                .collect::<Result<Vec<_>, _>>()?;
            SchemaValue::Record { fields: decoded }
        }
        wire::SchemaValueNode::VariantValue(p) => {
            let payload = match p.payload {
                Some(i) => Some(Box::new(decode_value_at(wire_tree, i, visiting)?)),
                None => None,
            };
            SchemaValue::Variant(VariantValuePayload {
                case: p.case,
                payload,
            })
        }
        wire::SchemaValueNode::EnumValue(c) => SchemaValue::Enum { case: *c },
        wire::SchemaValueNode::FlagsValue(bits) => SchemaValue::Flags { bits: bits.clone() },
        wire::SchemaValueNode::TupleValue(elements) => {
            let decoded = elements
                .iter()
                .map(|i| decode_value_at(wire_tree, *i, visiting))
                .collect::<Result<Vec<_>, _>>()?;
            SchemaValue::Tuple { elements: decoded }
        }
        wire::SchemaValueNode::ListValue(elements) => {
            let decoded = elements
                .iter()
                .map(|i| decode_value_at(wire_tree, *i, visiting))
                .collect::<Result<Vec<_>, _>>()?;
            SchemaValue::List { elements: decoded }
        }
        wire::SchemaValueNode::FixedListValue(elements) => {
            let decoded = elements
                .iter()
                .map(|i| decode_value_at(wire_tree, *i, visiting))
                .collect::<Result<Vec<_>, _>>()?;
            SchemaValue::FixedList { elements: decoded }
        }
        wire::SchemaValueNode::MapValue(entries) => {
            let decoded = entries
                .iter()
                .map(|e| {
                    Ok((
                        decode_value_at(wire_tree, e.key, visiting)?,
                        decode_value_at(wire_tree, e.value, visiting)?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
            SchemaValue::Map { entries: decoded }
        }
        wire::SchemaValueNode::OptionValue(inner) => SchemaValue::Option {
            inner: match inner {
                Some(i) => Some(Box::new(decode_value_at(wire_tree, *i, visiting)?)),
                None => None,
            },
        },
        wire::SchemaValueNode::ResultValue(p) => {
            let payload = match p {
                wire::ResultValuePayload::OkValue(opt) => ResultValuePayload::Ok {
                    value: match opt {
                        Some(i) => Some(Box::new(decode_value_at(wire_tree, *i, visiting)?)),
                        None => None,
                    },
                },
                wire::ResultValuePayload::ErrValue(opt) => ResultValuePayload::Err {
                    value: match opt {
                        Some(i) => Some(Box::new(decode_value_at(wire_tree, *i, visiting)?)),
                        None => None,
                    },
                },
            };
            SchemaValue::Result(payload)
        }
        wire::SchemaValueNode::TextValue(p) => SchemaValue::Text(TextValuePayload {
            text: p.text.clone(),
            language: p.language.clone(),
        }),
        wire::SchemaValueNode::BinaryValue(p) => SchemaValue::Binary(BinaryValuePayload {
            bytes: p.bytes.clone(),
            mime_type: p.mime_type.clone(),
        }),
        wire::SchemaValueNode::PathValue(p) => SchemaValue::Path { path: p.clone() },
        wire::SchemaValueNode::UrlValue(u) => SchemaValue::Url { url: u.clone() },
        wire::SchemaValueNode::DatetimeValue(d) => SchemaValue::Datetime {
            value: datetime_from_wire(d)?,
        },
        wire::SchemaValueNode::DurationValue(d) => SchemaValue::Duration(DurationValuePayload {
            nanoseconds: d.nanoseconds,
        }),
        wire::SchemaValueNode::QuantityValueNode(q) => SchemaValue::Quantity(QuantityValue {
            mantissa: q.mantissa,
            scale: q.scale,
            unit: q.unit.clone(),
        }),
        wire::SchemaValueNode::UnionValue(p) => SchemaValue::Union(UnionValuePayload {
            tag: p.tag.clone(),
            body: Box::new(decode_value_at(wire_tree, p.body, visiting)?),
        }),
        wire::SchemaValueNode::SecretValue(s) => SchemaValue::Secret(SecretValuePayload {
            secret_ref: s.secret_ref.clone(),
        }),
        wire::SchemaValueNode::QuotaTokenValue(q) => {
            SchemaValue::QuotaToken(QuotaTokenValuePayload {
                environment_id: uuid::Uuid::from_u64_pair(
                    q.environment_id.uuid.high_bits,
                    q.environment_id.uuid.low_bits,
                ),
                resource_name: q.resource_name.clone(),
                expected_use: q.expected_use,
                last_credit: q.last_credit,
                last_credit_at: datetime_from_wire(&q.last_credit_at)?,
            })
        }
    };
    Ok(out)
}

fn datetime_from_wire(d: &wire::Datetime) -> Result<DateTime<Utc>, DecodeError> {
    Utc.timestamp_opt(d.seconds, d.nanoseconds)
        .single()
        .ok_or(DecodeError::InvalidDatetime {
            seconds: d.seconds,
            nanoseconds: d.nanoseconds,
        })
}

fn decode_metadata(m: &wire::MetadataEnvelope) -> MetadataEnvelope {
    MetadataEnvelope {
        doc: m.doc.clone(),
        aliases: m.aliases.clone(),
        examples: m.examples.clone(),
        deprecated: m.deprecated.clone(),
        role: m.role.as_ref().map(|r| match r {
            wire::Role::Multimodal => Role::Multimodal,
            wire::Role::Other(s) => Role::Other(s.clone()),
        }),
    }
}

fn decode_quantity_value(q: &wire::QuantityValue) -> QuantityValue {
    QuantityValue {
        mantissa: q.mantissa,
        scale: q.scale,
        unit: q.unit.clone(),
    }
}

fn decode_discriminator(d: &wire::DiscriminatorRule) -> DiscriminatorRule {
    match d {
        wire::DiscriminatorRule::Prefix(s) => DiscriminatorRule::Prefix { prefix: s.clone() },
        wire::DiscriminatorRule::Suffix(s) => DiscriminatorRule::Suffix { suffix: s.clone() },
        wire::DiscriminatorRule::Contains(s) => DiscriminatorRule::Contains {
            substring: s.clone(),
        },
        wire::DiscriminatorRule::Regex(s) => DiscriminatorRule::Regex { regex: s.clone() },
        wire::DiscriminatorRule::FieldEquals(fd) => {
            DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: fd.field_name.clone(),
                literal: fd.literal.clone(),
            })
        }
        wire::DiscriminatorRule::FieldAbsent(s) => DiscriminatorRule::FieldAbsent {
            field_name: s.clone(),
        },
    }
}

fn usize_index(i: wire::TypeNodeIndex) -> Result<usize, DecodeError> {
    if i < 0 {
        Err(DecodeError::TypeNodeIndexOutOfRange(i))
    } else {
        Ok(i as usize)
    }
}

fn usize_index_v(i: wire::ValueNodeIndex) -> Result<usize, DecodeError> {
    if i < 0 {
        Err(DecodeError::ValueNodeIndexOutOfRange(i))
    } else {
        Ok(i as usize)
    }
}
