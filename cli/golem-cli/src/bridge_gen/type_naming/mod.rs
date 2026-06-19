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
use golem_common::schema::adapters::analysed_type::strip_disambiguation_suffix;
use golem_common::schema::adapters::data_schema::multimodal_variant_cases;
use golem_common::schema::adapters::unstructured::is_unstructured_variant;
use golem_common::schema::agent::{
    AgentTypeSchema, FieldSource, InputSchema, NamedField, OutputSchema,
};
use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
use golem_common::schema::metadata::TypeId;
use golem_common::schema::schema_type::SchemaType;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use type_location::{TypeLocation, TypeLocationPath};

mod builder;
pub(crate) mod schema_type_ext;
mod type_location;

type TypeLocationsBySchema = Vec<(SchemaType, Vec<TypeLocation>)>;

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
/// The walker is keyed on [`SchemaType`] structural identity. It walks the
/// schema-native roots ([`InputSchema`] / [`OutputSchema`] /
/// [`AgentConfigDeclarationSchema`](golem_common::schema::agent::AgentConfigDeclarationSchema)
/// `value_type`) of an [`AgentTypeSchema`], resolving every
/// [`SchemaType::Ref`] against the agent's own pre-built
/// [`SchemaGraph`](AgentTypeSchema::schema). Named-type disambiguation has
/// already happened at SDK-extraction time, so the walker just adopts that
/// graph instead of rebuilding it; refs that close a cycle on the current
/// DFS path are recorded in [`Self::recursive_ids`] (used by the Rust
/// generator to box recursive by-value positions) and not descended into
/// again.
pub struct TypeNaming<TN: TypeName> {
    graph: SchemaGraph,
    named_type_locations: IndexMap<TN, TypeLocationsBySchema>,
    anonymous_type_locations: Vec<(SchemaType, Vec<TypeLocation>)>,
    type_names: HashSet<TN>,
    types: Vec<(SchemaType, TN)>,
    /// Set of named-def [`TypeId`]s whose body is reachable from itself —
    /// detected as back-edges during the collection DFS. The Rust generator
    /// uses this to decide which `Ref` positions need `Box<T>`.
    recursive_ids: HashSet<TypeId>,
    /// [`TypeId`]s currently on the DFS path; used to detect recursive
    /// back-edges while collecting.
    visiting: HashSet<TypeId>,
    same_language: bool,
}

impl<TN: TypeName> TypeNaming<TN> {
    pub fn new(agent_type: &AgentTypeSchema, same_language: bool) -> anyhow::Result<Self> {
        Self::new_with_reserved_names(agent_type, same_language, std::iter::empty::<TN>())
    }

    pub fn new_with_reserved_names(
        agent_type: &AgentTypeSchema,
        same_language: bool,
        reserved_names: impl IntoIterator<Item = TN>,
    ) -> anyhow::Result<Self> {
        let mut type_names = HashSet::new();
        type_names.extend(reserved_names);

        let mut type_naming = Self {
            graph: agent_type.schema.clone(),
            named_type_locations: IndexMap::new(),
            anonymous_type_locations: Vec::new(),
            type_names,
            types: Vec::new(),
            recursive_ids: HashSet::new(),
            visiting: HashSet::new(),
            same_language,
        };

        type_naming.collect_all_schema_types(agent_type)?;
        type_naming.derive_type_names()?;

        Ok(type_naming)
    }

    /// The agent's schema graph. `SchemaType::Ref` values reached while
    /// walking resolve against this graph.
    pub fn graph(&self) -> &SchemaGraph {
        &self.graph
    }

    pub fn type_name_for_type(&self, typ: &SchemaType) -> Option<&TN> {
        self.types.iter().find_map(|(t, n)| (t == typ).then_some(n))
    }

    pub fn types(&self) -> impl Iterator<Item = (&SchemaType, &TN)> {
        self.types.iter().map(|(t, n)| (t, n))
    }

    /// Whether `typ` is a [`SchemaType::Ref`] to a definition that
    /// participates in a cycle (directly or transitively references itself).
    /// The Rust generator boxes such refs in by-value positions.
    pub fn is_recursive_ref(&self, typ: &SchemaType) -> bool {
        matches!(typ, SchemaType::Ref { id, .. } if self.recursive_ids.contains(id))
    }

