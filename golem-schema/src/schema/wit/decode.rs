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

use super::wire;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, NumericBound,
    NumericRestrictions, PathDirection, PathKind, PathSpec, QuantitySpec, QuantityValue,
    QuotaTokenSpec, ResultSpec, SchemaType, SecretSpec, TextRestrictions, UnionBranch, UnionSpec,
    UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{
    BinaryValuePayload, DurationValuePayload, QuotaTokenVariantValue, ResultValuePayload,
    SchemaValue, SecretVariantValue, TextValuePayload, UnionValuePayload, VariantValuePayload,
};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};

/// The owned handle type carried by `wire::SchemaValueNode::QuotaTokenHandle`.
/// On the host it is a resource-table handle backed by the opaque host rep; on a
/// guest it is the generated owned `quota-token` resource.
#[cfg(all(feature = "host", not(feature = "guest")))]
type WireQuotaHandle = wasmtime::component::Resource<super::QuotaTokenHandleRep>;
#[cfg(all(feature = "guest", not(feature = "host")))]
type WireQuotaHandle = wire::QuotaToken;

/// The owned handle type carried by `wire::SchemaValueNode::SecretValue`.
#[cfg(all(feature = "host", not(feature = "guest")))]
type WireSecretHandle = wasmtime::component::Resource<super::SecretHandleRep>;
#[cfg(all(feature = "guest", not(feature = "host")))]
type WireSecretHandle = wire::Secret;

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
    /// A `quota-token-handle` node was encountered while decoding through the
    /// pure (resolver-less) path. Owned quota-token handles can only be lifted
    /// on the host via [`decode_value_with`] and a `QuotaTokenResolver`.
    QuotaTokenRequiresResolver,
    /// A `secret-value` node was encountered while decoding through the pure
    /// path. Owned secret handles can only be lifted once the secret resolver
    /// path is added.
    SecretRequiresResolver,
    /// A value node was referenced more than once (or formed a cycle) while
    /// decoding an owned value tree. Owned handles are affine, so the tree must
    /// be a strict tree; aliasing would consume a handle twice.
    AliasedValueNode(wire::ValueNodeIndex),
    /// An owned `quota-token` handle was present in the value tree but never
    /// reached from the root. The handle is dropped (so it does not leak) and
    /// the tree is rejected as malformed, since every transferred owned resource
    /// must be referenced exactly once.
    UnconsumedQuotaTokenHandle(wire::ValueNodeIndex),
    /// An owned `secret` handle was present in the value tree but never reached
    /// from the root.
    UnconsumedSecretHandle(wire::ValueNodeIndex),
    /// The host `QuotaTokenResolver` failed to snapshot an owned handle.
    QuotaResolver(String),
    /// The host `SecretResolver` failed to snapshot an owned handle.
    SecretResolver(String),
    /// An owned `quota-token` handle was present in a value tree at a boundary
    /// that does not permit quota tokens (e.g. agent config, durable-function
    /// request/response, agent-error payloads, inbound oplog entries). The
    /// handle is dropped from the resource table before this error is returned,
    /// so nothing leaks.
    QuotaTokenNotPermitted(wire::ValueNodeIndex),
    /// An owned `secret` handle was present in a value tree at a boundary that
    /// does not permit secret transport. The handle is dropped from the resource
    /// table before this error is returned, so nothing leaks.
    SecretNotPermitted(wire::ValueNodeIndex),
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
            DecodeError::QuotaTokenRequiresResolver => write!(
                f,
                "quota-token handles can only be decoded through the host resolver-aware path"
            ),
            DecodeError::SecretRequiresResolver => write!(
                f,
                "secret handles can only be decoded through the secret resolver-aware path"
            ),
            DecodeError::AliasedValueNode(i) => {
                write!(f, "value node referenced more than once: {i}")
            }
            DecodeError::UnconsumedQuotaTokenHandle(i) => {
                write!(f, "quota-token handle not referenced from the root: {i}")
            }
            DecodeError::UnconsumedSecretHandle(i) => {
                write!(f, "secret handle not referenced from the root: {i}")
            }
            DecodeError::QuotaResolver(msg) => {
                write!(f, "quota-token handle could not be resolved: {msg}")
            }
            DecodeError::SecretResolver(msg) => {
                write!(f, "secret handle could not be resolved: {msg}")
            }
            DecodeError::QuotaTokenNotPermitted(i) => {
                write!(f, "quota-token handle not permitted at this boundary: {i}")
            }
            DecodeError::SecretNotPermitted(i) => {
                write!(f, "secret handle not permitted at this boundary: {i}")
            }
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
                body,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = ctx.decode_type(wire_graph.root, &mut HashSet::new())?;
    Ok(SchemaGraph { defs, root })
}

