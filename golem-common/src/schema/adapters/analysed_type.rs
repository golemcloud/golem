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

//! `AnalysedType` ↔ `SchemaType` conversion.
//!
//! Two front doors:
//!
//! - [`analysed_type_to_schema_type_inline`] / [`schema_type_to_analysed_type`]
//!   produce inline (anonymous) types. Names from named legacy composites are
//!   not preserved.
//! - [`analysed_type_to_schema_graph`] / [`schema_graph_to_analysed_type`]
//!   produce a self-contained graph: legacy composites with `name` / `owner`
//!   become `SchemaTypeDef` entries, referenced via `SchemaType::Ref`. Anonymous
//!   types stay inline.

use std::collections::HashMap;

use golem_wasm::analysis::{
    AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF32, TypeF64,
    TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult, TypeS8, TypeS16, TypeS32, TypeS64,
    TypeStr, TypeTuple, TypeU8, TypeU16, TypeU32, TypeU64, TypeVariant,
};

use crate::schema::adapters::error::{SchemaAdapterError, legacy_type_id};
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{
    NamedFieldType, ResultSpec, SchemaType, VariantCaseType,
};

/// Convert a legacy [`AnalysedType`] tree into an inline [`SchemaType`] tree.
///
/// Names (`owner` / `name`) on named composite variants are **dropped**; use
/// [`analysed_type_to_schema_graph`] when you need names preserved.
pub fn analysed_type_to_schema_type_inline(
    ty: &AnalysedType,
) -> Result<SchemaType, SchemaAdapterError> {
    inline_no_naming(ty)
}

/// Convert a legacy [`AnalysedType`] tree into a self-contained
/// [`SchemaGraph`]. Named composites become [`SchemaTypeDef`] entries
/// referenced via [`SchemaType::Ref`]; anonymous composites stay inline.
pub fn analysed_type_to_schema_graph(
    ty: &AnalysedType,
) -> Result<SchemaGraph, SchemaAdapterError> {
    let mut builder = GraphBuilder::default();
    let root = builder.lower(ty)?;
    Ok(SchemaGraph {
        defs: builder.into_defs(),
        root,
    })
}

/// Reverse: extract the root inline [`SchemaType`] of a graph back into an
/// [`AnalysedType`] tree. Fails on rich scalars, unions, capabilities, and
/// recursive cycles.
pub fn schema_graph_to_analysed_type(
    graph: &SchemaGraph,
) -> Result<AnalysedType, SchemaAdapterError> {
    let mut ctx = ReverseCtx::new(graph);
    ctx.lower(&graph.root)
}