    fn collect_all_schema_types(&mut self, agent_type: &AgentTypeSchema) -> anyhow::Result<()> {
        let mut builder = Builder::new();

        builder.set_root_owner(RootOwner::ConstructorInput);
        self.collect_input_schema(&mut builder, &agent_type.constructor.input_schema)?;

        for method in &agent_type.methods {
            builder.set_root_owner(RootOwner::MethodInput {
                method_name: method.name.clone(),
            });
            self.collect_input_schema(&mut builder, &method.input_schema)?;

            builder.set_root_owner(RootOwner::MethodOutput {
                method_name: method.name.clone(),
            });
            self.collect_output_schema(&mut builder, &method.output_schema)?;
        }

        builder.set_root_owner(RootOwner::AgentConfig);
        for config in &agent_type.config {
            builder.set_root_item_name(&config.path.join("_"));
            self.collect_schema_type(&mut builder, &config.value_type)?;
        }

        Ok(())
    }

    /// Walk the user-supplied fields of an [`InputSchema`]. Auto-injected
    /// fields are host-provided and never appear in the generated client
    /// surface, so they are skipped here. A single user-supplied field that
    /// is the structural multimodal form (`list<variant<… Role::Multimodal>>`)
    /// is walked alternative-by-alternative — the multimodal variant/list
    /// wrappers themselves are special-cased by the generators and do not get
    /// their own generated name.
    fn collect_input_schema(
        &mut self,
        builder: &mut Builder,
        input: &InputSchema,
    ) -> anyhow::Result<()> {
        let fields = user_supplied_fields(input);

        if let [field] = fields.as_slice()
            && let Some(cases) = multimodal_variant_cases(&self.graph, &field.schema)?
        {
            let cases = cases.to_vec();
            for case in cases {
                builder.set_root_item_name(&case.name);
                if let Some(payload) = &case.payload {
                    self.collect_schema_type(builder, payload)?;
                }
            }
            return Ok(());
        }

        for field in fields {
            builder.set_root_item_name(&field.name);
            self.collect_schema_type(builder, &field.schema)?;
        }
        Ok(())
    }

    /// Walk an [`OutputSchema`]. A multimodal output is walked
    /// alternative-by-alternative (like multimodal input); any other single
    /// output is walked directly.
    fn collect_output_schema(
        &mut self,
        builder: &mut Builder,
        output: &OutputSchema,
    ) -> anyhow::Result<()> {
        let OutputSchema::Single(ty) = output else {
            return Ok(());
        };

        if let Some(cases) = multimodal_variant_cases(&self.graph, ty)? {
            let cases = cases.to_vec();
            for case in cases {
                builder.set_root_item_name(&case.name);
                if let Some(payload) = &case.payload {
                    self.collect_schema_type(builder, payload)?;
                }
            }
            return Ok(());
        }

        builder.set_root_item_name("");
        self.collect_schema_type(builder, ty)
    }

