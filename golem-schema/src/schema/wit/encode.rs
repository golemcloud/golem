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
    BinaryRestrictions, DiscriminatorRule, NamedFieldType, NumericBound, NumericRestrictions,
    PathDirection, PathKind, PathSpec, QuantitySpec, QuantityValue, SchemaType, TextRestrictions,
    UnionBranch, UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{
    PermissionCardVariantValue, QuotaTokenVariantValue, ResultValuePayload, SchemaValue,
    SecretVariantValue,
};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};

/// Errors that can occur while encoding a recursive schema graph / value into
/// the flat wire form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// A [`SchemaType::Ref`] referenced a `TypeId` that is not defined in the
    /// enclosing graph.
    UnknownTypeId(TypeId),
    /// Two definitions share the same `TypeId`.
    DuplicateTypeId(TypeId),
    /// A [`SchemaValue::QuotaToken`] snapshot was encountered while encoding a
    /// value tree through the pure (resolver-less) path. Quota tokens cross the
    /// WASM boundary only as owned handles, which requires the host-side
    /// [`encode_value_with`] entry point and a `QuotaTokenResolver`.
    QuotaTokenNotTransportable,
    /// A [`SchemaValue::Secret`] snapshot was encountered while encoding a
    /// value tree through the pure (resolver-less) path. Secrets cross the
    /// WASM boundary only as owned handles.
    SecretNotTransportable,
    /// A [`SchemaValue::PermissionCard`] snapshot was encountered while encoding
    /// a value tree through the pure (resolver-less) path. Permission cards
    /// cross the WASM boundary only as owned handles, which requires the
    /// host-side [`encode_value_with`] entry point and a
    /// `PermissionCardResolver`.
    PermissionCardNotTransportable,
    /// The host `QuotaTokenResolver` failed to materialize an owned handle from
    /// a snapshot.
    QuotaResolver(String),
    /// The host `SecretResolver` failed to materialize an owned handle from a
    /// snapshot.
    SecretResolver(String),
    /// The host `PermissionCardResolver` failed to materialize an owned handle
    /// from a snapshot.
    PermissionCardResolver(String),
    /// (Guest) A quota-token value was encoded after its owned handle had
    /// already been transferred out by an earlier encode. An owned
    /// `quota-token` can only be lowered once.
    QuotaTokenAlreadyConsumed,
    /// (Guest) A secret value was encoded after its owned handle had already
    /// been transferred out by an earlier encode.
    SecretAlreadyConsumed,
    /// (Guest) A permission-card value was encoded after its owned handle had
    /// already been transferred out by an earlier encode.
    PermissionCardAlreadyConsumed,
    /// (Guest) The same owned quota-token handle appeared more than once in a
    /// single value tree. An owned `quota-token` cannot be lowered twice; split
    /// the token first if two independent capabilities are required.
    AliasedQuotaTokenHandle,
    /// (Guest) The same owned secret handle appeared more than once in a single
    /// value tree.
    AliasedSecretHandle,
    /// (Guest) The same owned permission-card handle appeared more than once in
    /// a single value tree. An owned `permission-card` cannot be lowered twice.
    AliasedPermissionCardHandle,
}