/// Reverse a single [`SchemaType`] back into an [`AnalysedType`]. The graph
/// is needed only to resolve [`SchemaType::Ref`] cases.
pub fn schema_type_to_analysed_type(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<AnalysedType, SchemaAdapterError> {
    let mut ctx = ReverseCtx::new(graph);
    ctx.lower(ty)
}

// --------------------------------------------------------------------------
// Forward: AnalysedType → SchemaType (inline, names dropped)
// --------------------------------------------------------------------------

fn inline_no_naming(ty: &AnalysedType) -> Result<SchemaType, SchemaAdapterError> {
    match ty {
        AnalysedType::Bool(_) => Ok(SchemaType::bool()),
        AnalysedType::S8(_) => Ok(SchemaType::s8()),
        AnalysedType::S16(_) => Ok(SchemaType::s16()),
        AnalysedType::S32(_) => Ok(SchemaType::s32()),
        AnalysedType::S64(_) => Ok(SchemaType::s64()),
        AnalysedType::U8(_) => Ok(SchemaType::u8()),
        AnalysedType::U16(_) => Ok(SchemaType::u16()),
        AnalysedType::U32(_) => Ok(SchemaType::u32()),
        AnalysedType::U64(_) => Ok(SchemaType::u64()),
        AnalysedType::F32(_) => Ok(SchemaType::f32()),
        AnalysedType::F64(_) => Ok(SchemaType::f64()),
        AnalysedType::Chr(_) => Ok(SchemaType::char()),
        AnalysedType::Str(_) => Ok(SchemaType::string()),
        AnalysedType::Handle(_) => Err(SchemaAdapterError::LegacyHandle),
        AnalysedType::List(TypeList { inner, .. }) => {
            Ok(SchemaType::list(inline_no_naming(inner)?))
        }
        AnalysedType::Tuple(TypeTuple { items, .. }) => Ok(SchemaType::tuple(
            items.iter().map(inline_no_naming).collect::<Result<_, _>>()?,
        )),
        AnalysedType::Record(TypeRecord { fields, .. }) => {
            let fields = fields
                .iter()
                .map(|NameTypePair { name, typ }| {
                    Ok(NamedFieldType {
                        name: name.clone(),
                        body: inline_no_naming(typ)?,
                        metadata: MetadataEnvelope::default(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SchemaType::record(fields))
        }
        AnalysedType::Variant(TypeVariant { cases, .. }) => {
            let cases = cases
                .iter()
                .map(|NameOptionTypePair { name, typ }| {
                    let payload = match typ {
                        Some(t) => Some(inline_no_naming(t)?),
                        None => None,
                    };
                    Ok(VariantCaseType {
                        name: name.clone(),
                        payload,
                        metadata: MetadataEnvelope::default(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SchemaType::variant(cases))
        }
        AnalysedType::Enum(TypeEnum { cases, .. }) => Ok(SchemaType::r#enum(cases.clone())),
        AnalysedType::Flags(TypeFlags { names, .. }) => Ok(SchemaType::flags(names.clone())),
        AnalysedType::Option(TypeOption { inner, .. }) => {
            Ok(SchemaType::option(inline_no_naming(inner)?))
        }
        AnalysedType::Result(TypeResult { ok, err, .. }) => {
            let ok = match ok {
                Some(t) => Some(Box::new(inline_no_naming(t)?)),
                None => None,
            };
            let err = match err {
                Some(t) => Some(Box::new(inline_no_naming(t)?)),
                None => None,
            };
            Ok(SchemaType::result(ResultSpec { ok, err }))
        }
    }
}

// --------------------------------------------------------------------------
// Forward: AnalysedType → SchemaGraph (names preserved as defs)
// --------------------------------------------------------------------------

#[derive(Default)]
struct GraphBuilder {
    defs_by_id: HashMap<TypeId, SchemaTypeDef>,
}

impl GraphBuilder {
    fn lower(&mut self, ty: &AnalysedType) -> Result<SchemaType, SchemaAdapterError> {
        // If the legacy type carries `name` (and optional `owner`) we register
        // it as a def and return a Ref. Otherwise lower inline.
        let id = match legacy_id_of(ty)? {
            Some(id) => id,
            None => return self.lower_inline(ty),
        };

        if !self.defs_by_id.contains_key(&id) {
            // Reserve the def slot up front (anonymous body) so recursive
            // references through the same name terminate.
            self.defs_by_id.insert(
                id.clone(),
                SchemaTypeDef {
                    id: id.clone(),
                    name: None,
                    body: SchemaType::bool(), // placeholder, replaced below
                },
            );
            let body = self.lower_inline(ty)?;
            let display_name = analysed_type_name(ty).map(|s| s.to_string());
            let def = self.defs_by_id.get_mut(&id).unwrap();
            def.name = display_name;
            def.body = body;
        } else {
            // Same TypeId already registered. We require the body to be
            // structurally equal — but recomputing here would lose laziness.
            // Since legacy AnalysedType is tree-shaped, equal owner+name
            // implies structural equality in practice; the only way to get
            // a mismatch is hand-built `AnalysedType` graphs with the same
            // (owner, name) on different bodies.
            let body = self.lower_inline(ty)?;
            let existing_body = &self.defs_by_id[&id].body;
            if existing_body != &body {
                return Err(SchemaAdapterError::DuplicateTypeIdConflict(id));
            }
        }

        Ok(SchemaType::ref_to(id))
    }

    fn lower_inline(&mut self, ty: &AnalysedType) -> Result<SchemaType, SchemaAdapterError> {
        match ty {
            AnalysedType::Bool(_) => Ok(SchemaType::bool()),
            AnalysedType::S8(_) => Ok(SchemaType::s8()),
            AnalysedType::S16(_) => Ok(SchemaType::s16()),
            AnalysedType::S32(_) => Ok(SchemaType::s32()),
            AnalysedType::S64(_) => Ok(SchemaType::s64()),
            AnalysedType::U8(_) => Ok(SchemaType::u8()),
            AnalysedType::U16(_) => Ok(SchemaType::u16()),
            AnalysedType::U32(_) => Ok(SchemaType::u32()),
            AnalysedType::U64(_) => Ok(SchemaType::u64()),
            AnalysedType::F32(_) => Ok(SchemaType::f32()),
            AnalysedType::F64(_) => Ok(SchemaType::f64()),
            AnalysedType::Chr(_) => Ok(SchemaType::char()),
            AnalysedType::Str(_) => Ok(SchemaType::string()),
            AnalysedType::Handle(_) => Err(SchemaAdapterError::LegacyHandle),
            AnalysedType::List(TypeList { inner, .. }) => {
                Ok(SchemaType::list(self.lower(inner)?))
            }
            AnalysedType::Tuple(TypeTuple { items, .. }) => {
                let items = items
                    .iter()
                    .map(|t| self.lower(t))
                    .collect::<Result<_, _>>()?;
                Ok(SchemaType::tuple(items))
            }
            AnalysedType::Record(TypeRecord { fields, .. }) => {
                let fields = fields
                    .iter()
                    .map(|NameTypePair { name, typ }| {
                        Ok(NamedFieldType {
                            name: name.clone(),
                            body: self.lower(typ)?,
                            metadata: MetadataEnvelope::default(),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(SchemaType::record(fields))
            }
            AnalysedType::Variant(TypeVariant { cases, .. }) => {
                let cases = cases
                    .iter()
                    .map(|NameOptionTypePair { name, typ }| {
                        let payload = match typ {
                            Some(t) => Some(self.lower(t)?),
                            None => None,
                        };
                        Ok(VariantCaseType {
                            name: name.clone(),
                            payload,
                            metadata: MetadataEnvelope::default(),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(SchemaType::variant(cases))
            }
            AnalysedType::Enum(TypeEnum { cases, .. }) => Ok(SchemaType::r#enum(cases.clone())),
            AnalysedType::Flags(TypeFlags { names, .. }) => Ok(SchemaType::flags(names.clone())),
            AnalysedType::Option(TypeOption { inner, .. }) => {
                Ok(SchemaType::option(self.lower(inner)?))
            }
            AnalysedType::Result(TypeResult { ok, err, .. }) => {
                let ok = match ok {
                    Some(t) => Some(Box::new(self.lower(t)?)),
                    None => None,
                };
                let err = match err {
                    Some(t) => Some(Box::new(self.lower(t)?)),
                    None => None,
                };
                Ok(SchemaType::result(ResultSpec { ok, err }))
            }
        }
    }

    fn into_defs(self) -> Vec<SchemaTypeDef> {
        let mut defs: Vec<_> = self.defs_by_id.into_values().collect();
        defs.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        defs
    }
}

fn legacy_id_of(ty: &AnalysedType) -> Result<Option<TypeId>, SchemaAdapterError> {
    let (owner, name) = match ty {
        AnalysedType::Variant(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Result(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Option(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Enum(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Flags(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Record(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Tuple(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::List(t) => (t.owner.as_deref(), t.name.as_deref()),
        AnalysedType::Handle(_) => return Err(SchemaAdapterError::LegacyHandle),
        _ => (None, None),
    };
    legacy_type_id(owner, name)
}

fn analysed_type_name(ty: &AnalysedType) -> Option<&str> {
    match ty {
        AnalysedType::Variant(t) => t.name.as_deref(),
        AnalysedType::Result(t) => t.name.as_deref(),
        AnalysedType::Option(t) => t.name.as_deref(),
        AnalysedType::Enum(t) => t.name.as_deref(),
        AnalysedType::Flags(t) => t.name.as_deref(),
        AnalysedType::Record(t) => t.name.as_deref(),
        AnalysedType::Tuple(t) => t.name.as_deref(),
        AnalysedType::List(t) => t.name.as_deref(),
        _ => None,
    }
}

// --------------------------------------------------------------------------
// Reverse: SchemaType → AnalysedType (lossy outside the legacy-compatible
// subset)
// --------------------------------------------------------------------------

struct ReverseCtx<'a> {
    graph: &'a SchemaGraph,
    visiting: Vec<TypeId>,
}

impl<'a> ReverseCtx<'a> {
    fn new(graph: &'a SchemaGraph) -> Self {
        Self {
            graph,
            visiting: Vec::new(),
        }
    }

    fn lower(&mut self, ty: &SchemaType) -> Result<AnalysedType, SchemaAdapterError> {
        match ty {
            SchemaType::Bool { .. } => Ok(AnalysedType::Bool(TypeBool)),
            SchemaType::S8 { .. } => Ok(AnalysedType::S8(TypeS8)),
            SchemaType::S16 { .. } => Ok(AnalysedType::S16(TypeS16)),
            SchemaType::S32 { .. } => Ok(AnalysedType::S32(TypeS32)),
            SchemaType::S64 { .. } => Ok(AnalysedType::S64(TypeS64)),
            SchemaType::U8 { .. } => Ok(AnalysedType::U8(TypeU8)),
            SchemaType::U16 { .. } => Ok(AnalysedType::U16(TypeU16)),
            SchemaType::U32 { .. } => Ok(AnalysedType::U32(TypeU32)),
            SchemaType::U64 { .. } => Ok(AnalysedType::U64(TypeU64)),
            SchemaType::F32 { .. } => Ok(AnalysedType::F32(TypeF32)),
            SchemaType::F64 { .. } => Ok(AnalysedType::F64(TypeF64)),
            SchemaType::Char { .. } => Ok(AnalysedType::Chr(TypeChr)),
            SchemaType::String { .. } => Ok(AnalysedType::Str(TypeStr)),
            SchemaType::Ref { id, .. } => self.resolve_ref(id),
            SchemaType::Record { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|f| {
                        Ok(NameTypePair {
                            name: f.name.clone(),
                            typ: self.lower(&f.body)?,
                        })
                    })
                    .collect::<Result<_, _>>()?;
                Ok(AnalysedType::Record(TypeRecord {
                    name: None,
                    owner: None,
                    fields,
                }))
            }
            SchemaType::Variant { cases, .. } => {
                let cases = cases
                    .iter()
                    .map(|c| {
                        let typ = match &c.payload {
                            Some(p) => Some(self.lower(p)?),
                            None => None,
                        };
                        Ok(NameOptionTypePair {
                            name: c.name.clone(),
                            typ,
                        })
                    })
                    .collect::<Result<_, _>>()?;
                Ok(AnalysedType::Variant(TypeVariant {
                    name: None,
                    owner: None,
                    cases,
                }))
            }
            SchemaType::Enum { cases, .. } => Ok(AnalysedType::Enum(TypeEnum {
                name: None,
                owner: None,
                cases: cases.clone(),
            })),
            SchemaType::Flags { flags, .. } => Ok(AnalysedType::Flags(TypeFlags {
                name: None,
                owner: None,
                names: flags.clone(),
            })),
            SchemaType::Tuple { elements, .. } => {
                let items = elements
                    .iter()
                    .map(|e| self.lower(e))
                    .collect::<Result<_, _>>()?;
                Ok(AnalysedType::Tuple(TypeTuple {
                    name: None,
                    owner: None,
                    items,
                }))
            }
            SchemaType::List { element, .. } => Ok(AnalysedType::List(TypeList {
                name: None,
                owner: None,
                inner: Box::new(self.lower(element)?),
            })),
            SchemaType::Option { inner, .. } => Ok(AnalysedType::Option(TypeOption {
                name: None,
                owner: None,
                inner: Box::new(self.lower(inner)?),
            })),
            SchemaType::Result { spec, .. } => {
                let ok = match &spec.ok {
                    Some(t) => Some(Box::new(self.lower(t)?)),
                    None => None,
                };
                let err = match &spec.err {
                    Some(t) => Some(Box::new(self.lower(t)?)),
                    None => None,
                };
                Ok(AnalysedType::Result(TypeResult {
                    name: None,
                    owner: None,
                    ok,
                    err,
                }))
            }
            // -- All cases with no legacy counterpart --
            SchemaType::FixedList { .. } => Err(SchemaAdapterError::LossySchemaType(
                "FixedList has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Map { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Map has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Text { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Text rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Binary { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Binary rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Path { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Path rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Url { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Url rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Datetime { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Datetime rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Duration { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Duration rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Quantity { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Quantity rich scalar has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Union { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Union has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Secret { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Secret capability has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::QuotaToken { .. } => Err(SchemaAdapterError::LossySchemaType(
                "QuotaToken capability has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Future { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Future has no legacy AnalysedType counterpart".into(),
            )),
            SchemaType::Stream { .. } => Err(SchemaAdapterError::LossySchemaType(
                "Stream has no legacy AnalysedType counterpart".into(),
            )),
        }
    }

    fn resolve_ref(&mut self, id: &TypeId) -> Result<AnalysedType, SchemaAdapterError> {
        if self.visiting.iter().any(|x| x == id) {
            return Err(SchemaAdapterError::RecursiveRef(id.clone()));
        }
        let def = self
            .graph
            .lookup(id)
            .ok_or_else(|| SchemaAdapterError::DanglingRef(id.clone()))?;
        self.visiting.push(id.clone());
        let result = self.lower(&def.body);
        self.visiting.pop();
        // Re-attach the legacy display name if the def carries one. We only do
        // this on named composites — primitives never carry name/owner.
        result.map(|t| reattach_name(t, def.name.as_deref(), id))
    }
}

/// Re-attach a legacy display name (and synthesised owner from the TypeId
/// prefix) to a converted `AnalysedType`. Only applies to composite variants
/// that actually have `name`/`owner` fields.
fn reattach_name(
    ty: AnalysedType,
    display_name: Option<&str>,
    id: &TypeId,
) -> AnalysedType {
    let (owner, name) = split_type_id(id, display_name);
    match ty {
        AnalysedType::Record(mut r) => {
            r.name = name;
            r.owner = owner;
            AnalysedType::Record(r)
        }
        AnalysedType::Variant(mut v) => {
            v.name = name;
            v.owner = owner;
            AnalysedType::Variant(v)
        }
        AnalysedType::Enum(mut e) => {
            e.name = name;
            e.owner = owner;
            AnalysedType::Enum(e)
        }
        AnalysedType::Flags(mut f) => {
            f.name = name;
            f.owner = owner;
            AnalysedType::Flags(f)
        }
        AnalysedType::Tuple(mut t) => {
            t.name = name;
            t.owner = owner;
            AnalysedType::Tuple(t)
        }
        AnalysedType::List(mut l) => {
            l.name = name;
            l.owner = owner;
            AnalysedType::List(l)
        }
        AnalysedType::Option(mut o) => {
            o.name = name;
            o.owner = owner;
            AnalysedType::Option(o)
        }
        AnalysedType::Result(mut r) => {
            r.name = name;
            r.owner = owner;
            AnalysedType::Result(r)
        }
        other => other,
    }
}

/// Split a dotted `TypeId` into `(owner, name)`. If the legacy
/// `display_name` is present, use it as the name; otherwise the last dotted
/// segment becomes the name and the prefix becomes the owner.
fn split_type_id(id: &TypeId, display_name: Option<&str>) -> (Option<String>, Option<String>) {
    let raw = &id.0;
    if let Some(name) = display_name {
        // Best-effort: if the TypeId ends with `.<name>`, treat the prefix
        // as owner; otherwise leave owner empty.
        let suffix = format!(".{name}");
        if let Some(owner) = raw.strip_suffix(&suffix) {
            return (Some(owner.to_string()), Some(name.to_string()));
        }
        if raw == name {
            return (None, Some(name.to_string()));
        }
        return (None, Some(name.to_string()));
    }
    // No legacy display name recorded; reconstruct from the TypeId.
    match raw.rsplit_once('.') {
        Some((owner, name)) => (Some(owner.to_string()), Some(name.to_string())),
        None => (None, Some(raw.clone())),
    }
}