/// Decoder for a single flat [`wire::SchemaGraph`] that holds several
/// independent root types (the agent-layer carrier produced by
/// [`crate::schema::wit::GraphEncoder`]). Decodes the shared `defs` once and
/// then any `type-node-index` into a recursive [`SchemaType`].
pub struct GraphDecoder<'a> {
    ctx: GraphCtx<'a>,
}

impl<'a> GraphDecoder<'a> {
    pub fn new(wire_graph: &'a wire::SchemaGraph) -> Result<Self, DecodeError> {
        Ok(Self {
            ctx: GraphCtx::new(wire_graph)?,
        })
    }

    /// Decode the graph's named definitions. The resulting [`SchemaGraph`] uses
    /// these `defs` together with a placeholder root (matching
    /// [`SchemaGraph::empty`]); the agent-layer carriers never consult `root`.
    pub fn decode_defs(&self) -> Result<Vec<SchemaTypeDef>, DecodeError> {
        self.ctx
            .wire
            .defs
            .iter()
            .map(|d| {
                let body = self.ctx.decode_type(d.body, &mut HashSet::new())?;
                Ok(SchemaTypeDef {
                    id: TypeId(d.id.clone()),
                    name: d.name.clone(),
                    body,
                })
            })
            .collect()
    }

    /// Decode the (possibly recursive) schema type rooted at `idx`.
    pub fn decode_type_at(&self, idx: wire::TypeNodeIndex) -> Result<SchemaType, DecodeError> {
        self.ctx.decode_type(idx, &mut HashSet::new())
    }
}

/// Decode a value tree by reference, rejecting any quota-token handle (host /
/// feature-neutral).
///
/// This is the pure path used where the tree cannot contain a live quota token.
/// On a guest, [`decode_value`] instead consumes the tree by value so it can
/// move owned `quota-token` handles out of it; see the guest definition below.
#[cfg(not(all(feature = "guest", not(feature = "host"))))]
pub fn decode_value(wire_tree: &wire::SchemaValueTree) -> Result<SchemaValue, DecodeError> {
    reject_handles_in_pure_value_tree(wire_tree)?;
    decode_value_at(wire_tree, wire_tree.root, &mut HashSet::new())
}

/// Decode a value tree on a guest, consuming it so that each
/// `quota-token-handle` node's owned `own<quota-token>` resource can be moved
/// out into an opaque [`super::GuestQuotaTokenHandle`].
///
/// The tree is consumed because owned handles are affine: each must be moved out
/// exactly once. Any node referenced more than once (or forming a cycle) is
/// rejected with [`DecodeError::AliasedValueNode`]. Any handle that is never
/// reached from the root is dropped (releasing the underlying resource) and the
/// tree is rejected as malformed with [`DecodeError::UnconsumedQuotaTokenHandle`].
#[cfg(all(feature = "guest", not(feature = "host")))]
pub fn decode_value(wire_tree: wire::SchemaValueTree) -> Result<SchemaValue, DecodeError> {
    let root = wire_tree.root;
    let mut slots: Vec<Option<wire::SchemaValueNode>> =
        wire_tree.value_nodes.into_iter().map(Some).collect();
    // Validate the whole tree before lifting any owned handle, so a malformed
    // sibling cannot cause an already-lifted token to be discarded.
    let result = match preflight_owned_value_tree(&slots, root) {
        Ok(()) => decode_owned_at(
            &mut slots,
            root,
            &mut |handle| Ok(super::GuestQuotaTokenHandle::new(handle)),
            &mut |handle| Ok(super::GuestSecretHandle::new(handle)),
        ),
        Err(e) => Err(e),
    };

    // Drop every handle that was not consumed while walking the tree, regardless
    // of success or failure, so no owned resource leaks. On the guest the
    // generated resource's RAII drop releases the underlying host resource.
    let mut leaked: Option<DecodeError> = None;
    for (i, slot) in slots.iter_mut().enumerate() {
        match slot.take() {
            Some(wire::SchemaValueNode::QuotaTokenHandle(_)) => {
                leaked.get_or_insert(DecodeError::UnconsumedQuotaTokenHandle(
                    i as wire::ValueNodeIndex,
                ));
            }
            Some(wire::SchemaValueNode::SecretValue(_)) => {
                leaked.get_or_insert(DecodeError::UnconsumedSecretHandle(
                    i as wire::ValueNodeIndex,
                ));
            }
            other => *slot = other,
        }
    }

    match result {
        Ok(value) => leaked.map_or(Ok(value), Err),
        Err(err) => Err(err),
    }
}