impl Display for EncodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::UnknownTypeId(id) => write!(f, "unknown type id: {id}"),
            EncodeError::DuplicateTypeId(id) => write!(f, "duplicate type id: {id}"),
            EncodeError::QuotaTokenNotTransportable => write!(
                f,
                "quota-token values can only be encoded through the host resolver-aware path"
            ),
            EncodeError::SecretNotTransportable => write!(
                f,
                "secret values can only be encoded through a secret resolver-aware path"
            ),
            EncodeError::PermissionCardNotTransportable => write!(
                f,
                "permission-card values can only be encoded through the host resolver-aware path"
            ),
            EncodeError::QuotaResolver(msg) => {
                write!(f, "quota-token handle could not be created: {msg}")
            }
            EncodeError::SecretResolver(msg) => {
                write!(f, "secret handle could not be created: {msg}")
            }
            EncodeError::PermissionCardResolver(msg) => {
                write!(f, "permission-card handle could not be created: {msg}")
            }
            EncodeError::QuotaTokenAlreadyConsumed => write!(
                f,
                "quota-token handle was already transferred; an owned quota-token can only be sent once"
            ),
            EncodeError::SecretAlreadyConsumed => write!(
                f,
                "secret handle was already transferred; an owned secret can only be sent once"
            ),
            EncodeError::PermissionCardAlreadyConsumed => write!(
                f,
                "permission-card handle was already transferred; an owned permission-card can only be sent once"
            ),
            EncodeError::AliasedQuotaTokenHandle => write!(
                f,
                "the same quota-token handle appeared more than once in one value tree"
            ),
            EncodeError::AliasedSecretHandle => {
                write!(
                    f,
                    "the same secret handle appeared more than once in one value tree"
                )
            }
            EncodeError::AliasedPermissionCardHandle => write!(
                f,
                "the same permission-card handle appeared more than once in one value tree"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

pub fn encode_graph(graph: &SchemaGraph) -> Result<wire::SchemaGraph, EncodeError> {
    let mut ctx = GraphCtx::new(&graph.defs)?;
    let root = ctx.encode_type(&graph.root)?;
    Ok(wire::SchemaGraph {
        type_nodes: ctx.type_nodes,
        defs: ctx.defs,
        root,
    })
}

/// Incremental builder for a single flat [`wire::SchemaGraph`] that holds
/// several independent root types in one shared `type-nodes` pool.
///
/// Used by the agent-layer conversions, where one agent type carries a single
/// `schema-graph` whose `defs` are shared and whose constructor / method /
/// config schema roots are `type-node-index` values into that same graph (see
/// [`crate::schema::agent::wit`]).
///
/// Seed the builder with the agent's named definitions via [`GraphEncoder::new`],
/// then call [`GraphEncoder::encode_type`] for each inline root type (collecting
/// the returned indices), and finally [`GraphEncoder::finish`] to obtain the
/// graph with a placeholder root.
pub struct GraphEncoder {
    ctx: GraphCtx,
}

impl GraphEncoder {
    /// Create a builder seeded with the given named definitions. Each def body
    /// is encoded eagerly so forward [`SchemaType::Ref`] references resolve.
    pub fn new(defs: &[SchemaTypeDef]) -> Result<Self, EncodeError> {
        Ok(Self {
            ctx: GraphCtx::new(defs)?,
        })
    }

    /// Flatten one (possibly recursive) schema type into the shared pool and
    /// return its `type-node-index`.
    pub fn encode_type(&mut self, ty: &SchemaType) -> Result<wire::TypeNodeIndex, EncodeError> {
        self.ctx.encode_type(ty)
    }

    /// Finish the graph. The `root` field is a structural placeholder (an empty
    /// record) — agent-layer carriers never consult it; the real roots are the
    /// indices returned by [`GraphEncoder::encode_type`].
    pub fn finish(mut self) -> wire::SchemaGraph {
        let root = self.ctx.push_body(
            wire::SchemaTypeBody::RecordType(Vec::new()),
            &MetadataEnvelope::default(),
        );
        wire::SchemaGraph {
            type_nodes: self.ctx.type_nodes,
            defs: self.ctx.defs,
            root,
        }
    }
}

/// Encode a value tree without resolving capability handles (host / feature-neutral).
///
/// This is the pure path used everywhere a value tree cannot contain a live
/// quota token (no resource table is available). A [`SchemaValue::QuotaToken`]
/// snapshot is rejected with [`EncodeError::QuotaTokenNotTransportable`]; use
/// the host-side [`encode_value_with`] when quota tokens may be present.
#[cfg(not(all(feature = "guest", not(feature = "host"))))]
pub fn encode_value(value: &SchemaValue) -> Result<wire::SchemaValueTree, EncodeError> {
    encode_value_inner(
        value,
        &mut |_snapshot| Err(EncodeError::QuotaTokenNotTransportable),
        &mut |_snapshot| Err(EncodeError::SecretNotTransportable),
        &mut |_snapshot| Err(EncodeError::PermissionCardNotTransportable),
    )
}

/// Encode a value tree on a guest, transferring each [`SchemaValue::QuotaToken`]
/// owned handle into the wire tree as a `quota-token-handle` node.
///
/// A guest holds quota tokens as opaque, affine owned handles. Lowering a value
/// that contains a token moves the underlying `own<quota-token>` resource into
/// the wire tree (first encode wins); after this the originating
/// [`SchemaValue`] / SDK token wrapper is empty.
///
/// A preflight pass runs first so that an aliased handle (the same token
/// appearing twice) or an already-consumed handle is reported
/// ([`EncodeError::AliasedQuotaTokenHandle`] / [`EncodeError::QuotaTokenAlreadyConsumed`])
/// *before* any handle is moved, so a failed encode never destroys a still-valid
/// token.
#[cfg(all(feature = "guest", not(feature = "host")))]
pub fn encode_value(value: &SchemaValue) -> Result<wire::SchemaValueTree, EncodeError> {
    preflight_guest_handles(value)?;
    encode_value_inner(
        value,
        &mut |handle| {
            let owned = handle
                .take()
                .ok_or(EncodeError::QuotaTokenAlreadyConsumed)?;
            Ok(wire::SchemaValueNode::QuotaTokenHandle(owned))
        },
        &mut |handle| {
            let owned = handle.take().ok_or(EncodeError::SecretAlreadyConsumed)?;
            Ok(wire::SchemaValueNode::SecretValue(owned))
        },
        &mut |handle| {
            let owned = handle
                .take()
                .ok_or(EncodeError::PermissionCardAlreadyConsumed)?;
            Ok(wire::SchemaValueNode::PermissionCardHandle(owned))
        },
    )
}

/// Walk a value tree and verify that every quota-token handle is still present
/// and unique, without taking any handle. Returns an error if a handle was
/// already consumed or the same handle appears more than once.
#[cfg(all(feature = "guest", not(feature = "host")))]
fn preflight_guest_handles(value: &SchemaValue) -> Result<(), EncodeError> {
    fn walk(
        value: &SchemaValue,
        seen_quota: &mut std::collections::HashSet<*const ()>,
        seen_secret: &mut std::collections::HashSet<*const ()>,
        seen_permission_card: &mut std::collections::HashSet<*const ()>,
    ) -> Result<(), EncodeError> {
        match value {
            SchemaValue::QuotaToken(handle) => {
                if !handle.is_present() {
                    return Err(EncodeError::QuotaTokenAlreadyConsumed);
                }
                if !seen_quota.insert(handle.cell_id()) {
                    return Err(EncodeError::AliasedQuotaTokenHandle);
                }
                Ok(())
            }
            SchemaValue::Secret(handle) => {
                if !handle.is_present() {
                    return Err(EncodeError::SecretAlreadyConsumed);
                }
                if !seen_secret.insert(handle.cell_id()) {
                    return Err(EncodeError::AliasedSecretHandle);
                }
                Ok(())
            }
            SchemaValue::PermissionCard(handle) => {
                if !handle.is_present() {
                    return Err(EncodeError::PermissionCardAlreadyConsumed);
                }
                if !seen_permission_card.insert(handle.cell_id()) {
                    return Err(EncodeError::AliasedPermissionCardHandle);
                }
                Ok(())
            }
            SchemaValue::Record { fields } => {
                for f in fields {
                    walk(f, seen_quota, seen_secret, seen_permission_card)?;
                }
                Ok(())
            }
            SchemaValue::Tuple { elements }
            | SchemaValue::List { elements }
            | SchemaValue::FixedList { elements } => {
                for e in elements {
                    walk(e, seen_quota, seen_secret, seen_permission_card)?;
                }
                Ok(())
            }
            SchemaValue::Variant(p) => {
                if let Some(payload) = &p.payload {
                    walk(payload, seen_quota, seen_secret, seen_permission_card)?;
                }
                Ok(())
            }
            SchemaValue::Map { entries } => {
                for (k, v) in entries {
                    walk(k, seen_quota, seen_secret, seen_permission_card)?;
                    walk(v, seen_quota, seen_secret, seen_permission_card)?;
                }
                Ok(())
            }
            SchemaValue::Option { inner } => {
                if let Some(inner) = inner {
                    walk(inner, seen_quota, seen_secret, seen_permission_card)?;
                }
                Ok(())
            }
            SchemaValue::Result(p) => {
                let inner = match p {
                    ResultValuePayload::Ok { value } | ResultValuePayload::Err { value } => value,
                };
                if let Some(inner) = inner {
                    walk(inner, seen_quota, seen_secret, seen_permission_card)?;
                }
                Ok(())
            }
            SchemaValue::Union(p) => walk(&p.body, seen_quota, seen_secret, seen_permission_card),
            _ => Ok(()),
        }
    }

    let mut seen_quota = std::collections::HashSet::new();
    let mut seen_secret = std::collections::HashSet::new();
    let mut seen_permission_card = std::collections::HashSet::new();
    walk(
        value,
        &mut seen_quota,
        &mut seen_secret,
        &mut seen_permission_card,
    )
}

/// Encode a value tree, turning each [`SchemaValue::QuotaToken`] snapshot into a
/// fresh owned `quota-token` handle via the supplied resolver.
///
/// Used on the host when lowering a value tree to a guest (agent invocation
/// input, RPC input, invocation result), where the snapshot must be converted
/// back into an opaque, unforgeable handle.
///
/// If encoding fails after one or more handles were already minted (for example
/// a later snapshot's `handle_from_snapshot` returns an error), every handle
/// that was created so far is released through
/// [`super::QuotaTokenResolver::drop_handle`] so none leak from the resource
/// table.
#[cfg(all(feature = "host", not(feature = "guest")))]
pub fn encode_value_with<
    R: super::QuotaTokenResolver + super::SecretResolver + super::PermissionCardResolver,
>(
    value: &SchemaValue,
    resolver: &mut R,
) -> Result<wire::SchemaValueTree, EncodeError> {
    let resolver = std::cell::RefCell::new(resolver);
    let mut ctx = ValueCtx::default();
    let root = ctx.encode(
        value,
        &mut |snapshot| {
            let mut resolver = resolver.borrow_mut();
            let handle = resolver
                .handle_from_snapshot(snapshot)
                .map_err(|e| EncodeError::QuotaResolver(e.to_string()))?;
            Ok(wire::SchemaValueNode::QuotaTokenHandle(handle))
        },
        &mut |snapshot| {
            let mut resolver = resolver.borrow_mut();
            let handle = resolver
                .secret_handle_from_snapshot(snapshot)
                .map_err(|e| EncodeError::SecretResolver(e.to_string()))?;
            Ok(wire::SchemaValueNode::SecretValue(handle))
        },
        &mut |snapshot| {
            let mut resolver = resolver.borrow_mut();
            let handle = resolver
                .permission_card_handle_from_snapshot(snapshot)
                .map_err(|e| EncodeError::PermissionCardResolver(e.to_string()))?;
            Ok(wire::SchemaValueNode::PermissionCardHandle(handle))
        },
    );
    match root {
        Ok(root) => Ok(wire::SchemaValueTree {
            value_nodes: ctx.value_nodes,
            root,
        }),
        Err(err) => {
            let mut resolver = resolver.borrow_mut();
            for node in ctx.value_nodes {
                match node {
                    wire::SchemaValueNode::QuotaTokenHandle(handle) => resolver.drop_handle(handle),
                    wire::SchemaValueNode::SecretValue(handle) => {
                        super::SecretResolver::drop_secret_handle(&mut **resolver, handle)
                    }
                    wire::SchemaValueNode::PermissionCardHandle(handle) => {
                        super::PermissionCardResolver::drop_permission_card_handle(
                            &mut **resolver,
                            handle,
                        )
                    }
                    _ => {}
                }
            }
            Err(err)
        }
    }
}

fn encode_value_inner(
    value: &SchemaValue,
    quota: &mut dyn FnMut(&QuotaTokenVariantValue) -> Result<wire::SchemaValueNode, EncodeError>,
    secret: &mut dyn FnMut(&SecretVariantValue) -> Result<wire::SchemaValueNode, EncodeError>,
    permission_card: &mut dyn FnMut(
        &PermissionCardVariantValue,
    ) -> Result<wire::SchemaValueNode, EncodeError>,
) -> Result<wire::SchemaValueTree, EncodeError> {
    let mut ctx = ValueCtx::default();
    let root = ctx.encode(value, quota, secret, permission_card)?;
    Ok(wire::SchemaValueTree {
        value_nodes: ctx.value_nodes,
        root,
    })
}

pub fn encode_typed(typed: &TypedSchemaValue) -> Result<wire::TypedSchemaValue, EncodeError> {
    Ok(wire::TypedSchemaValue {
        graph: encode_graph(typed.graph())?,
        value: encode_value(typed.value())?,
    })
}

struct GraphCtx {
    type_nodes: Vec<wire::SchemaTypeNode>,
    defs: Vec<wire::SchemaTypeDef>,
    /// Maps a `TypeId` to its index in `defs`.
    def_index: HashMap<TypeId, wire::DefIndex>,
}

impl GraphCtx {
    fn new(defs: &[SchemaTypeDef]) -> Result<Self, EncodeError> {
        let mut ctx = GraphCtx {
            type_nodes: Vec::new(),
            defs: Vec::with_capacity(defs.len()),
            def_index: HashMap::with_capacity(defs.len()),
        };
        // Reserve def slots first so `ref-type` can resolve forward references
        // during type-body encoding.
        for def in defs {
            if ctx.def_index.contains_key(&def.id) {
                return Err(EncodeError::DuplicateTypeId(def.id.clone()));
            }
            let idx = ctx.defs.len() as wire::DefIndex;
            ctx.def_index.insert(def.id.clone(), idx);
            ctx.defs.push(wire::SchemaTypeDef {
                id: def.id.0.clone(),
                name: def.name.clone(),
                body: -1,
            });
        }
        // Encode each def body now that the index is fully populated.
        for def in defs {
            let body = ctx.encode_type(&def.body)?;
            let idx = ctx.def_index[&def.id];
            ctx.defs[idx as usize].body = body;
        }
        Ok(ctx)
    }

    fn push_body(
        &mut self,
        body: wire::SchemaTypeBody,
        metadata: &MetadataEnvelope,
    ) -> wire::TypeNodeIndex {
        let idx = self.type_nodes.len() as wire::TypeNodeIndex;
        self.type_nodes.push(wire::SchemaTypeNode {
            body,
            metadata: encode_metadata(metadata),
        });
        idx
    }

    fn encode_type(&mut self, ty: &SchemaType) -> Result<wire::TypeNodeIndex, EncodeError> {
        let body = match ty {
            SchemaType::Ref { id, .. } => {
                let def_idx = *self
                    .def_index
                    .get(id)
                    .ok_or_else(|| EncodeError::UnknownTypeId(id.clone()))?;
                wire::SchemaTypeBody::RefType(def_idx)
            }
            SchemaType::Bool { .. } => wire::SchemaTypeBody::BoolType,
            SchemaType::S8 { restrictions, .. } => {
                wire::SchemaTypeBody::S8Type(encode_numeric(restrictions))
            }
            SchemaType::S16 { restrictions, .. } => {
                wire::SchemaTypeBody::S16Type(encode_numeric(restrictions))
            }
            SchemaType::S32 { restrictions, .. } => {
                wire::SchemaTypeBody::S32Type(encode_numeric(restrictions))
            }
            SchemaType::S64 { restrictions, .. } => {
                wire::SchemaTypeBody::S64Type(encode_numeric(restrictions))
            }
            SchemaType::U8 { restrictions, .. } => {
                wire::SchemaTypeBody::U8Type(encode_numeric(restrictions))
            }
            SchemaType::U16 { restrictions, .. } => {
                wire::SchemaTypeBody::U16Type(encode_numeric(restrictions))
            }
            SchemaType::U32 { restrictions, .. } => {
                wire::SchemaTypeBody::U32Type(encode_numeric(restrictions))
            }
            SchemaType::U64 { restrictions, .. } => {
                wire::SchemaTypeBody::U64Type(encode_numeric(restrictions))
            }
            SchemaType::F32 { restrictions, .. } => {
                wire::SchemaTypeBody::F32Type(encode_numeric(restrictions))
            }
            SchemaType::F64 { restrictions, .. } => {
                wire::SchemaTypeBody::F64Type(encode_numeric(restrictions))
            }
            SchemaType::Char { .. } => wire::SchemaTypeBody::CharType,
            SchemaType::String { .. } => wire::SchemaTypeBody::StringType,
            SchemaType::Record { fields, .. } => {
                let encoded = fields
                    .iter()
                    .map(|f| self.encode_field(f))
                    .collect::<Result<Vec<_>, _>>()?;
                wire::SchemaTypeBody::RecordType(encoded)
            }
            SchemaType::Variant { cases, .. } => {
                let encoded = cases
                    .iter()
                    .map(|c| self.encode_case(c))
                    .collect::<Result<Vec<_>, _>>()?;
                wire::SchemaTypeBody::VariantType(encoded)
            }
            SchemaType::Enum { cases, .. } => wire::SchemaTypeBody::EnumType(cases.clone()),
            SchemaType::Flags { flags, .. } => wire::SchemaTypeBody::FlagsType(flags.clone()),
            SchemaType::Tuple { elements, .. } => {
                let encoded = elements
                    .iter()
                    .map(|e| self.encode_type(e))
                    .collect::<Result<Vec<_>, _>>()?;
                wire::SchemaTypeBody::TupleType(encoded)
            }
            SchemaType::List { element, .. } => {
                let inner = self.encode_type(element)?;
                wire::SchemaTypeBody::ListType(inner)
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner = self.encode_type(element)?;
                wire::SchemaTypeBody::FixedListType(wire::FixedListSpec {
                    element: inner,
                    length: *length,
                })
            }
            SchemaType::Map { key, value, .. } => {
                let key = self.encode_type(key)?;
                let value = self.encode_type(value)?;
                wire::SchemaTypeBody::MapType(wire::MapSpec { key, value })
            }
            SchemaType::Option { inner, .. } => {
                let inner = self.encode_type(inner)?;
                wire::SchemaTypeBody::OptionType(inner)
            }
            SchemaType::Result { spec, .. } => {
                let ok = match &spec.ok {
                    Some(t) => Some(self.encode_type(t)?),
                    None => None,
                };
                let err = match &spec.err {
                    Some(t) => Some(self.encode_type(t)?),
                    None => None,
                };
                wire::SchemaTypeBody::ResultType(wire::ResultSpec { ok, err })
            }
            SchemaType::Text { restrictions, .. } => {
                wire::SchemaTypeBody::TextType(encode_text(restrictions))
            }
            SchemaType::Binary { restrictions, .. } => {
                wire::SchemaTypeBody::BinaryType(encode_binary(restrictions))
            }
            SchemaType::Path { spec, .. } => wire::SchemaTypeBody::PathType(encode_path(spec)),
            SchemaType::Url { restrictions, .. } => {
                wire::SchemaTypeBody::UrlType(encode_url(restrictions))
            }
            SchemaType::Datetime { .. } => wire::SchemaTypeBody::DatetimeType,
            SchemaType::Duration { .. } => wire::SchemaTypeBody::DurationType,
            SchemaType::Quantity { spec, .. } => {
                wire::SchemaTypeBody::QuantityType(encode_quantity(spec))
            }
            SchemaType::Union { spec, .. } => {
                let branches = spec
                    .branches
                    .iter()
                    .map(|b| self.encode_branch(b))
                    .collect::<Result<Vec<_>, _>>()?;
                wire::SchemaTypeBody::UnionType(wire::UnionSpec { branches })
            }
            SchemaType::Secret { spec, .. } => {
                let inner = self.encode_type(&spec.inner)?;
                wire::SchemaTypeBody::SecretType(wire::SecretSpec {
                    inner,
                    category: spec.category.clone(),
                })
            }
            SchemaType::QuotaToken { spec, .. } => {
                wire::SchemaTypeBody::QuotaTokenType(wire::QuotaTokenSpec {
                    resource_name: spec.resource_name.clone(),
                })
            }
            SchemaType::PermissionCard { spec, .. } => {
                wire::SchemaTypeBody::PermissionCardType(wire::PermissionCardSpec {
                    polymorphic: spec.polymorphic,
                })
            }
            SchemaType::Future { inner, .. } => {
                let inner = match inner {
                    Some(t) => Some(self.encode_type(t)?),
                    None => None,
                };
                wire::SchemaTypeBody::FutureType(inner)
            }
            SchemaType::Stream { inner, .. } => {
                let inner = match inner {
                    Some(t) => Some(self.encode_type(t)?),
                    None => None,
                };
                wire::SchemaTypeBody::StreamType(inner)
            }
        };
        Ok(self.push_body(body, ty.metadata()))
    }

    fn encode_field(
        &mut self,
        field: &NamedFieldType,
    ) -> Result<wire::NamedFieldType, EncodeError> {
        let body = self.encode_type(&field.body)?;
        Ok(wire::NamedFieldType {
            name: field.name.clone(),
            body,
            metadata: encode_metadata(&field.metadata),
        })
    }

    fn encode_case(
        &mut self,
        case: &VariantCaseType,
    ) -> Result<wire::VariantCaseType, EncodeError> {
        let payload = match &case.payload {
            Some(t) => Some(self.encode_type(t)?),
            None => None,
        };
        Ok(wire::VariantCaseType {
            name: case.name.clone(),
            payload,
            metadata: encode_metadata(&case.metadata),
        })
    }

    fn encode_branch(&mut self, branch: &UnionBranch) -> Result<wire::UnionBranch, EncodeError> {
        let body = self.encode_type(&branch.body)?;
        Ok(wire::UnionBranch {
            tag: branch.tag.clone(),
            body,
            discriminator: encode_discriminator(&branch.discriminator),
            metadata: encode_metadata(&branch.metadata),
        })
    }
}

pub fn encode_metadata(m: &MetadataEnvelope) -> wire::MetadataEnvelope {
    wire::MetadataEnvelope {
        doc: m.doc.clone(),
        aliases: m.aliases.clone(),
        examples: m.examples.clone(),
        deprecated: m.deprecated.clone(),
        role: m.role.as_ref().map(|r| match r {
            Role::Multimodal => wire::Role::Multimodal,
            Role::UnstructuredText => wire::Role::UnstructuredText,
            Role::UnstructuredBinary => wire::Role::UnstructuredBinary,
            Role::Other(s) => wire::Role::Other(s.clone()),
        }),
    }
}

fn encode_numeric(r: &Option<NumericRestrictions>) -> Option<wire::NumericRestrictions> {
    r.as_ref().map(|r| wire::NumericRestrictions {
        min: r.min.map(encode_numeric_bound),
        max: r.max.map(encode_numeric_bound),
        unit: r.unit.clone(),
    })
}

fn encode_numeric_bound(b: NumericBound) -> wire::NumericBound {
    match b {
        NumericBound::Signed(v) => wire::NumericBound::Signed(v),
        NumericBound::Unsigned(v) => wire::NumericBound::Unsigned(v),
        NumericBound::FloatBits(bits) => wire::NumericBound::FloatBits(bits),
    }
}

fn encode_text(r: &TextRestrictions) -> wire::TextRestrictions {
    wire::TextRestrictions {
        languages: r.languages.clone(),
        min_length: r.min_length,
        max_length: r.max_length,
        regex: r.regex.clone(),
    }
}

fn encode_binary(r: &BinaryRestrictions) -> wire::BinaryRestrictions {
    wire::BinaryRestrictions {
        mime_types: r.mime_types.clone(),
        min_bytes: r.min_bytes,
        max_bytes: r.max_bytes,
    }
}

fn encode_path(p: &PathSpec) -> wire::PathSpec {
    wire::PathSpec {
        direction: match p.direction {
            PathDirection::Input => wire::PathDirection::Input,
            PathDirection::Output => wire::PathDirection::Output,
            PathDirection::InOut => wire::PathDirection::InOut,
        },
        kind: match p.kind {
            PathKind::File => wire::PathKind::File,
            PathKind::Directory => wire::PathKind::Directory,
            PathKind::Any => wire::PathKind::Any,
        },
        allowed_mime_types: p.allowed_mime_types.clone(),
        allowed_extensions: p.allowed_extensions.clone(),
    }
}

fn encode_url(r: &UrlRestrictions) -> wire::UrlRestrictions {
    wire::UrlRestrictions {
        allowed_schemes: r.allowed_schemes.clone(),
        allowed_hosts: r.allowed_hosts.clone(),
    }
}

fn encode_quantity(q: &QuantitySpec) -> wire::QuantitySpec {
    wire::QuantitySpec {
        base_unit: q.base_unit.clone(),
        allowed_suffixes: q.allowed_suffixes.clone(),
        min: q.min.as_ref().map(encode_quantity_value),
        max: q.max.as_ref().map(encode_quantity_value),
    }
}

fn encode_quantity_value(q: &QuantityValue) -> wire::QuantityValue {
    wire::QuantityValue {
        mantissa: q.mantissa,
        scale: q.scale,
        unit: q.unit.clone(),
    }
}

fn encode_discriminator(d: &DiscriminatorRule) -> wire::DiscriminatorRule {
    match d {
        DiscriminatorRule::Prefix { prefix } => wire::DiscriminatorRule::Prefix(prefix.clone()),
        DiscriminatorRule::Suffix { suffix } => wire::DiscriminatorRule::Suffix(suffix.clone()),
        DiscriminatorRule::Contains { substring } => {
            wire::DiscriminatorRule::Contains(substring.clone())
        }
        DiscriminatorRule::Regex { regex } => wire::DiscriminatorRule::Regex(regex.clone()),
        DiscriminatorRule::FieldEquals(fd) => {
            wire::DiscriminatorRule::FieldEquals(wire::FieldDiscriminator {
                field_name: fd.field_name.clone(),
                literal: fd.literal.clone(),
            })
        }
        DiscriminatorRule::FieldAbsent { field_name } => {
            wire::DiscriminatorRule::FieldAbsent(field_name.clone())
        }
    }
}

#[derive(Default)]
struct ValueCtx {
    value_nodes: Vec<wire::SchemaValueNode>,
}

impl ValueCtx {
    fn push(&mut self, node: wire::SchemaValueNode) -> wire::ValueNodeIndex {
        let idx = self.value_nodes.len() as wire::ValueNodeIndex;
        self.value_nodes.push(node);
        idx
    }

    fn encode(
        &mut self,
        value: &SchemaValue,
        quota: &mut dyn FnMut(
            &QuotaTokenVariantValue,
        ) -> Result<wire::SchemaValueNode, EncodeError>,
        secret: &mut dyn FnMut(&SecretVariantValue) -> Result<wire::SchemaValueNode, EncodeError>,
        permission_card: &mut dyn FnMut(
            &PermissionCardVariantValue,
        ) -> Result<wire::SchemaValueNode, EncodeError>,
    ) -> Result<wire::ValueNodeIndex, EncodeError> {
        let node = match value {
            SchemaValue::Bool(b) => wire::SchemaValueNode::BoolValue(*b),
            SchemaValue::S8(v) => wire::SchemaValueNode::S8Value(*v),
            SchemaValue::S16(v) => wire::SchemaValueNode::S16Value(*v),
            SchemaValue::S32(v) => wire::SchemaValueNode::S32Value(*v),
            SchemaValue::S64(v) => wire::SchemaValueNode::S64Value(*v),
            SchemaValue::U8(v) => wire::SchemaValueNode::U8Value(*v),
            SchemaValue::U16(v) => wire::SchemaValueNode::U16Value(*v),
            SchemaValue::U32(v) => wire::SchemaValueNode::U32Value(*v),
            SchemaValue::U64(v) => wire::SchemaValueNode::U64Value(*v),
            SchemaValue::F32(v) => wire::SchemaValueNode::F32Value(*v),
            SchemaValue::F64(v) => wire::SchemaValueNode::F64Value(*v),
            SchemaValue::Char(c) => wire::SchemaValueNode::CharValue(*c),
            SchemaValue::String(s) => wire::SchemaValueNode::StringValue(s.clone()),
            SchemaValue::Record { fields } => {
                let mut indices = Vec::with_capacity(fields.len());
                for v in fields {
                    indices.push(self.encode(v, quota, secret, permission_card)?);
                }
                wire::SchemaValueNode::RecordValue(indices)
            }
            SchemaValue::Variant(p) => {
                let payload = match &p.payload {
                    Some(v) => Some(self.encode(v, quota, secret, permission_card)?),
                    None => None,
                };
                wire::SchemaValueNode::VariantValue(wire::VariantValuePayload {
                    case: p.case,
                    payload,
                })
            }
            SchemaValue::Enum { case } => wire::SchemaValueNode::EnumValue(*case),
            SchemaValue::Flags { bits } => wire::SchemaValueNode::FlagsValue(bits.clone()),
            SchemaValue::Tuple { elements } => {
                let mut indices = Vec::with_capacity(elements.len());
                for v in elements {
                    indices.push(self.encode(v, quota, secret, permission_card)?);
                }
                wire::SchemaValueNode::TupleValue(indices)
            }
            SchemaValue::List { elements } => {
                let mut indices = Vec::with_capacity(elements.len());
                for v in elements {
                    indices.push(self.encode(v, quota, secret, permission_card)?);
                }
                wire::SchemaValueNode::ListValue(indices)
            }
            SchemaValue::FixedList { elements } => {
                let mut indices = Vec::with_capacity(elements.len());
                for v in elements {
                    indices.push(self.encode(v, quota, secret, permission_card)?);
                }
                wire::SchemaValueNode::FixedListValue(indices)
            }
            SchemaValue::Map { entries } => {
                let mut encoded = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    encoded.push(wire::MapEntry {
                        key: self.encode(k, quota, secret, permission_card)?,
                        value: self.encode(v, quota, secret, permission_card)?,
                    });
                }
                wire::SchemaValueNode::MapValue(encoded)
            }
            SchemaValue::Option { inner } => {
                let inner = match inner {
                    Some(v) => Some(self.encode(v, quota, secret, permission_card)?),
                    None => None,
                };
                wire::SchemaValueNode::OptionValue(inner)
            }
            SchemaValue::Result(p) => {
                let payload = match p {
                    ResultValuePayload::Ok { value } => {
                        let v = match value {
                            Some(v) => Some(self.encode(v, quota, secret, permission_card)?),
                            None => None,
                        };
                        wire::ResultValuePayload::OkValue(v)
                    }
                    ResultValuePayload::Err { value } => {
                        let v = match value {
                            Some(v) => Some(self.encode(v, quota, secret, permission_card)?),
                            None => None,
                        };
                        wire::ResultValuePayload::ErrValue(v)
                    }
                };
                wire::SchemaValueNode::ResultValue(payload)
            }
            SchemaValue::Text(p) => wire::SchemaValueNode::TextValue(wire::TextValuePayload {
                text: p.text.clone(),
                language: p.language.clone(),
            }),
            SchemaValue::Binary(p) => {
                wire::SchemaValueNode::BinaryValue(wire::BinaryValuePayload {
                    bytes: p.bytes.clone(),
                    mime_type: p.mime_type.clone(),
                })
            }
            SchemaValue::Path { path } => wire::SchemaValueNode::PathValue(path.clone()),
            SchemaValue::Url { url } => wire::SchemaValueNode::UrlValue(url.clone()),
            SchemaValue::Datetime { value } => {
                let seconds = value.timestamp();
                let nanoseconds = value.timestamp_subsec_nanos();
                wire::SchemaValueNode::DatetimeValue(wire::Datetime {
                    seconds,
                    nanoseconds,
                })
            }
            SchemaValue::Duration(d) => {
                wire::SchemaValueNode::DurationValue(wire::DurationValuePayload {
                    nanoseconds: d.nanoseconds,
                })
            }
            SchemaValue::Quantity(q) => {
                wire::SchemaValueNode::QuantityValueNode(wire::QuantityValue {
                    mantissa: q.mantissa,
                    scale: q.scale,
                    unit: q.unit.clone(),
                })
            }
            SchemaValue::Union(p) => {
                let body = self.encode(&p.body, quota, secret, permission_card)?;
                wire::SchemaValueNode::UnionValue(wire::UnionValuePayload {
                    tag: p.tag.clone(),
                    body,
                })
            }
            SchemaValue::Secret(s) => secret(s)?,
            SchemaValue::QuotaToken(q) => quota(q)?,
            SchemaValue::PermissionCard(p) => permission_card(p)?,
        };
        Ok(self.push(node))
    }
}
