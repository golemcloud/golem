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

use crate::bridge_gen::type_naming::builder::{Builder, RootOwner};
use crate::bridge_gen::type_naming::schema_type_ext::SchemaTypeExt;
use anyhow::bail;
use golem_common::base_model::agent::{AgentType, DataSchema, ElementSchema};
use golem_common::schema::adapters::analysed_type::{
    SchemaGraphBuilder, strip_disambiguation_suffix,
};
use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
use golem_common::schema::schema_type::SchemaType;
use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use type_location::{TypeLocation, TypeLocationPath};

mod builder;
pub(crate) mod schema_type_ext;
mod type_location;

pub trait TypeName: Debug + Display + Clone + PartialEq + Eq + Hash {
    // This is intended to be used for custom or special mappings. If this method returns some
    // result for a type, then no further type naming will be attempted.
    fn from_schema_type(typ: &SchemaType) -> Option<Self>;

    /// Creates a type name from an optional owner and a name.
    /// When `same_language` is true, the metadata names are already in the target language's
    /// native casing, so no heck transformations should be applied.
    fn from_owner_and_name(
        owner: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        same_language: bool,
    ) -> Self;

    /// Creates a type name by joining path segments.
    /// When `same_language` is true, segments are joined as-is without casing transformations.
    fn from_segments(
        segments: impl IntoIterator<Item = impl AsRef<str>>,
        same_language: bool,
    ) -> Self;

    fn requires_type_name(typ: &SchemaType) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TsTypeName {
    pub name: String,
    pub owner: Option<String>,
}

impl Display for TsTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(owner) = &self.owner {
            write!(f, "{}::", owner)?;
        }
        write!(f, "{}", self.name)
    }
}

/// Tracks the set of distinct types discovered while walking an agent's
/// constructor, methods, and config, and assigns each distinct type a
/// language-specific generated name.
///
/// The walker is keyed on [`SchemaType`] structural identity. Named legacy
/// composites are converted through a shared
/// [`SchemaGraphBuilder`](golem_common::schema::adapters::analysed_type::SchemaGraphBuilder)
/// kept on `self`, which materialises a [`SchemaTypeDef`] per named type
/// and references it from the body via [`SchemaType::Ref`]. The same
/// builder lowers every root (constructor, each method's input/output,
/// every config field), so same-name structural collisions across roots
/// are disambiguated globally instead of producing two roots that
/// silently resolve to the same wrong def.
///
/// The merged graph collected from every walked subtree is exposed via
/// [`TypeNaming::graph`] so downstream callers can resolve refs without
/// re-converting from the legacy [`AnalysedType`](golem_wasm::analysis::AnalysedType).
pub struct TypeNaming<TN: TypeName> {
    schema_builder: SchemaGraphBuilder,
    graph: SchemaGraph,
    named_type_locations: IndexMap<TN, Vec<(SchemaType, Vec<TypeLocation>)>>,
    anonymous_type_locations: Vec<(SchemaType, Vec<TypeLocation>)>,
    type_names: HashSet<TN>,
    types: Vec<(SchemaType, TN)>,
    /// Memoises `AnalysedType → SchemaType` lookups built during the
    /// `collect_all_schema_types` walk. Generator emit code reuses this to
    /// resolve a legacy type against the disambiguated def table — a
    /// fresh `analysed_type_to_schema_graph(typ)` would only see the
    /// type in isolation and return the base `TypeId`, silently colliding
    /// with the suffixed id stored in `graph`.
    imported_schema_types: HashMap<AnalysedType, SchemaType>,
    same_language: bool,
}

impl<TN: TypeName> TypeNaming<TN> {
    pub fn new(agent_type: &AgentType, same_language: bool) -> anyhow::Result<Self> {
        Self::new_with_reserved_names(agent_type, same_language, std::iter::empty::<TN>())
    }