pub fn decode_typed(wire_typed: &wire::TypedSchemaValue) -> Result<TypedSchemaValue, DecodeError> {
    let graph = decode_graph(&wire_typed.graph)?;
    // `decode_typed` is a quota-rejecting boundary on both host and guest: it is
    // used for durable-function request/response and agent errors, which never
    // carry quota tokens. Decode the value by reference so any stray handle is
    // rejected (and left for the caller's tree to drop) rather than lifted.
    reject_handles_in_pure_value_tree(&wire_typed.value)?;
    let value = decode_value_at(
        &wire_typed.value,
        wire_typed.value.root,
        &mut HashSet::new(),
    )?;
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
        let metadata = decode_metadata(&node.metadata);
        let out = match &node.body {
            wire::SchemaTypeBody::RefType(def_idx) => {
                let def = self
                    .wire
                    .defs
                    .get(usize_index(*def_idx)?)
                    .ok_or(DecodeError::DefIndexOutOfRange(*def_idx))?;
                SchemaType::Ref {
                    id: TypeId(def.id.clone()),
                    metadata,
                }
            }
            wire::SchemaTypeBody::BoolType => SchemaType::Bool { metadata },
            wire::SchemaTypeBody::S8Type(r) => SchemaType::S8 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::S16Type(r) => SchemaType::S16 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::S32Type(r) => SchemaType::S32 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::S64Type(r) => SchemaType::S64 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::U8Type(r) => SchemaType::U8 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::U16Type(r) => SchemaType::U16 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::U32Type(r) => SchemaType::U32 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::U64Type(r) => SchemaType::U64 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::F32Type(r) => SchemaType::F32 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::F64Type(r) => SchemaType::F64 {
                restrictions: decode_numeric(r),
                metadata,
            },
            wire::SchemaTypeBody::CharType => SchemaType::Char { metadata },
            wire::SchemaTypeBody::StringType => SchemaType::String { metadata },
            wire::SchemaTypeBody::RecordType(fields) => {
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
                SchemaType::Record {
                    fields: decoded,
                    metadata,
                }
            }
            wire::SchemaTypeBody::VariantType(cases) => {
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
                SchemaType::Variant {
                    cases: decoded,
                    metadata,
                }
            }
            wire::SchemaTypeBody::EnumType(cases) => SchemaType::Enum {
                cases: cases.clone(),
                metadata,
            },
            wire::SchemaTypeBody::FlagsType(flags) => SchemaType::Flags {
                flags: flags.clone(),
                metadata,
            },
            wire::SchemaTypeBody::TupleType(elements) => {
                let decoded = elements
                    .iter()
                    .map(|e| self.decode_type(*e, visiting))
                    .collect::<Result<Vec<_>, _>>()?;
                SchemaType::Tuple {
                    elements: decoded,
                    metadata,
                }
            }
            wire::SchemaTypeBody::ListType(element) => SchemaType::List {
                element: Box::new(self.decode_type(*element, visiting)?),
                metadata,
            },
            wire::SchemaTypeBody::FixedListType(spec) => SchemaType::FixedList {
                element: Box::new(self.decode_type(spec.element, visiting)?),
                length: spec.length,
                metadata,
            },
            wire::SchemaTypeBody::MapType(spec) => SchemaType::Map {
                key: Box::new(self.decode_type(spec.key, visiting)?),
                value: Box::new(self.decode_type(spec.value, visiting)?),
                metadata,
            },
            wire::SchemaTypeBody::OptionType(inner) => SchemaType::Option {
                inner: Box::new(self.decode_type(*inner, visiting)?),
                metadata,
            },
            wire::SchemaTypeBody::ResultType(spec) => {
                let ok = match spec.ok {
                    Some(idx) => Some(Box::new(self.decode_type(idx, visiting)?)),
                    None => None,
                };
                let err = match spec.err {
                    Some(idx) => Some(Box::new(self.decode_type(idx, visiting)?)),
                    None => None,
                };
                SchemaType::Result {
                    spec: ResultSpec { ok, err },
                    metadata,
                }
            }
            wire::SchemaTypeBody::TextType(r) => SchemaType::Text {
                restrictions: TextRestrictions {
                    languages: r.languages.clone(),
                    min_length: r.min_length,
                    max_length: r.max_length,
                    regex: r.regex.clone(),
                },
                metadata,
            },
            wire::SchemaTypeBody::BinaryType(r) => SchemaType::Binary {
                restrictions: BinaryRestrictions {
                    mime_types: r.mime_types.clone(),
                    min_bytes: r.min_bytes,
                    max_bytes: r.max_bytes,
                },
                metadata,
            },
            wire::SchemaTypeBody::PathType(p) => SchemaType::Path {
                spec: PathSpec {
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
                },
                metadata,
            },
            wire::SchemaTypeBody::UrlType(r) => SchemaType::Url {
                restrictions: UrlRestrictions {
                    allowed_schemes: r.allowed_schemes.clone(),
                    allowed_hosts: r.allowed_hosts.clone(),
                },
                metadata,
            },
            wire::SchemaTypeBody::DatetimeType => SchemaType::Datetime { metadata },
            wire::SchemaTypeBody::DurationType => SchemaType::Duration { metadata },
            wire::SchemaTypeBody::QuantityType(q) => SchemaType::Quantity {
                spec: QuantitySpec {
                    base_unit: q.base_unit.clone(),
                    allowed_suffixes: q.allowed_suffixes.clone(),
                    min: q.min.as_ref().map(decode_quantity_value),
                    max: q.max.as_ref().map(decode_quantity_value),
                },
                metadata,
            },
            wire::SchemaTypeBody::UnionType(u) => {
                let branches = u
                    .branches
                    .iter()
                    .map(|b| {
                        let body = self.decode_type(b.body, visiting)?;
                        Ok(UnionBranch {
                            tag: b.tag.clone(),
                            body,
                            discriminator: decode_discriminator(&b.discriminator),
                            metadata: decode_metadata(&b.metadata),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                SchemaType::Union {
                    spec: UnionSpec { branches },
                    metadata,
                }
            }
            wire::SchemaTypeBody::SecretType(s) => SchemaType::Secret {
                spec: SecretSpec {
                    inner: Box::new(self.decode_type(s.inner, visiting)?),
                    category: s.category.clone(),
                },
                metadata,
            },
            wire::SchemaTypeBody::QuotaTokenType(q) => SchemaType::QuotaToken {
                spec: QuotaTokenSpec {
                    resource_name: q.resource_name.clone(),
                },
                metadata,
            },
            wire::SchemaTypeBody::FutureType(inner) => {
                let inner = match inner {
                    Some(i) => Some(Box::new(self.decode_type(*i, visiting)?)),
                    None => None,
                };
                SchemaType::Future { inner, metadata }
            }
            wire::SchemaTypeBody::StreamType(inner) => {
                let inner = match inner {
                    Some(i) => Some(Box::new(self.decode_type(*i, visiting)?)),
                    None => None,
                };
                SchemaType::Stream { inner, metadata }
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
        wire::SchemaValueNode::SecretValue(_) => return Err(DecodeError::SecretRequiresResolver),
        wire::SchemaValueNode::QuotaTokenHandle(_) => {
            return Err(DecodeError::QuotaTokenRequiresResolver);
        }
    };
    Ok(out)
}

/// Decode an owned value tree, lifting each `quota-token-handle` node into a
/// trusted [`SchemaValue::QuotaToken`] snapshot via the supplied resolver.
///
/// The tree is consumed because the owned handles it carries are affine: each
/// must be moved out and snapshotted exactly once. Any node referenced more
/// than once (or forming a cycle) is rejected with
/// [`DecodeError::AliasedValueNode`].
///
/// Every `own<quota-token>` handle present in the tree has already been
/// transferred to the host, so each must be consumed exactly once. After
/// decoding, any handle that was never reached from the root is released through
/// [`super::QuotaTokenResolver::drop_handle`] (so none leak from the table) and,
/// on an otherwise successful decode, the tree is rejected as malformed with
/// [`DecodeError::UnconsumedQuotaTokenHandle`]. The same cleanup runs when
/// decoding fails partway through.
#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn decode_value_with<R: super::QuotaTokenResolver + super::SecretResolver>(
    wire_tree: wire::SchemaValueTree,
    resolver: &mut R,
) -> Result<SchemaValue, DecodeError> {
    let resolver = std::cell::RefCell::new(resolver);
    let root = wire_tree.root;
    let mut slots: Vec<Option<wire::SchemaValueNode>> =
        wire_tree.value_nodes.into_iter().map(Some).collect();
    // Validate the whole tree before snapshotting any owned handle, so a
    // malformed sibling cannot cause an already-snapshotted token to be
    // discarded. After a successful preflight the only fallible step left is the
    // snapshot itself.
    let result = match preflight_owned_value_tree(&slots, root) {
        Ok(()) => decode_owned_at(
            &mut slots,
            root,
            &mut |handle| {
                let mut resolver = resolver.borrow_mut();
                resolver
                    .snapshot_handle(handle)
                    .map_err(|e| DecodeError::QuotaResolver(e.to_string()))
            },
            &mut |handle| {
                let mut resolver = resolver.borrow_mut();
                resolver
                    .snapshot_secret_handle(handle)
                    .map_err(|e| DecodeError::SecretResolver(e.to_string()))
            },
        ),
        Err(e) => Err(e),
    };

    // Drop every handle that was not consumed while walking the tree, regardless
    // of success or failure, so no owned resource leaks from the table.
    let mut leaked: Option<DecodeError> = None;
    let mut resolver = resolver.borrow_mut();
    for (i, slot) in slots.iter_mut().enumerate() {
        match slot.take() {
            Some(wire::SchemaValueNode::QuotaTokenHandle(handle)) => {
                resolver.drop_handle(handle);
                leaked.get_or_insert(DecodeError::UnconsumedQuotaTokenHandle(
                    i as wire::ValueNodeIndex,
                ));
            }
            Some(wire::SchemaValueNode::SecretValue(handle)) => {
                super::SecretResolver::drop_secret_handle(&mut **resolver, handle);
                leaked.get_or_insert(DecodeError::UnconsumedSecretHandle(
                    i as wire::ValueNodeIndex,
                ));
            }
            other => *slot = other,
        }
    }

    match result {
        Ok(value) => leaked.map_or(Ok(value), Err),
        Err(err) => Err(err),
    }
}

/// Drain every owned `quota-token` handle out of an owned value tree at a
/// boundary that does **not** permit quota tokens.
///
/// The tree is consumed because the owned handles it carries were already
/// transferred into the host resource table at the WIT boundary; each must be
/// deleted exactly once. Every handle node is dropped via
/// [`super::QuotaTokenHandleDropper`], regardless of where it sits in the tree
/// (including unreferenced nodes), so nothing leaks. If any handle was present
/// the tree is rejected with [`DecodeError::QuotaTokenNotPermitted`]; otherwise
/// the handle-free tree is returned unchanged so the caller can decode it with
/// the pure path.
#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn reject_quota_handles_in_value_tree<
    D: super::QuotaTokenHandleDropper + super::SecretHandleDropper,
>(
    mut wire_tree: wire::SchemaValueTree,
    dropper: &mut D,
) -> Result<wire::SchemaValueTree, DecodeError> {
    let mut first_error: Option<DecodeError> = None;
    for (i, node) in wire_tree.value_nodes.iter_mut().enumerate() {
        if matches!(node, wire::SchemaValueNode::QuotaTokenHandle(_))
            || matches!(node, wire::SchemaValueNode::SecretValue(_))
        {
            // Replace the handle node with a placeholder so the `Resource` can be
            // moved out and deleted from the table. The tree is rejected below,
            // so the placeholder is never observed.
            let taken = std::mem::replace(node, wire::SchemaValueNode::BoolValue(false));
            match taken {
                wire::SchemaValueNode::QuotaTokenHandle(handle) => {
                    dropper.drop_quota_token_handle(handle);
                    first_error.get_or_insert(DecodeError::QuotaTokenNotPermitted(
                        i as wire::ValueNodeIndex,
                    ));
                }
                wire::SchemaValueNode::SecretValue(handle) => {
                    dropper.drop_secret_handle(handle);
                    first_error
                        .get_or_insert(DecodeError::SecretNotPermitted(i as wire::ValueNodeIndex));
                }
                _ => unreachable!(),
            }
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(wire_tree),
    }
}

#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn reject_secret_handles_in_value_tree<
    D: super::QuotaTokenHandleDropper + super::SecretHandleDropper,
>(
    wire_tree: wire::SchemaValueTree,
    dropper: &mut D,
) -> Result<wire::SchemaValueTree, DecodeError> {
    reject_quota_handles_in_value_tree(wire_tree, dropper)
}

/// Decode an owned value tree at a boundary that does not permit quota tokens.
///
/// Any owned `quota-token` handle is deleted from the resource table and the
/// decode is rejected with [`DecodeError::QuotaTokenNotPermitted`], so a guest
/// cannot leak a handle by smuggling one into a reject-only position.
#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn decode_value_rejecting_quota_with<
    D: super::QuotaTokenHandleDropper + super::SecretHandleDropper,
>(
    wire_tree: wire::SchemaValueTree,
    dropper: &mut D,
) -> Result<SchemaValue, DecodeError> {
    let wire_tree = reject_quota_handles_in_value_tree(wire_tree, dropper)?;
    decode_value(&wire_tree)
}

#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn decode_value_rejecting_secret_with<
    D: super::QuotaTokenHandleDropper + super::SecretHandleDropper,
>(
    wire_tree: wire::SchemaValueTree,
    dropper: &mut D,
) -> Result<SchemaValue, DecodeError> {
    decode_value_rejecting_quota_with(wire_tree, dropper)
}

/// Decode an owned typed value at a boundary that does not permit quota tokens.
///
/// The value tree is drained of owned handles **before** the schema graph is
/// decoded, so an invalid graph cannot cause an early return that leaks a
/// handle. Any owned `quota-token` handle is deleted from the resource table
/// and the decode is rejected with [`DecodeError::QuotaTokenNotPermitted`].
#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn decode_typed_rejecting_quota_with<
    D: super::QuotaTokenHandleDropper + super::SecretHandleDropper,
>(
    wire_typed: wire::TypedSchemaValue,
    dropper: &mut D,
) -> Result<TypedSchemaValue, DecodeError> {
    let value_tree = reject_quota_handles_in_value_tree(wire_typed.value, dropper)?;
    let graph = decode_graph(&wire_typed.graph)?;
    let value = decode_value(&value_tree)?;
    Ok(TypedSchemaValue::new(graph, value))
}

#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn decode_typed_rejecting_secret_with<
    D: super::QuotaTokenHandleDropper + super::SecretHandleDropper,
>(
    wire_typed: wire::TypedSchemaValue,
    dropper: &mut D,
) -> Result<TypedSchemaValue, DecodeError> {
    decode_typed_rejecting_quota_with(wire_typed, dropper)
}

/// Validate an owned value tree by reference *before* any affine
/// `quota-token` handle is moved out of it.
///
/// [`decode_owned_at`] lifts each handle (snapshotting and deleting it from the
/// host table, or moving the owned guest resource out) as soon as it reaches
/// the node, but later sibling nodes can still fail validation. Without this
/// pass a malformed sibling — e.g. `tuple([quota-token-handle, invalid-datetime])`
/// — would cause an already-lifted token to be discarded. Running this first
/// guarantees that the only fallible step left in [`decode_owned_at`] is the
/// quota lift itself, so a token is never consumed because of an unrelated
/// failure elsewhere in the tree.
///
/// It checks, without consuming anything, that:
/// - every reachable index is in range;
/// - the reachable nodes form a strict tree — no node is referenced more than
///   once and there are no cycles — since owned handles are affine and each
///   slot may be moved out exactly once;
/// - every `datetime` node is a valid timestamp;
/// - every `quota-token-handle` node in the list is reachable from the root,
///   since an unreferenced owned handle is malformed (it would otherwise be
///   silently dropped).
fn preflight_owned_value_tree(
    slots: &[Option<wire::SchemaValueNode>],
    root: wire::ValueNodeIndex,
) -> Result<(), DecodeError> {
    let mut reached = vec![false; slots.len()];
    preflight_owned_at(slots, root, &mut reached)?;
    for (i, slot) in slots.iter().enumerate() {
        match slot {
            Some(wire::SchemaValueNode::SecretValue(_)) if !reached[i] => {
                return Err(DecodeError::UnconsumedSecretHandle(
                    i as wire::ValueNodeIndex,
                ));
            }
            Some(wire::SchemaValueNode::QuotaTokenHandle(_)) if !reached[i] => {
                return Err(DecodeError::UnconsumedQuotaTokenHandle(
                    i as wire::ValueNodeIndex,
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn preflight_owned_at(
    slots: &[Option<wire::SchemaValueNode>],
    idx: wire::ValueNodeIndex,
    reached: &mut [bool],
) -> Result<(), DecodeError> {
    let pos = usize_index_v(idx)?;
    let node = slots
        .get(pos)
        .ok_or(DecodeError::ValueNodeIndexOutOfRange(idx))?
        .as_ref()
        // Every slot is populated during preflight, so a missing one can only be
        // a node that was already reached, i.e. an aliasing violation.
        .ok_or(DecodeError::AliasedValueNode(idx))?;
    if reached[pos] {
        return Err(DecodeError::AliasedValueNode(idx));
    }
    reached[pos] = true;
    match node {
        wire::SchemaValueNode::RecordValue(fields) => {
            for i in fields {
                preflight_owned_at(slots, *i, reached)?;
            }
        }
        wire::SchemaValueNode::VariantValue(p) => {
            if let Some(i) = p.payload {
                preflight_owned_at(slots, i, reached)?;
            }
        }
        wire::SchemaValueNode::TupleValue(elements)
        | wire::SchemaValueNode::ListValue(elements)
        | wire::SchemaValueNode::FixedListValue(elements) => {
            for i in elements {
                preflight_owned_at(slots, *i, reached)?;
            }
        }
        wire::SchemaValueNode::MapValue(entries) => {
            for e in entries {
                preflight_owned_at(slots, e.key, reached)?;
                preflight_owned_at(slots, e.value, reached)?;
            }
        }
        wire::SchemaValueNode::OptionValue(Some(i)) => {
            preflight_owned_at(slots, *i, reached)?;
        }
        wire::SchemaValueNode::OptionValue(None) => {}
        wire::SchemaValueNode::ResultValue(p) => match p {
            wire::ResultValuePayload::OkValue(opt) | wire::ResultValuePayload::ErrValue(opt) => {
                if let Some(i) = opt {
                    preflight_owned_at(slots, *i, reached)?;
                }
            }
        },
        wire::SchemaValueNode::UnionValue(p) => {
            preflight_owned_at(slots, p.body, reached)?;
        }
        wire::SchemaValueNode::DatetimeValue(d) => {
            datetime_from_wire(d)?;
        }
        wire::SchemaValueNode::SecretValue(_) => {}
        // All remaining node kinds are leaves with no child indices and no
        // extra decode-time validation. Quota handles are leaves too; their
        // reachability is checked by the caller after the walk.
        _ => {}
    }
    Ok(())
}

fn reject_handles_in_pure_value_tree(wire_tree: &wire::SchemaValueTree) -> Result<(), DecodeError> {
    for node in &wire_tree.value_nodes {
        match node {
            wire::SchemaValueNode::SecretValue(_) => {
                return Err(DecodeError::SecretRequiresResolver);
            }
            wire::SchemaValueNode::QuotaTokenHandle(_) => {
                return Err(DecodeError::QuotaTokenRequiresResolver);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Decode an owned value tree node-by-node, taking each slot exactly once so
/// affine owned handles can be moved out. Each `quota-token-handle` node's owned
/// handle is passed to `lift_quota`, which converts it into the build-specific
/// [`QuotaTokenVariantValue`] (a trusted snapshot on the host, an opaque owned
/// handle on a guest).
///
/// Callers must run [`preflight_owned_value_tree`] first so that the only
/// fallible step here is `lift_quota`; otherwise a later validation failure
/// could discard an already-lifted token.
fn decode_owned_at(
    slots: &mut [Option<wire::SchemaValueNode>],
    idx: wire::ValueNodeIndex,
    lift_quota: &mut dyn FnMut(WireQuotaHandle) -> Result<QuotaTokenVariantValue, DecodeError>,
    lift_secret: &mut dyn FnMut(WireSecretHandle) -> Result<SecretVariantValue, DecodeError>,
) -> Result<SchemaValue, DecodeError> {
    let pos = usize_index_v(idx)?;
    let node = slots
        .get_mut(pos)
        .ok_or(DecodeError::ValueNodeIndexOutOfRange(idx))?
        .take()
        .ok_or(DecodeError::AliasedValueNode(idx))?;
    let out = match node {
        wire::SchemaValueNode::BoolValue(b) => SchemaValue::Bool(b),
        wire::SchemaValueNode::S8Value(v) => SchemaValue::S8(v),
        wire::SchemaValueNode::S16Value(v) => SchemaValue::S16(v),
        wire::SchemaValueNode::S32Value(v) => SchemaValue::S32(v),
        wire::SchemaValueNode::S64Value(v) => SchemaValue::S64(v),
        wire::SchemaValueNode::U8Value(v) => SchemaValue::U8(v),
        wire::SchemaValueNode::U16Value(v) => SchemaValue::U16(v),
        wire::SchemaValueNode::U32Value(v) => SchemaValue::U32(v),
        wire::SchemaValueNode::U64Value(v) => SchemaValue::U64(v),
        wire::SchemaValueNode::F32Value(v) => SchemaValue::F32(v),
        wire::SchemaValueNode::F64Value(v) => SchemaValue::F64(v),
        wire::SchemaValueNode::CharValue(c) => SchemaValue::Char(c),
        wire::SchemaValueNode::StringValue(s) => SchemaValue::String(s),
        wire::SchemaValueNode::RecordValue(fields) => {
            let mut decoded = Vec::with_capacity(fields.len());
            for i in fields {
                decoded.push(decode_owned_at(slots, i, lift_quota, lift_secret)?);
            }
            SchemaValue::Record { fields: decoded }
        }
        wire::SchemaValueNode::VariantValue(p) => {
            let payload = match p.payload {
                Some(i) => Some(Box::new(decode_owned_at(
                    slots,
                    i,
                    lift_quota,
                    lift_secret,
                )?)),
                None => None,
            };
            SchemaValue::Variant(VariantValuePayload {
                case: p.case,
                payload,
            })
        }
        wire::SchemaValueNode::EnumValue(c) => SchemaValue::Enum { case: c },
        wire::SchemaValueNode::FlagsValue(bits) => SchemaValue::Flags { bits },
        wire::SchemaValueNode::TupleValue(elements) => {
            let mut decoded = Vec::with_capacity(elements.len());
            for i in elements {
                decoded.push(decode_owned_at(slots, i, lift_quota, lift_secret)?);
            }
            SchemaValue::Tuple { elements: decoded }
        }
        wire::SchemaValueNode::ListValue(elements) => {
            let mut decoded = Vec::with_capacity(elements.len());
            for i in elements {
                decoded.push(decode_owned_at(slots, i, lift_quota, lift_secret)?);
            }
            SchemaValue::List { elements: decoded }
        }
        wire::SchemaValueNode::FixedListValue(elements) => {
            let mut decoded = Vec::with_capacity(elements.len());
            for i in elements {
                decoded.push(decode_owned_at(slots, i, lift_quota, lift_secret)?);
            }
            SchemaValue::FixedList { elements: decoded }
        }
        wire::SchemaValueNode::MapValue(entries) => {
            let mut decoded = Vec::with_capacity(entries.len());
            for e in entries {
                let key = decode_owned_at(slots, e.key, lift_quota, lift_secret)?;
                let value = decode_owned_at(slots, e.value, lift_quota, lift_secret)?;
                decoded.push((key, value));
            }
            SchemaValue::Map { entries: decoded }
        }
        wire::SchemaValueNode::OptionValue(inner) => SchemaValue::Option {
            inner: match inner {
                Some(i) => Some(Box::new(decode_owned_at(
                    slots,
                    i,
                    lift_quota,
                    lift_secret,
                )?)),
                None => None,
            },
        },
        wire::SchemaValueNode::ResultValue(p) => {
            let payload = match p {
                wire::ResultValuePayload::OkValue(opt) => ResultValuePayload::Ok {
                    value: match opt {
                        Some(i) => Some(Box::new(decode_owned_at(
                            slots,
                            i,
                            lift_quota,
                            lift_secret,
                        )?)),
                        None => None,
                    },
                },
                wire::ResultValuePayload::ErrValue(opt) => ResultValuePayload::Err {
                    value: match opt {
                        Some(i) => Some(Box::new(decode_owned_at(
                            slots,
                            i,
                            lift_quota,
                            lift_secret,
                        )?)),
                        None => None,
                    },
                },
            };
            SchemaValue::Result(payload)
        }
        wire::SchemaValueNode::TextValue(p) => SchemaValue::Text(TextValuePayload {
            text: p.text,
            language: p.language,
        }),
        wire::SchemaValueNode::BinaryValue(p) => SchemaValue::Binary(BinaryValuePayload {
            bytes: p.bytes,
            mime_type: p.mime_type,
        }),
        wire::SchemaValueNode::PathValue(p) => SchemaValue::Path { path: p },
        wire::SchemaValueNode::UrlValue(u) => SchemaValue::Url { url: u },
        wire::SchemaValueNode::DatetimeValue(d) => SchemaValue::Datetime {
            value: datetime_from_wire(&d)?,
        },
        wire::SchemaValueNode::DurationValue(d) => SchemaValue::Duration(DurationValuePayload {
            nanoseconds: d.nanoseconds,
        }),
        wire::SchemaValueNode::QuantityValueNode(q) => SchemaValue::Quantity(QuantityValue {
            mantissa: q.mantissa,
            scale: q.scale,
            unit: q.unit,
        }),
        wire::SchemaValueNode::UnionValue(p) => SchemaValue::Union(UnionValuePayload {
            tag: p.tag,
            body: Box::new(decode_owned_at(slots, p.body, lift_quota, lift_secret)?),
        }),
        wire::SchemaValueNode::SecretValue(handle) => SchemaValue::Secret(lift_secret(handle)?),
        wire::SchemaValueNode::QuotaTokenHandle(handle) => {
            SchemaValue::QuotaToken(lift_quota(handle)?)
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

pub fn decode_metadata(m: &wire::MetadataEnvelope) -> MetadataEnvelope {
    MetadataEnvelope {
        doc: m.doc.clone(),
        aliases: m.aliases.clone(),
        examples: m.examples.clone(),
        deprecated: m.deprecated.clone(),
        role: m.role.as_ref().map(|r| match r {
            wire::Role::Multimodal => Role::Multimodal,
            wire::Role::UnstructuredText => Role::UnstructuredText,
            wire::Role::UnstructuredBinary => Role::UnstructuredBinary,
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

/// Decode a wire numeric restriction, normalizing a decoded empty restriction
/// set to `None` (the canonicalization invariant: `Some(empty)` is never kept).
fn decode_numeric(r: &Option<wire::NumericRestrictions>) -> Option<NumericRestrictions> {
    r.as_ref().and_then(|r| {
        NumericRestrictions {
            min: r.min.as_ref().map(decode_numeric_bound),
            max: r.max.as_ref().map(decode_numeric_bound),
            unit: r.unit.clone(),
        }
        .normalize()
    })
}

fn decode_numeric_bound(b: &wire::NumericBound) -> NumericBound {
    match b {
        wire::NumericBound::Signed(v) => NumericBound::Signed(*v),
        wire::NumericBound::Unsigned(v) => NumericBound::Unsigned(*v),
        wire::NumericBound::FloatBits(bits) => NumericBound::FloatBits(*bits),
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