    fn collect_schema_type(
        &mut self,
        builder: &mut Builder,
        typ: &SchemaType,
    ) -> anyhow::Result<()> {
        // Resolve refs so we walk the underlying body, but remember the
        // ref's display name/owner for path annotations. The body is
        // cloned here so the recursive walk does not hold a borrow on
        // `self.graph` while it mutates `self.types`.
        let ref_id = if let SchemaType::Ref { id, .. } = typ {
            Some(id.clone())
        } else {
            None
        };
        // A ref to a def already on the current DFS path closes a cycle:
        // record it as recursive, register its name, and stop — descending
        // again would loop forever.
        let is_back_edge = ref_id
            .as_ref()
            .is_some_and(|id| self.visiting.contains(id));

        let (display_name, display_owner, resolved) = self.resolve_for_walk_owned(typ)?;
        let resolved = &resolved;

        if let Some(id) = &ref_id {
            if is_back_edge {
                self.recursive_ids.insert(id.clone());
            } else {
                self.visiting.insert(id.clone());
            }
        }

        // A role-marked unstructured-text/binary variant is a leaf: it renders
        // inline as the ergonomic wrapper, so the walker must not descend into
        // its `inline` / `url` cases nor register them as named types.
        if !is_back_edge && !is_unstructured_variant(resolved) {
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
                            self.collect_schema_type(builder, payload)?;
                            builder.pop();
                        } else if let Some(payload) = &case.payload {
                            self.collect_schema_type(builder, payload)?;
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
                        self.collect_schema_type(builder, ok)?;
                        builder.pop();
                    } else if let Some(ok) = ok_ref {
                        self.collect_schema_type(builder, ok)?;
                    }

                    if let Some(err) = err_ref.and_then(SchemaType::as_path_elem_type) {
                        builder.push(TypeLocationPath::ResultErr {
                            name: display_name.clone(),
                            owner: display_owner.clone(),
                            inner: None,
                        });
                        self.collect_schema_type(builder, err)?;
                        builder.pop();
                    } else if let Some(err) = err_ref {
                        self.collect_schema_type(builder, err)?;
                    }
                }
                SchemaType::Option { inner, .. } => {
                    if let Some(inner_payload) = inner.as_path_elem_type() {
                        builder.push(TypeLocationPath::Option {
                            name: display_name.clone(),
                            owner: display_owner.clone(),
                            inner: None,
                        });
                        self.collect_schema_type(builder, inner_payload)?;
                        builder.pop();
                    } else {
                        self.collect_schema_type(builder, inner)?;
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
                            self.collect_schema_type(builder, inner)?;
                            builder.pop();
                        } else {
                            self.collect_schema_type(builder, &field.body)?;
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
                            self.collect_schema_type(builder, inner)?;
                            builder.pop();
                        } else {
                            self.collect_schema_type(builder, item)?;
                        }
                    }
                }
                SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                    if let Some(inner) = element.as_path_elem_type() {
                        builder.push(TypeLocationPath::List {
                            name: display_name.clone(),
                            owner: display_owner.clone(),
                            inner: None,
                        });
                        self.collect_schema_type(builder, inner)?;
                        builder.pop();
                    } else {
                        self.collect_schema_type(builder, element)?;
                    }
                }
                SchemaType::Map { key, value, .. } => {
                    // Maps render inline (`Map<K, V>` / `Vec<(K, V)>`); the
                    // walk only needs to discover named types nested in the
                    // key and value bodies.
                    self.collect_schema_type(builder, key)?;
                    self.collect_schema_type(builder, value)?;
                }
                SchemaType::Union { spec, .. } => {
                    for branch in &spec.branches {
                        if let Some(inner) = branch.body.as_path_elem_type() {
                            builder.push(TypeLocationPath::VariantCase {
                                name: display_name.clone(),
                                owner: display_owner.clone(),
                                case: branch.tag.clone(),
                                inner: None,
                            });
                            self.collect_schema_type(builder, inner)?;
                            builder.pop();
                        } else {
                            self.collect_schema_type(builder, &branch.body)?;
                        }
                    }
                }
                SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
                    if let Some(inner) = inner {
                        self.collect_schema_type(builder, inner)?;
                    }
                }
                // Closed sums without nested children and all primitives are
                // leaves, as are the rich scalar / capability types whose
                // payloads carry no nested user `SchemaType`.
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
                | SchemaType::String { .. }
                | SchemaType::Text { .. }
                | SchemaType::Binary { .. }
                | SchemaType::Path { .. }
                | SchemaType::Url { .. }
                | SchemaType::Datetime { .. }
                | SchemaType::Duration { .. }
                | SchemaType::Quantity { .. }
                | SchemaType::Secret { .. }
                | SchemaType::QuotaToken { .. } => {
                    // NOP
                }
                // `Ref` was already unwrapped by `resolve_for_walk_owned`;
                // recursion proceeds on the body.
                SchemaType::Ref { .. } => {
                    unreachable!("Ref is resolved before reaching this match")
                }
            }
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

        if let Some(id) = &ref_id
            && !is_back_edge
        {
            self.visiting.remove(id);
        }

        Ok(())
    }

    /// Resolve a [`SchemaType::Ref`] against the agent's [`SchemaGraph`] and
    /// return the body to recurse into plus the displayed name/owner pair.
    ///
    /// For inline types this returns `(None, None, typ.clone())` — the
    /// walker inspects the inline body without any extra context. The
    /// body is owned (cloned for refs) so the caller can mutate `self`
    /// during recursion without holding a borrow on the graph.
    fn resolve_for_walk_owned(
        &self,
        typ: &SchemaType,
    ) -> anyhow::Result<(Option<String>, Option<String>, SchemaType)> {
        match typ {
            SchemaType::Ref { id, .. } => {
                let def: &SchemaTypeDef = self.graph.lookup(id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Dangling SchemaType::Ref `{}` while collecting agent bridge types",
                        id.0
                    )
                })?;
                let (owner, name) = split_type_id(&id.0, def.name.as_deref());
                Ok((name, owner, def.body.clone()))
            }
            other => Ok((None, None, other.clone())),
        }
    }

    fn derive_type_names(&mut self) -> anyhow::Result<()> {
        let named_groups: Vec<(TN, TypeLocationsBySchema)> = self
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

/// The user-supplied fields of an [`InputSchema`], in declaration order.
///
/// Auto-injected fields (e.g. the host-provided
/// [`Principal`](golem_common::schema::agent::AutoInjectedKind::Principal))
/// are filled in by the host at invocation time, never by the generated
/// client, so the bridge generators omit them from the generated parameter
/// surface and from the encoded request record — matching the legacy
/// `DataSchema`, which had no notion of auto-injected fields.
pub(crate) fn user_supplied_fields(input: &InputSchema) -> Vec<&NamedField> {
    input
        .fields()
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .collect()
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