    pub fn new_with_reserved_names(
        agent_type: &AgentType,
        same_language: bool,
        reserved_names: impl IntoIterator<Item = TN>,
    ) -> anyhow::Result<Self> {
        let mut type_names = HashSet::new();
        type_names.extend(reserved_names);

        let mut type_naming = Self {
            schema_builder: SchemaGraphBuilder::new(),
            graph: SchemaGraph::empty(),
            named_type_locations: IndexMap::new(),
            anonymous_type_locations: Vec::new(),
            type_names,
            types: Vec::new(),
            imported_schema_types: HashMap::new(),
            same_language,
        };

        type_naming.collect_all_schema_types(agent_type)?;
        // After every root has been lowered into the shared builder, take a
        // single graph snapshot so downstream ref resolution sees the
        // disambiguated def table.
        type_naming.graph = type_naming
            .schema_builder
            .snapshot_graph(SchemaType::bool());
        type_naming.derive_type_names()?;

        Ok(type_naming)
    }

    /// The merged schema graph collected from every traversed subtree.
    /// `SchemaType::Ref` values produced by the converter all resolve
    /// against this graph.
    pub fn graph(&self) -> &SchemaGraph {
        &self.graph
    }

    pub fn type_name_for_type(&self, typ: &SchemaType) -> Option<&TN> {
        self.types.iter().find_map(|(t, n)| (t == typ).then_some(n))
    }

    pub fn types(&self) -> impl Iterator<Item = (&SchemaType, &TN)> {
        self.types.iter().map(|(t, n)| (t, n))
    }

    fn collect_all_schema_types(&mut self, agent_type: &AgentType) -> anyhow::Result<()> {
        let mut builder = Builder::new();

        self.collect_schema_types_in_data_schema(
            &mut builder,
            &agent_type.constructor.input_schema,
        )?;

        for method in &agent_type.methods {
            builder.set_root_owner(RootOwner::MethodInput {
                method_name: method.name.clone(),
            });
            self.collect_schema_types_in_data_schema(&mut builder, &method.input_schema)?;

            builder.set_root_owner(RootOwner::MethodOutput {
                method_name: method.name.clone(),
            });
            self.collect_schema_types_in_data_schema(&mut builder, &method.output_schema)?;
        }

        builder.set_root_owner(RootOwner::AgentConfig);
        for config in &agent_type.config {
            builder.set_root_item_name(&config.path.join("_"));
            let schema_type = self.import_analysed_type(&config.value_type)?;
            self.collect_schema_type(&mut builder, &schema_type);
        }

        Ok(())
    }

