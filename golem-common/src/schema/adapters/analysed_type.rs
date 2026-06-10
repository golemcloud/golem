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

/// Marker inserted between a base [`TypeId`] and its structural fingerprint
/// when disambiguating two distinct legacy types that share a single
/// `(owner, name)`. The marker is alphanumeric / underscore only, which
/// keeps the resulting id URI-safe (it can appear in JSON Schema `$defs`
/// keys, JSON Pointer refs, and OpenAPI component names without
/// percent-encoding). Stripping the marker reconstructs the original base
/// id; see [`strip_disambiguation_suffix`] and [`split_type_id`].
///
/// Exposed publicly so downstream consumers that re-derive `(owner, name)`
/// from a generated `TypeId` (e.g. CLI bridge code generation) can strip
/// the suffix using the same constant.
pub const DISAMBIGUATION_MARKER: &str = "__g_";

use crate::schema::adapters::error::{SchemaAdapterError, legacy_type_id};
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};

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
///
/// For converting many legacy roots that share named types — e.g. an agent's
/// constructor plus its method inputs and outputs — use
/// [`SchemaGraphBuilder`] directly so disambiguation runs against the union
/// of all previously lowered types instead of independently per root.
pub fn analysed_type_to_schema_graph(ty: &AnalysedType) -> Result<SchemaGraph, SchemaAdapterError> {
    let mut builder = SchemaGraphBuilder::new();
    let root = builder.lower(ty)?;
    Ok(builder.into_graph_with_root(root))
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
            items
                .iter()
                .map(inline_no_naming)
                .collect::<Result<_, _>>()?,
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

/// Stateful builder for [`SchemaGraph`]s assembled from one or more legacy
/// [`AnalysedType`] roots. Use this directly when you need disambiguation
/// to consider every previously lowered type — for example when bridge
/// generation imports the constructor, every method's input/output, and
/// every config field of an agent into a single shared graph. Each call to
/// [`SchemaGraphBuilder::lower`] adds the new root's named composites as
/// [`SchemaTypeDef`] entries and returns the schema-layer type to use as
/// the new root.
#[derive(Default)]
pub struct SchemaGraphBuilder {
    defs_by_id: HashMap<TypeId, SchemaTypeDef>,
}

impl SchemaGraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lower a single legacy [`AnalysedType`] tree into the shared graph and
    /// return the schema-layer type that should be used as a root. Named
    /// composites encountered along the way are registered into the
    /// builder's def table; subsequent calls disambiguate against the
    /// accumulated state.
    pub fn lower(&mut self, ty: &AnalysedType) -> Result<SchemaType, SchemaAdapterError> {
        self.lower_named(ty)
    }

    /// Snapshot the accumulated defs (sorted by id) without consuming the
    /// builder. Use this when you need a `SchemaGraph` to resolve refs
    /// without losing the ability to lower more roots into the same
    /// builder.
    pub fn snapshot_graph(&self, root: SchemaType) -> SchemaGraph {
        let mut defs: Vec<_> = self.defs_by_id.values().cloned().collect();
        defs.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        SchemaGraph { defs, root }
    }

    /// Resolve a `TypeId` against the accumulated defs without allocating
    /// a `SchemaGraph`. Useful during in-progress lowering when callers
    /// alternate between [`Self::lower`] and ref resolution; downstream
    /// consumers that only need a snapshot at the end should still use
    /// [`Self::snapshot_graph`] / [`Self::into_graph_with_root`].
    pub fn lookup(&self, id: &TypeId) -> Option<&SchemaTypeDef> {
        self.defs_by_id.get(id)
    }

    /// Consume the builder and produce a [`SchemaGraph`] anchored at `root`.
    pub fn into_graph_with_root(self, root: SchemaType) -> SchemaGraph {
        SchemaGraph {
            defs: self.into_defs(),
            root,
        }
    }

    fn lower_named(&mut self, ty: &AnalysedType) -> Result<SchemaType, SchemaAdapterError> {
        // If the legacy type carries `name` (and optional `owner`) we register
        // it as a def and return a Ref. Otherwise lower inline.
        let base_id = match legacy_id_of(ty)? {
            Some(id) => id,
            None => return self.lower_inline(ty),
        };

        // Legacy `AnalysedType` is tree-shaped (no `Ref` / cycles), so we can
        // always materialise the body before deciding on the final TypeId.
        let body = self.lower_inline(ty)?;
        let final_id = self.assign_id(&base_id, &body);

        if !self.defs_by_id.contains_key(&final_id) {
            let display_name = analysed_type_name(ty).map(|s| s.to_string());
            self.defs_by_id.insert(
                final_id.clone(),
                SchemaTypeDef {
                    id: final_id.clone(),
                    name: display_name,
                    body,
                },
            );
        }

        Ok(SchemaType::ref_to(final_id))
    }

    /// Pick a TypeId for `body` given the legacy-derived `base_id`. If
    /// `base_id` is unused, or if the existing entry under `base_id` has a
    /// structurally identical body, return `base_id`. Otherwise disambiguate
    /// by appending a deterministic structural fingerprint suffix using the
    /// [`DISAMBIGUATION_MARKER`] separator.
    ///
    /// Same-name structural collisions occur when an SDK reuses a single
    /// legacy name across multiple instantiations of a generic — for example
    /// the Rust SDK emits `name = "Bound"` for every `std::ops::Bound<T>`
    /// regardless of `T`. Returning an error in that case would block
    /// otherwise valid agent metadata; instead, the second and subsequent
    /// distinct bodies get a `<base_id>__g_<hash>` TypeId so each
    /// instantiation keeps its own `SchemaTypeDef`. Stripping the marker
    /// suffix yields the original base id so owner/name recovery in
    /// [`split_type_id`] is unaffected.
    fn assign_id(&self, base_id: &TypeId, body: &SchemaType) -> TypeId {
        match self.defs_by_id.get(base_id) {
            Some(existing) if &existing.body == body => base_id.clone(),
            Some(_) => {
                let fp = body_fingerprint(body);
                let mut candidate =
                    TypeId::new(format!("{}{DISAMBIGUATION_MARKER}{fp}", base_id.0));
                // Pathological hash-collision guard: walk a counter suffix
                // until either an unused id is found, or one whose body
                // matches.
                let mut counter: u32 = 0;
                while let Some(existing) = self.defs_by_id.get(&candidate) {
                    if &existing.body == body {
                        return candidate;
                    }
                    counter += 1;
                    candidate = TypeId::new(format!(
                        "{}{DISAMBIGUATION_MARKER}{fp}_{counter}",
                        base_id.0
                    ));
                }
                candidate
            }
            None => base_id.clone(),
        }
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
            AnalysedType::List(TypeList { inner, .. }) => Ok(SchemaType::list(self.lower(inner)?)),
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

/// Deterministic structural fingerprint over a [`SchemaType`] body, used
/// to disambiguate same-name distinct legacy types. The output is stable
/// across processes and Rust toolchain versions: serialisation goes
/// through `serde_json` over `SchemaType` (which is `Serialize` and
/// contains no hash-ordered maps), and the digest is the 64-bit FNV-1a
/// hash of those bytes implemented inline.
///
/// Equal bodies always produce equal fingerprints. Distinct bodies almost
/// always produce distinct fingerprints; a counter suffix in
/// [`SchemaGraphBuilder::assign_id`] guards against the astronomically
/// unlikely hash collision case.
fn body_fingerprint(body: &SchemaType) -> String {
    let bytes = serde_json::to_vec(body)
        .expect("SchemaType is `Serialize` and free of map ordering; serialisation cannot fail");
    format!("{:016x}", fnv1a_64(&bytes))
}

/// FNV-1a 64-bit hash. The constants and algorithm are public domain and
/// stable by specification — see <http://www.isthe.com/chongo/tech/comp/fnv/>.
/// Implemented locally to keep the fingerprint independent of std's
/// [`std::collections::hash_map::DefaultHasher`], whose internal algorithm
/// is not part of Rust's stability guarantees.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Return the base portion of `id` with the disambiguation suffix removed,
/// or `id` unchanged if no suffix is present. The suffix format is
/// `__g_<hex>` optionally followed by `_<counter>`; both follow
/// [`assign_id`](SchemaGraphBuilder::assign_id).
///
/// Exposed for downstream code (CLI bridge generation, schema renderers)
/// that re-derives `(owner, name)` from a generated `TypeId` and needs to
/// see through the suffix.
pub fn strip_disambiguation_suffix(id: &str) -> &str {
    match id.rfind(DISAMBIGUATION_MARKER) {
        Some(pos) => &id[..pos],
        None => id,
    }
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
fn reattach_name(ty: AnalysedType, display_name: Option<&str>, id: &TypeId) -> AnalysedType {
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
///
/// Disambiguation suffixes appended by [`SchemaGraphBuilder::assign_id`] are
/// stripped before splitting so a TypeId like `std.ops.Bound__g_<hex>` with
/// display name `Bound` still recovers `(Some("std.ops"), Some("Bound"))`
/// rather than dropping the owner.
fn split_type_id(id: &TypeId, display_name: Option<&str>) -> (Option<String>, Option<String>) {
    let raw = strip_disambiguation_suffix(&id.0);
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
        None => (None, Some(raw.to_string())),
    }
}