    fn collect_schema_types_in_data_schema(
        &mut self,
        builder: &mut Builder,
        schema: &DataSchema,
    ) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(items) => {
                for named_item in &items.elements {
                    builder.set_root_item_name(&named_item.name);
                    self.collect_schema_types_in_element_schema(builder, &named_item.schema)?;
                }
            }
            DataSchema::Multimodal(variants) => {
                for named_variant in &variants.elements {
                    builder.set_root_item_name(&named_variant.name);
                    self.collect_schema_types_in_element_schema(builder, &named_variant.schema)?;
                }
            }
        }
        Ok(())
    }

    fn collect_schema_types_in_element_schema(
        &mut self,
        builder: &mut Builder,
        schema: &ElementSchema,
    ) -> anyhow::Result<()> {
        let ElementSchema::ComponentModel(component_model_type) = schema else {
            return Ok(());
        };

        let schema_type = self.import_analysed_type(&component_model_type.element_type)?;
        self.collect_schema_type(builder, &schema_type);
        Ok(())
    }

    /// Convert a legacy `AnalysedType` into the schema layer by lowering it
    /// into the shared [`SchemaGraphBuilder`] kept on `self`. Disambiguation
    /// runs against every previously lowered root, so two same-name
    /// structurally distinct types appearing in separate constructor /
    /// method / config positions of the same agent get distinct
    /// [`SchemaTypeDef`] entries rather than silently merging into the
    /// first def's body. The lowering result is memoised so emit-time
    /// callers can replay the exact disambiguated [`SchemaType`] via
    /// [`Self::imported_schema_type`] without re-lowering through a
    /// fresh standalone builder.
    fn import_analysed_type(&mut self, typ: &AnalysedType) -> anyhow::Result<SchemaType> {
        if let Some(cached) = self.imported_schema_types.get(typ) {
            return Ok(cached.clone());
        }
        let lowered = self
            .schema_builder
            .lower(typ)
            .map_err(|e| anyhow::anyhow!("Failed to convert legacy type into schema: {e}"))?;
        self.imported_schema_types
            .insert(typ.clone(), lowered.clone());
        Ok(lowered)
    }

    /// Look up the [`SchemaType`] that was produced when this
    /// [`AnalysedType`] was lowered during `collect_all_schema_types`. Use
    /// this from emit code that holds an `&self` and needs to reach the
    /// disambiguated [`SchemaType::Ref`] / inline body without
    /// re-lowering through a fresh standalone builder (which would lose
    /// cross-root disambiguation suffixes).
    pub fn imported_schema_type(&self, typ: &AnalysedType) -> Option<&SchemaType> {
        self.imported_schema_types.get(typ)
    }

    fn collect_schema_type(&mut self, builder: &mut Builder, typ: &SchemaType) {
        // Resolve refs so we walk the underlying body, but remember the
        // ref's display name/owner for path annotations. The body is
        // cloned here so the recursive walk does not hold a borrow on
        // `self.graph` while it mutates `self.types`.
        let (display_name, display_owner, resolved) = self.resolve_for_walk_owned(typ);
        let resolved = &resolved;

        match resolved {
            SchemaType::Variant { cases, .. } => {
                for case in cases {
                    if let Some(payload) = case.payload.as_path_elem_type() {
                        builder.push(TypeLocationPath::VariantCase {
                            name: display_name.clone(),
                            owner: display_owner.clone(),
                            case: case.name.clone(),
                            inner: None,
                        });
                        self.collect_schema_type(builder, payload);
                        builder.pop();
                    } else if let Some(payload) = &case.payload {
                        self.collect_schema_type(builder, payload);
                    }
                }
            }
            SchemaType::Result { spec, .. } => {
                let ok_ref = spec.ok.as_deref();
                let err_ref = spec.err.as_deref();
                if let Some(ok) = ok_ref.and_then(SchemaType::as_path_elem_type) {
                    builder.push(TypeLocationPath::ResultOk {
                        name: display_name.clone(),
                        owner: display_owner.clone(),
                        inner: None,
                    });
                    self.collect_schema_type(builder, ok);
                    builder.pop();
                } else if let Some(ok) = ok_ref {
                    self.collect_schema_type(builder, ok);
                }

                if let Some(err) = err_ref.and_then(SchemaType::as_path_elem_type) {
                    builder.push(TypeLocationPath::ResultErr {
                        name: display_name.clone(),
                        owner: display_owner.clone(),
                        inner: None,
                    });
                    self.collect_schema_type(builder, err);
                    builder.pop();
                } else if let Some(err) = err_ref {
                    self.collect_schema_type(builder, err);
                }
            }
            SchemaType::Option { inner, .. } => {
                if let Some(inner_payload) = inner.as_path_elem_type() {
                    builder.push(TypeLocationPath::Option {
                        name: display_name.clone(),
                        owner: display_owner.clone(),
                        inner: None,
                    });
                    self.collect_schema_type(builder, inner_payload);
                    builder.pop();
                } else {
                    self.collect_schema_type(builder, inner);
                }
            }
            SchemaType::Record { fields, .. } => {
                for field in fields {
                    if let Some(inner) = field.body.as_path_elem_type() {
                        builder.push(TypeLocationPath::RecordField {
                            name: display_name.clone(),
                            owner: display_owner.clone(),
                            field_name: field.name.clone(),
                            inner: None,
                        });
                        self.collect_schema_type(builder, inner);
                        builder.pop();
                    } else {
                        self.collect_schema_type(builder, &field.body);
                    }
                }
            }
            SchemaType::Tuple { elements, .. } => {
                for (idx, item) in elements.iter().enumerate() {
                    if let Some(inner) = item.as_path_elem_type() {
                        builder.push(TypeLocationPath::TupleItem {
                            name: display_name.clone(),
                            owner: display_owner.clone(),
                            idx: idx.to_string(),
                            inner: None,
                        });
                        self.collect_schema_type(builder, inner);
                        builder.pop();
                    } else {
                        self.collect_schema_type(builder, item);
                    }
                }
            }
            SchemaType::List { element, .. } => {
                if let Some(inner) = element.as_path_elem_type() {
                    builder.push(TypeLocationPath::List {
                        name: display_name.clone(),
                        owner: display_owner.clone(),
                        inner: None,
                    });
                    self.collect_schema_type(builder, inner);
                    builder.pop();
                } else {
                    self.collect_schema_type(builder, element);
                }
            }
            // Closed sums without nested children and all primitives are leaves.
            SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Bool { .. }
            | SchemaType::S8 { .. }
            | SchemaType::S16 { .. }
            | SchemaType::S32 { .. }
            | SchemaType::S64 { .. }
            | SchemaType::U8 { .. }
            | SchemaType::U16 { .. }
            | SchemaType::U32 { .. }
            | SchemaType::U64 { .. }
            | SchemaType::F32 { .. }
            | SchemaType::F64 { .. }
            | SchemaType::Char { .. }
            | SchemaType::String { .. } => {
                // NOP
            }
            // Rich variants (Text, Binary, Path, Url, Datetime, Duration,
            // Quantity, Union, Secret, QuotaToken, FixedList, Map, Future,
            // Stream) have no legacy AnalysedType counterpart and never
            // appear when the bridge generator is fed a legacy `AgentType`
            // (the only construction path today). They are silently treated
            // as leaves here; if such a type ever shows up in a custom
            // schema, the emission-time projection via
            // `schema_type_to_analysed_type` will surface a clear error.
            SchemaType::FixedList { .. }
            | SchemaType::Map { .. }
            | SchemaType::Text { .. }
            | SchemaType::Binary { .. }
            | SchemaType::Path { .. }
            | SchemaType::Url { .. }
            | SchemaType::Datetime { .. }
            | SchemaType::Duration { .. }
            | SchemaType::Quantity { .. }
            | SchemaType::Union { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                // NOP
            }
            // `Ref` was already unwrapped by `resolve_for_walk`; recursion
            // proceeds on the body.
            SchemaType::Ref { .. } => unreachable!("Ref is resolved before reaching this match"),
        }

        // Register the original `typ` (not the resolved body) for naming —
        // a Ref keeps its named identity, an inline composite carries its
        // own inline identity.
        if typ.can_be_named() {
            match display_name.as_deref() {
                Some(name) => {
                    let key =
                        TN::from_owner_and_name(display_owner.as_deref(), name, self.same_language);
                    let entries = self.named_type_locations.entry(key).or_default();
                    let location = builder.type_location();
                    if let Some((_, locs)) = entries.iter_mut().find(|(t, _)| t == typ) {
                        locs.push(location);
                    } else {
                        entries.push((typ.clone(), vec![location]));
                    }
                }
                None => {
                    if TN::requires_type_name(typ) {
                        let location = builder.type_location();
                        if let Some((_, locs)) = self
                            .anonymous_type_locations
                            .iter_mut()
                            .find(|(t, _)| t == typ)
                        {
                            locs.push(location);
                        } else {
                            self.anonymous_type_locations
                                .push((typ.clone(), vec![location]));
                        }
                    }
                }
            }
        }
    }

    /// Resolve a [`SchemaType::Ref`] against the in-progress
    /// [`SchemaGraphBuilder`] and return the body to recurse into plus the
    /// displayed name/owner pair.
    ///
    /// For inline types this returns `(None, None, typ.clone())` — the
    /// walker inspects the inline body without any extra context. The
    /// body is owned (cloned for refs) so the caller can mutate `self`
    /// during recursion without holding a borrow on the builder.
    ///
    /// The lookup goes through the live builder rather than the
    /// `self.graph` snapshot because [`Self::collect_all_schema_types`]
    /// interleaves `import_analysed_type` (which mutates the builder)
    /// with `collect_schema_type` (which resolves refs); the snapshot is
    /// only materialised once collection has finished.
    fn resolve_for_walk_owned(
        &self,
        typ: &SchemaType,
    ) -> (Option<String>, Option<String>, SchemaType) {
        match typ {
            SchemaType::Ref { id, .. } => {
                let def: &SchemaTypeDef = self
                    .schema_builder
                    .lookup(id)
                    .expect("Ref points to a def in the shared graph");
                let (owner, name) = split_type_id(&id.0, def.name.as_deref());
                (name, owner, def.body.clone())
            }
            other => (None, None, other.clone()),
        }
    }

    fn derive_type_names(&mut self) -> anyhow::Result<()> {
        let named_groups: Vec<(TN, Vec<(SchemaType, Vec<TypeLocation>)>)> = self
            .named_type_locations
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, type_to_locations) in named_groups {
            let force_generate_unique_name_by_location = type_to_locations.len() > 1;
            for (typ, locations) in type_to_locations {
                self.add_unique_type(
                    Some(name.clone()),
                    typ,
                    &locations,
                    force_generate_unique_name_by_location,
                )?;
            }
        }
        let anonymous = std::mem::take(&mut self.anonymous_type_locations);
        for (typ, locations) in anonymous {
            self.add_unique_type(None, typ, &locations, false)?;
        }
        Ok(())
    }

    fn add_unique_type(
        &mut self,
        name: Option<TN>,
        typ: SchemaType,
        locations: &[TypeLocation],
        force_generate_unique_by_location: bool,
    ) -> anyhow::Result<()> {
        if self.types.iter().any(|(t, _)| t == &typ) {
            return Ok(());
        }

        let name = match name {
            Some(name) => {
                if force_generate_unique_by_location || self.type_names.contains(&name) {
                    self.generate_unique_type_name_based_on_locations(Some(&name), locations)?
                } else {
                    name
                }
            }
            None => self.generate_unique_type_name_based_on_locations(None, locations)?,
        };

        self.type_names.insert(name.clone());
        self.types.push((typ, name));

        Ok(())
    }

    fn generate_unique_type_name_based_on_locations(
        &self,
        name: Option<&TN>,
        locations: &[TypeLocation],
    ) -> anyhow::Result<TN> {
        for location in locations {
            let segments = location.to_type_naming_segments();
            let len = segments.len();
            let mut candidate = match name {
                Some(name) => name.to_string(),
                None => "".to_string(),
            };
            for i in (0..len).rev() {
                let subsegments = &segments[i];
                if subsegments.is_empty() {
                    continue;
                }
                let candidate_type_name = TN::from_segments(
                    subsegments
                        .iter()
                        .copied()
                        .chain(std::iter::once(candidate.as_str())),
                    self.same_language,
                );
                if !self.type_names.contains(&candidate_type_name) {
                    return Ok(candidate_type_name);
                }
                candidate = candidate_type_name.to_string();
            }
        }
        bail!(
            "Failed to generate unique location based type name for {:#?}\n\nlocations: {:#?}",
            name,
            locations,
        )
    }
}

/// Split a dotted `TypeId` into `(owner, name)`. If the legacy display
/// `name` is recorded on the def, use it as the name component and treat
/// any preceding dotted segment as the owner; otherwise reconstruct the
/// pair from the raw id by splitting at the last dot.
///
/// Disambiguation suffixes produced by the schema adapter (see
/// [`SchemaGraphBuilder`](golem_common::schema::adapters::analysed_type::SchemaGraphBuilder))
/// are stripped before splitting so an owner-qualified duplicate generic
/// (e.g. `std.ops.Bound__g_<hex>` with display name `Bound`) still
/// recovers `(Some("std.ops"), Some("Bound"))` instead of losing the
/// owner.
///
/// This mirrors the legacy [`AnalysedType::name`](golem_wasm::analysis::AnalysedType::name)
/// / [`AnalysedType::owner`](golem_wasm::analysis::AnalysedType::owner)
/// reattachment performed inside
/// [`golem_common::schema::adapters::analysed_type::schema_graph_to_analysed_type`].
fn split_type_id(raw: &str, display_name: Option<&str>) -> (Option<String>, Option<String>) {
    let raw = strip_disambiguation_suffix(raw);
    if let Some(name) = display_name {
        let suffix = format!(".{name}");
        if let Some(owner) = raw.strip_suffix(&suffix) {
            return (Some(owner.to_string()), Some(name.to_string()));
        }
        if raw == name {
            return (None, Some(name.to_string()));
        }
        return (None, Some(name.to_string()));
    }
    match raw.rsplit_once('.') {
        Some((owner, name)) => (Some(owner.to_string()), Some(name.to_string())),
        None => (None, Some(raw.to_string())),
    }
}
