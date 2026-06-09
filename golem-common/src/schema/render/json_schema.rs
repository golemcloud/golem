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

//! Renderer that produces a JSON Schema document from a `SchemaGraph`/
//! `SchemaType`.

use crate::schema::agent::{FieldSource, InputSchema, OutputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NamedFieldType, PathSpec, QuantitySpec, QuantityValue,
    QuotaTokenSpec, ResultSpec, SchemaType, SecretSpec, TextRestrictions, UnionBranch, UnionSpec,
    UrlRestrictions, VariantCaseType,
};
use serde_json::{Map, Number, Value};
use std::collections::{HashMap, HashSet};

const JSON_SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";
const MIME_TYPE_PATTERN: &str = "^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$";

/// JSON shape used for the rich `Text` / `Binary` scalar nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RichScalarShape {
    /// Canonical encoding: `Text` → `{ text, language? }`, `Binary` →
    /// `{ bytes (base64url), mime_type? }`.
    Canonical,
    /// MCP content-block encoding: `Text` → `{ data, languageCode? }`,
    /// `Binary` → `{ data (base64), mimeType }`. Matches the historical MCP
    /// wire shapes that MCP clients and the invoke-side parsers expect.
    McpLegacy,
}

/// JSON shape used for a multimodal `list<variant<… Role::Multimodal>>` node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultimodalShape {
    /// Render like any other `list<variant<…>>`: an array whose items are a
    /// `oneOf` of inline `{ <case name>: <case payload schema> }` objects
    /// (or a bare `const` for a payload-less case).
    Canonical,
    /// MCP "parts" encoding: an array whose items are a `oneOf` of inline
    /// `{ name: <const case>, value: <case payload schema> }` objects.
    McpParts,
}

/// Configuration for the JSON Schema renderer.
///
/// The renderer produces the same structural document for every consumer for
/// the common type set; the knobs below select between the canonical
/// standalone JSON Schema form and the MCP tool-schema form (no `$schema`
/// draft marker, MCP content-block shapes for `Text` / `Binary` and the
/// multimodal `parts` array).
#[derive(Clone, Copy, Debug)]
pub struct JsonSchemaConfig {
    /// Emit the `$schema` JSON Schema draft marker at the document root.
    pub include_draft_marker: bool,
    /// JSON shape for `Text` / `Binary` nodes.
    pub rich_scalar_shape: RichScalarShape,
    /// JSON shape for multimodal `list<variant<… Role::Multimodal>>` nodes.
    pub multimodal_shape: MultimodalShape,
}

impl JsonSchemaConfig {
    /// Canonical standalone JSON Schema document (includes the `$schema`
    /// draft marker, canonical rich-scalar and multimodal shapes).
    pub const CANONICAL: Self = Self {
        include_draft_marker: true,
        rich_scalar_shape: RichScalarShape::Canonical,
        multimodal_shape: MultimodalShape::Canonical,
    };

    /// MCP tool input/output schema document. MCP clients do not expect the
    /// `$schema` draft marker on tool schemas, so it is omitted; `Text` /
    /// `Binary` use the MCP content-block shapes and multimodal lists use the
    /// `parts` array shape.
    pub const MCP: Self = Self {
        include_draft_marker: false,
        rich_scalar_shape: RichScalarShape::McpLegacy,
        multimodal_shape: MultimodalShape::McpParts,
    };
}

/// Render `(graph, ty)` to a canonical JSON Schema document (includes the
/// `$schema` draft marker). See [`to_json_schema_with_config`] for the
/// configurable form.
pub fn to_json_schema(graph: &SchemaGraph, ty: &SchemaType) -> Value {
    to_json_schema_with_config(graph, ty, JsonSchemaConfig::CANONICAL)
}

/// Render `(graph, ty)` to a JSON Schema document. When `ty` is a
/// `Ref(TypeId)` the document is `{ "$defs": {…}, "$ref": "#/$defs/<id>" }`;
/// otherwise the root schema is emitted inline with `$defs` carrying every
/// named definition from the graph plus any union per-branch synthesised
/// schemas under tag-derived keys (see [`BranchNameTable`]).
///
/// `config.include_draft_marker` controls whether the `$schema` draft marker
/// is added at the document root.
pub fn to_json_schema_with_config(
    graph: &SchemaGraph,
    ty: &SchemaType,
    config: JsonSchemaConfig,
) -> Value {
    let table = build_branch_name_table(graph, ty);
    let mut root = render_type(graph, ty, true, &table, config);
    let mut defs = render_defs(graph, &table, config);
    add_union_branch_defs(graph, ty, &mut defs, &table, config);
    if !defs.is_empty() {
        if let Some(obj) = root.as_object_mut() {
            obj.insert("$defs".to_string(), Value::Object(defs));
        } else {
            let mut wrapper = Map::new();
            wrapper.insert("$defs".to_string(), Value::Object(defs));
            wrapper.insert("allOf".to_string(), Value::Array(vec![root.clone()]));
            root = Value::Object(wrapper);
        }
    }
    if config.include_draft_marker
        && let Some(obj) = root.as_object_mut()
    {
        // Insert the JSON Schema draft marker at the top of the produced
        // root schema. OpenAPI removes this; see `super::openapi`.
        let mut with_schema = Map::with_capacity(obj.len() + 1);
        with_schema.insert(
            "$schema".to_string(),
            Value::String(JSON_SCHEMA_DRAFT.to_string()),
        );
        for (k, v) in obj.iter() {
            with_schema.insert(k.clone(), v.clone());
        }
        return Value::Object(with_schema);
    }
    root
}

/// Render an [`InputSchema`] to a JSON Schema object document.
///
/// The result is an `object` schema whose `properties` are the input's
/// **user-supplied** parameters (`FieldSource::AutoInjected` fields are host
/// provided and never surfaced to callers, so they are omitted). `required`
/// lists every user-supplied parameter whose schema is not an `option<…>`.
/// `$defs` (named definitions plus synthesised per-union-branch schemas) is
/// attached at the document root so the document is self-contained.
///
/// This reuses the same node rendering as [`to_json_schema_with_config`] by
/// projecting the parameter list onto a synthetic record root, then replacing
/// the record's (all-required) `required` array with the option-aware one.
pub fn input_schema_to_json_schema(
    graph: &SchemaGraph,
    input: &InputSchema,
    config: JsonSchemaConfig,
) -> Value {
    let InputSchema::Parameters(fields) = input;
    let user_fields: Vec<&crate::schema::agent::NamedField> = fields
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .collect();
    let record_fields: Vec<NamedFieldType> = user_fields
        .iter()
        .map(|f| NamedFieldType {
            name: f.name.clone(),
            body: f.schema.clone(),
            metadata: f.metadata.clone(),
        })
        .collect();
    let record = SchemaType::Record {
        fields: record_fields,
        metadata: MetadataEnvelope::default(),
    };
    let mut doc = to_json_schema_with_config(graph, &record, config);
    if let Some(obj) = doc.as_object_mut() {
        let required: Vec<Value> = user_fields
            .iter()
            .filter(|f| !resolves_to_option(graph, &f.schema))
            .map(|f| Value::String(f.name.clone()))
            .collect();
        obj.insert("required".to_string(), Value::Array(required));
    }
    doc
}

/// Render an [`OutputSchema`] to an optional JSON Schema document.
///
/// `OutputSchema::Unit` renders to `None` (the method has no return value).
/// `OutputSchema::Single(ty)` renders `ty` via [`to_json_schema_with_config`].
///
/// This renderer applies no protocol policy: it does **not** suppress
/// multimodal outputs. Consumers that omit `outputSchema` for multimodal
/// (e.g. the MCP exporter) make that decision themselves.
pub fn output_schema_to_json_schema(
    graph: &SchemaGraph,
    output: &OutputSchema,
    config: JsonSchemaConfig,
) -> Option<Value> {
    match output {
        OutputSchema::Unit => None,
        OutputSchema::Single(ty) => Some(to_json_schema_with_config(graph, ty, config)),
    }
}

/// Whether `ty`, after following any `Ref` chain against `graph`, is an
/// `option<…>`. Used to decide whether an input parameter is required.
fn resolves_to_option(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    let mut current = ty;
    let mut visited: HashSet<TypeId> = HashSet::new();
    loop {
        match current {
            SchemaType::Option { .. } => return true,
            SchemaType::Ref { id, .. } => {
                if !visited.insert(id.clone()) {
                    return false;
                }
                match graph.lookup(id) {
                    Some(def) => current = &def.body,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
}

/// Build a `$defs` object covering every named definition in the graph.
///
/// Per RFC 6901 §4, JSON Pointer escaping (`~0`/`~1`) applies to the
/// *pointer string*, not to the resolved object member name. The map key
/// is therefore the **raw** `TypeId.0` string; the escaped form is only
/// used inside `$ref` pointers (see [`ref_pointer`]).
pub(super) fn render_defs(
    graph: &SchemaGraph,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) -> Map<String, Value> {
    let mut defs = Map::new();
    for def in &graph.defs {
        // The def's metadata now lives on `def.body` directly; `render_type`
        // already attaches inline-node metadata, so no extra `attach_metadata`
        // call is required here.
        let mut body = render_type(graph, &def.body, false, table, config);
        if let Some(name) = &def.name
            && let Some(obj) = body.as_object_mut()
        {
            obj.entry("title").or_insert(Value::String(name.clone()));
        }
        defs.insert(def.id.0.clone(), body);
    }
    defs
}

/// Walk every union under the graph and synthesize per-branch `$defs`
/// entries so discriminator-mapping pointers always resolve.
pub(super) fn add_union_branch_defs(
    graph: &SchemaGraph,
    root_ty: &SchemaType,
    defs: &mut Map<String, Value>,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) {
    let mut emitted = HashSet::new();
    collect_union_branch_defs(graph, root_ty, defs, &mut emitted, table, config);
    for def in &graph.defs {
        collect_union_branch_defs(graph, &def.body, defs, &mut emitted, table, config);
    }
}

fn collect_union_branch_defs(
    graph: &SchemaGraph,
    ty: &SchemaType,
    defs: &mut Map<String, Value>,
    emitted: &mut HashSet<String>,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) {
    match ty {
        SchemaType::Union { spec, .. } => {
            for branch in spec.branches.iter() {
                let key = table.name_for(branch).to_string();
                if emitted.insert(key.clone()) {
                    let mut body = render_type(graph, &branch.body, false, table, config);
                    attach_metadata(&mut body, &branch.metadata);
                    if let Some(obj) = body.as_object_mut() {
                        // Constrain the branch schema further with the
                        // discriminator. For record-shaped rules this adds
                        // an extra constraint on the discriminator field;
                        // for string rules it adds a `pattern`/`const`.
                        apply_discriminator_constraint(obj, &branch.discriminator);
                    }
                    defs.insert(key, body);
                }
                collect_union_branch_defs(graph, &branch.body, defs, emitted, table, config);
            }
        }
        SchemaType::Record { fields, .. } => {
            for f in fields {
                collect_union_branch_defs(graph, &f.body, defs, emitted, table, config);
            }
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let Some(p) = &case.payload {
                    collect_union_branch_defs(graph, p, defs, emitted, table, config);
                }
            }
        }
        SchemaType::Tuple { elements, .. } => {
            for e in elements {
                collect_union_branch_defs(graph, e, defs, emitted, table, config);
            }
        }
        SchemaType::List { element, .. }
        | SchemaType::FixedList { element, .. }
        | SchemaType::Option { inner: element, .. } => {
            collect_union_branch_defs(graph, element, defs, emitted, table, config);
        }
        SchemaType::Map { key, value, .. } => {
            collect_union_branch_defs(graph, key, defs, emitted, table, config);
            collect_union_branch_defs(graph, value, defs, emitted, table, config);
        }
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &spec.ok {
                collect_union_branch_defs(graph, t, defs, emitted, table, config);
            }
            if let Some(t) = &spec.err {
                collect_union_branch_defs(graph, t, defs, emitted, table, config);
            }
        }
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                collect_union_branch_defs(graph, t, defs, emitted, table, config);
            }
        }
        _ => {}
    }
}

/// Stable, tag-preserving `$defs` / `components.schemas` keys for every
/// union branch reachable from a render root.
///
/// Built once per render via [`build_branch_name_table`]. The names are
/// derived primarily from each branch's `tag` (sanitised to
/// `UpperCamelCase`) so that types in a generated OpenAPI client carry
/// human-meaningful names rather than opaque content hashes.
///
/// Collisions (two structurally distinct branches sharing a tag) are
/// resolved by progressively prepending segments from the schema-graph
/// path that reached each branch, mirroring the algorithm used by
/// `bridge_gen::type_naming`. As a last resort — when no contextual
/// disambiguation works — a short hash suffix is appended.
///
/// Determinism: the walk visits branches in source order; collision
/// resolution iterates the resulting group deterministically. The
/// canonical structural key used internally is a `blake3` hash of the
/// branch's deterministic JSON serialisation.
pub(super) struct BranchNameTable {
    names: HashMap<String, String>,
}

impl BranchNameTable {
    pub(super) fn name_for(&self, branch: &UnionBranch) -> &str {
        let key = canonical_branch_key(branch);
        self.names.get(&key).map(String::as_str).expect(
            "BranchNameTable must contain every union branch reachable from the render root \
                 — `build_branch_name_table` is the source of truth for this invariant",
        )
    }
}

pub(super) fn build_branch_name_table(
    graph: &SchemaGraph,
    root_ty: &SchemaType,
) -> BranchNameTable {
    let mut collector = BranchCollector::default();
    collector.walk_type(root_ty);
    for def in &graph.defs {
        // Each named def starts a fresh path rooted at its name (or
        // TypeId fallback); this lets disambiguation lift names through
        // the named-def boundary when needed.
        collector.path.clear();
        let seg = def.name.clone().unwrap_or_else(|| def.id.0.clone());
        collector.path.push(seg);
        collector.walk_type(&def.body);
    }
    collector.path.clear();
    // Pre-seed `taken` with every named def's TypeId so branch names
    // never silently overwrite a real graph def in `$defs`.
    let taken: HashSet<String> = graph.defs.iter().map(|d| d.id.0.clone()).collect();
    collector.into_table(taken)
}

/// Deterministic structural key for a `UnionBranch`. Internal only;
/// never appears in rendered output.
fn canonical_branch_key(branch: &UnionBranch) -> String {
    let bytes = serde_json::to_vec(branch).expect("UnionBranch serializes deterministically");
    let hex = blake3::hash(&bytes).to_hex();
    // 128 bits of hash output — sufficient to uniquely identify a branch
    // body within any practical schema document.
    hex.as_str()[..32].to_string()
}

#[derive(Default)]
struct BranchCollector {
    /// Canonical key for every encountered branch, in walk order, with no
    /// duplicates. Used both for membership and for deterministic iteration
    /// during name resolution.
    keys: Vec<String>,
    occurrences: HashMap<String, Occurrence>,
    /// Path of segments describing the current position in the schema
    /// graph (record-field names, variant-case names, "key"/"value",
    /// "ok"/"err", outer branch tags, …).
    path: Vec<String>,
}

struct Occurrence {
    tag: String,
    /// First path at which this branch was reached. Used to disambiguate
    /// colliding preferred names.
    path: Vec<String>,
}

impl BranchCollector {
    fn record(&mut self, branch: &UnionBranch) {
        let key = canonical_branch_key(branch);
        if let std::collections::hash_map::Entry::Vacant(slot) = self.occurrences.entry(key.clone())
        {
            slot.insert(Occurrence {
                tag: branch.tag.clone(),
                path: self.path.clone(),
            });
            self.keys.push(key);
        }
    }

    /// Walk a `SchemaType` subtree, pushing/popping path segments and
    /// recording every encountered `UnionBranch`.
    ///
    /// `Ref(TypeId)` nodes are not followed: the named def they point
    /// at is walked separately from `build_branch_name_table` with its
    /// own fresh path, so following refs here would record duplicate
    /// occurrences and pollute the disambiguation path.
    fn walk_type(&mut self, ty: &SchemaType) {
        match ty {
            SchemaType::Union { spec, .. } => {
                for branch in &spec.branches {
                    self.record(branch);
                    self.path.push(branch.tag.clone());
                    self.walk_type(&branch.body);
                    self.path.pop();
                }
            }
            SchemaType::Record { fields, .. } => {
                for f in fields {
                    self.path.push(f.name.clone());
                    self.walk_type(&f.body);
                    self.path.pop();
                }
            }
            SchemaType::Variant { cases, .. } => {
                for case in cases {
                    if let Some(p) = &case.payload {
                        self.path.push(case.name.clone());
                        self.walk_type(p);
                        self.path.pop();
                    }
                }
            }
            SchemaType::Tuple { elements, .. } => {
                for (i, e) in elements.iter().enumerate() {
                    self.path.push(format!("item{i}"));
                    self.walk_type(e);
                    self.path.pop();
                }
            }
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                self.path.push("item".to_string());
                self.walk_type(element);
                self.path.pop();
            }
            SchemaType::Option { inner, .. } => {
                self.path.push("inner".to_string());
                self.walk_type(inner);
                self.path.pop();
            }
            SchemaType::Map { key, value, .. } => {
                self.path.push("key".to_string());
                self.walk_type(key);
                self.path.pop();
                self.path.push("value".to_string());
                self.walk_type(value);
                self.path.pop();
            }
            SchemaType::Result { spec, .. } => {
                if let Some(t) = &spec.ok {
                    self.path.push("ok".to_string());
                    self.walk_type(t);
                    self.path.pop();
                }
                if let Some(t) = &spec.err {
                    self.path.push("err".to_string());
                    self.walk_type(t);
                    self.path.pop();
                }
            }
            SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
                if let Some(t) = inner {
                    self.path.push("inner".to_string());
                    self.walk_type(t);
                    self.path.pop();
                }
            }
            _ => {}
        }
    }

    fn into_table(self, mut taken: HashSet<String>) -> BranchNameTable {
        // Compute each occurrence's preferred name (from its `tag`) and
        // group by it.
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for key in &self.keys {
            let occ = &self.occurrences[key];
            let preferred = sanitise_to_upper_camel(&occ.tag);
            match groups.iter_mut().find(|(name, _)| name == &preferred) {
                Some((_, members)) => members.push(key.clone()),
                None => groups.push((preferred, vec![key.clone()])),
            }
        }

        let mut names = HashMap::<String, String>::new();
        for (preferred, members) in groups {
            if members.len() == 1 && !taken.contains(&preferred) {
                // Unique preferred name and not colliding with a graph
                // def TypeId — use it verbatim.
                let only = members.into_iter().next().unwrap();
                taken.insert(preferred.clone());
                names.insert(only, preferred);
            } else {
                // Real collision (or shadows a graph def): every member
                // is forced through location-based disambiguation so the
                // assigned names are symmetric.
                for key in members {
                    let occ = &self.occurrences[&key];
                    let assigned = disambiguate(&preferred, &occ.path, &taken, &key);
                    taken.insert(assigned.clone());
                    names.insert(key, assigned);
                }
            }
        }

        BranchNameTable { names }
    }
}

/// Find a unique name by progressively prepending sanitised path
/// segments (innermost → outermost) to `base`. Falls back to a short
/// canonical-key suffix if no contextual disambiguation works.
fn disambiguate(base: &str, path: &[String], taken: &HashSet<String>, canonical: &str) -> String {
    let mut candidate = base.to_string();
    for seg in path.iter().rev() {
        let seg_camel = sanitise_to_upper_camel(seg);
        if seg_camel.is_empty() {
            continue;
        }
        candidate = format!("{seg_camel}{candidate}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    let suffix_len = 6.min(canonical.len());
    format!("{base}_{}", &canonical[..suffix_len])
}

/// Sanitise an arbitrary string to a non-empty UpperCamelCase identifier
/// suitable for both JSON Pointer member names and OpenAPI schema names
/// (alphabet `[A-Za-z0-9]`). Non-alphanumerics are dropped and treated
/// as word separators; a leading digit is prefixed with `_`.
fn sanitise_to_upper_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper_next = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            if upper_next {
                for u in ch.to_uppercase() {
                    out.push(u);
                }
                upper_next = false;
            } else {
                out.push(ch);
            }
        } else {
            upper_next = true;
        }
    }
    if out.is_empty() {
        return "Branch".to_string();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

fn apply_discriminator_constraint(obj: &mut Map<String, Value>, rule: &DiscriminatorRule) {
    match rule {
        DiscriminatorRule::Prefix { prefix } => {
            obj.entry("pattern")
                .or_insert(Value::String(format!("^{}", regex_escape(prefix))));
        }
        DiscriminatorRule::Suffix { suffix } => {
            obj.entry("pattern")
                .or_insert(Value::String(format!("{}$", regex_escape(suffix))));
        }
        DiscriminatorRule::Contains { substring } => {
            obj.entry("pattern")
                .or_insert(Value::String(regex_escape(substring)));
        }
        DiscriminatorRule::Regex { regex } => {
            obj.entry("pattern").or_insert(Value::String(regex.clone()));
        }
        DiscriminatorRule::FieldEquals(disc) => {
            // Constrain the field's value with `const` if a literal is set;
            // otherwise just require the field to be present.
            let mut required = obj
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !required
                .iter()
                .any(|v| v.as_str() == Some(disc.field_name.as_str()))
            {
                required.push(Value::String(disc.field_name.clone()));
            }
            obj.insert("required".to_string(), Value::Array(required));
            if let Some(lit) = &disc.literal {
                let props = obj
                    .entry("properties")
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("properties is object");
                let field = props
                    .entry(disc.field_name.clone())
                    .or_insert_with(|| obj_inline([("type", Value::String("string".to_string()))]));
                if let Some(field_obj) = field.as_object_mut() {
                    field_obj
                        .entry("const")
                        .or_insert(Value::String(lit.clone()));
                }
            }
        }
        DiscriminatorRule::FieldAbsent { field_name } => {
            // Express absence via `not: { required: [field] }`.
            let not = obj_inline([(
                "required",
                Value::Array(vec![Value::String(field_name.clone())]),
            )]);
            obj.insert("not".to_string(), not);
        }
    }
}

pub(super) fn render_type(
    graph: &SchemaGraph,
    ty: &SchemaType,
    root: bool,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) -> Value {
    let mut rendered = match ty {
        SchemaType::Ref { id, .. } => obj([("$ref", Value::String(ref_pointer(id, root)))]),

        SchemaType::Bool { .. } => obj([("type", Value::String("boolean".to_string()))]),
        SchemaType::S8 { .. } => integer_schema(i8::MIN as i64, i8::MAX as i64),
        SchemaType::S16 { .. } => integer_schema(i16::MIN as i64, i16::MAX as i64),
        SchemaType::S32 { .. } => integer_schema(i32::MIN as i64, i32::MAX as i64),
        SchemaType::S64 { .. } => integer_schema(i64::MIN, i64::MAX),
        SchemaType::U8 { .. } => integer_schema(0, u8::MAX as i64),
        SchemaType::U16 { .. } => integer_schema(0, u16::MAX as i64),
        SchemaType::U32 { .. } => integer_schema(0, u32::MAX as i64),
        SchemaType::U64 { .. } => unsigned_64_schema(),
        SchemaType::F32 { .. } | SchemaType::F64 { .. } => {
            obj([("type", Value::String("number".to_string()))])
        }
        SchemaType::Char { .. } => obj([
            ("type", Value::String("string".to_string())),
            ("minLength", Value::Number(1.into())),
            ("maxLength", Value::Number(1.into())),
        ]),
        SchemaType::String { .. } => obj([("type", Value::String("string".to_string()))]),

        SchemaType::Record { fields, .. } => {
            let mut props = Map::new();
            let mut required = Vec::with_capacity(fields.len());
            for field in fields {
                let mut field_schema = render_type(graph, &field.body, false, table, config);
                attach_metadata(&mut field_schema, &field.metadata);
                props.insert(field.name.clone(), field_schema);
                required.push(Value::String(field.name.clone()));
            }
            obj([
                ("type", Value::String("object".to_string())),
                ("properties", Value::Object(props)),
                ("required", Value::Array(required)),
                ("additionalProperties", Value::Bool(false)),
            ])
        }

        SchemaType::Variant { cases, metadata } => {
            let multimodal = matches!(
                metadata.role,
                Some(crate::schema::metadata::Role::Multimodal)
            );
            Value::Object(variant_schema(graph, cases, multimodal, table, config))
        }

        SchemaType::Enum { cases, .. } => obj([
            ("type", Value::String("string".to_string())),
            (
                "enum",
                Value::Array(cases.iter().cloned().map(Value::String).collect()),
            ),
        ]),

        SchemaType::Flags { flags, .. } => obj([
            ("type", Value::String("array".to_string())),
            (
                "items",
                obj([
                    ("type", Value::String("string".to_string())),
                    (
                        "enum",
                        Value::Array(flags.iter().cloned().map(Value::String).collect()),
                    ),
                ]),
            ),
            ("uniqueItems", Value::Bool(true)),
        ]),

        SchemaType::Tuple { elements, .. } => {
            if elements.is_empty() {
                // JSON Schema 2020-12 requires `prefixItems` to be a
                // non-empty array, so the empty-tuple shape uses
                // `maxItems`/`minItems` only.
                obj([
                    ("type", Value::String("array".to_string())),
                    ("minItems", Value::Number(0u64.into())),
                    ("maxItems", Value::Number(0u64.into())),
                ])
            } else {
                obj([
                    ("type", Value::String("array".to_string())),
                    (
                        "prefixItems",
                        Value::Array(
                            elements
                                .iter()
                                .map(|e| render_type(graph, e, false, table, config))
                                .collect(),
                        ),
                    ),
                    ("items", Value::Bool(false)),
                    ("minItems", Value::Number((elements.len() as u64).into())),
                ])
            }
        }

        SchemaType::List { element, .. } => obj([
            ("type", Value::String("array".to_string())),
            ("items", render_type(graph, element, false, table, config)),
        ]),

        SchemaType::FixedList {
            element, length, ..
        } => obj([
            ("type", Value::String("array".to_string())),
            ("items", render_type(graph, element, false, table, config)),
            ("minItems", Value::Number((*length).into())),
            ("maxItems", Value::Number((*length).into())),
        ]),

        SchemaType::Map { key, value, .. } => {
            let pair = obj([
                ("type", Value::String("array".to_string())),
                (
                    "prefixItems",
                    Value::Array(vec![
                        render_type(graph, key, false, table, config),
                        render_type(graph, value, false, table, config),
                    ]),
                ),
                ("items", Value::Bool(false)),
                ("minItems", Value::Number(2.into())),
                ("maxItems", Value::Number(2.into())),
            ]);
            obj([
                ("type", Value::String("array".to_string())),
                ("items", pair),
            ])
        }

        SchemaType::Option { inner, .. } => obj([(
            "oneOf",
            Value::Array(vec![
                obj([("type", Value::String("null".to_string()))]),
                render_type(graph, inner, false, table, config),
            ]),
        )]),

        SchemaType::Result { spec, .. } => Value::Object(result_schema(graph, spec, table, config)),

        SchemaType::Text { restrictions, .. } => Value::Object(text_schema(restrictions, config)),
        SchemaType::Binary { restrictions, .. } => {
            Value::Object(binary_schema(restrictions, config))
        }
        SchemaType::Path { spec, .. } => Value::Object(path_schema(spec)),
        SchemaType::Url { restrictions, .. } => Value::Object(url_schema(restrictions)),
        SchemaType::Datetime { .. } => obj([
            ("type", Value::String("string".to_string())),
            ("format", Value::String("date-time".to_string())),
        ]),
        SchemaType::Duration { .. } => obj([
            ("type", Value::String("string".to_string())),
            ("format", Value::String("duration".to_string())),
        ]),
        SchemaType::Quantity { spec, .. } => Value::Object(quantity_schema(spec)),

        SchemaType::Union { spec, .. } => Value::Object(union_schema(graph, spec, table, config)),

        SchemaType::Secret { spec, .. } => Value::Object(secret_schema(spec)),
        SchemaType::QuotaToken { spec, .. } => Value::Object(quota_token_schema(spec)),

        SchemaType::Future { .. } | SchemaType::Stream { .. } => obj([
            ("type", Value::String("null".to_string())),
            (
                "description",
                Value::String("WASI P3 placeholder".to_string()),
            ),
        ]),
    };

    // Per-node metadata: attach docs / examples / deprecated for every
    // SchemaType node so inline-typed positions (record fields, list
    // elements, etc.) propagate their metadata into the generated JSON
    // Schema, not only named definitions.
    attach_metadata(&mut rendered, ty.metadata());
    rendered
}

fn ref_pointer(id: &TypeId, _root: bool) -> String {
    ref_to_def_key(&id.0)
}

/// Build the `$ref` pointer string for a raw `$defs` member key.
///
/// `key` is the raw (un-escaped) member name; this helper applies
/// RFC 6901 JSON Pointer escaping when embedding it in the pointer path.
pub(super) fn ref_to_def_key(key: &str) -> String {
    format!("#/$defs/{}", escape_pointer_token(key))
}

fn integer_schema(min: i64, max: i64) -> Value {
    obj([
        ("type", Value::String("integer".to_string())),
        ("minimum", Value::Number(Number::from(min))),
        ("maximum", Value::Number(Number::from(max))),
    ])
}

fn unsigned_64_schema() -> Value {
    obj([
        ("type", Value::String("integer".to_string())),
        ("minimum", Value::Number(Number::from(0u64))),
        ("maximum", Value::Number(Number::from(u64::MAX))),
    ])
}

fn variant_schema(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    multimodal: bool,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) -> Map<String, Value> {
    // MCP "parts" mode renders a multimodal variant inline as a `oneOf` of
    // `{ name: <const case>, value: <case payload schema> }` objects,
    // matching the historical worker-service MCP renderer. The carried tag
    // (the case name) is advertised as the `name` const.
    if multimodal && config.multimodal_shape == MultimodalShape::McpParts {
        let one_of: Vec<Value> = cases
            .iter()
            .map(|case| {
                let value_schema = match &case.payload {
                    Some(payload_ty) => render_type(graph, payload_ty, false, table, config),
                    None => Map::new().into(),
                };
                obj([
                    ("type", Value::String("object".to_string())),
                    (
                        "properties",
                        Value::Object({
                            let mut props = Map::new();
                            props.insert(
                                "name".to_string(),
                                obj([
                                    ("type", Value::String("string".to_string())),
                                    ("const", Value::String(case.name.clone())),
                                ]),
                            );
                            props.insert("value".to_string(), value_schema);
                            props
                        }),
                    ),
                    (
                        "required",
                        Value::Array(vec![
                            Value::String("name".to_string()),
                            Value::String("value".to_string()),
                        ]),
                    ),
                    ("additionalProperties", Value::Bool(false)),
                ])
            })
            .collect();
        let mut out = Map::new();
        out.insert("oneOf".to_string(), Value::Array(one_of));
        return out;
    }
    let one_of: Vec<Value> = cases
        .iter()
        .map(|case| match &case.payload {
            None => obj([("const", Value::String(case.name.clone()))]),
            Some(payload_ty) => {
                let mut props = Map::new();
                props.insert(
                    case.name.clone(),
                    render_type(graph, payload_ty, false, table, config),
                );
                obj([
                    ("type", Value::String("object".to_string())),
                    ("properties", Value::Object(props)),
                    (
                        "required",
                        Value::Array(vec![Value::String(case.name.clone())]),
                    ),
                    ("additionalProperties", Value::Bool(false)),
                ])
            }
        })
        .collect();
    let mut out = Map::new();
    out.insert("oneOf".to_string(), Value::Array(one_of));
    out
}

fn result_schema(
    graph: &SchemaGraph,
    spec: &ResultSpec,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) -> Map<String, Value> {
    let ok_inner = spec
        .ok
        .as_deref()
        .map(|t| render_type(graph, t, false, table, config))
        .unwrap_or_else(|| obj([("type", Value::String("null".to_string()))]));
    let err_inner = spec
        .err
        .as_deref()
        .map(|t| render_type(graph, t, false, table, config))
        .unwrap_or_else(|| obj([("type", Value::String("null".to_string()))]));
    let one_of = vec![
        obj([
            ("type", Value::String("object".to_string())),
            (
                "properties",
                Value::Object({
                    let mut m = Map::new();
                    m.insert("ok".to_string(), ok_inner);
                    m
                }),
            ),
            ("required", Value::Array(vec![Value::String("ok".into())])),
            ("additionalProperties", Value::Bool(false)),
        ]),
        obj([
            ("type", Value::String("object".to_string())),
            (
                "properties",
                Value::Object({
                    let mut m = Map::new();
                    m.insert("err".to_string(), err_inner);
                    m
                }),
            ),
            ("required", Value::Array(vec![Value::String("err".into())])),
            ("additionalProperties", Value::Bool(false)),
        ]),
    ];
    let mut out = Map::new();
    out.insert("oneOf".to_string(), Value::Array(one_of));
    out
}

fn text_schema(restrictions: &TextRestrictions, config: JsonSchemaConfig) -> Map<String, Value> {
    if config.rich_scalar_shape == RichScalarShape::McpLegacy {
        return mcp_text_schema(restrictions);
    }
    // Canonical Text JSON shape: `{ text: string, language?: string }` with
    // length / pattern constraints lifted into the `text` field.
    let mut text_field = Map::new();
    text_field.insert("type".to_string(), Value::String("string".to_string()));
    if let Some(min) = restrictions.min_length {
        text_field.insert("minLength".to_string(), Value::Number(min.into()));
    }
    if let Some(max) = restrictions.max_length {
        text_field.insert("maxLength".to_string(), Value::Number(max.into()));
    }
    if let Some(regex) = &restrictions.regex {
        text_field.insert("pattern".to_string(), Value::String(regex.clone()));
    }
    let mut properties = Map::new();
    properties.insert("text".to_string(), Value::Object(text_field));
    properties.insert(
        "language".to_string(),
        obj([("type", Value::String("string".to_string()))]),
    );
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("text".to_string())]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    if let Some(langs) = &restrictions.languages {
        m.insert(
            "description".to_string(),
            Value::String(format!("Allowed languages: {}", langs.join(", "))),
        );
    }
    m
}

/// MCP content-block shape for `Text`: `{ data, languageCode? }`. Mirrors the
/// historical worker-service MCP renderer so MCP clients and the invoke-side
/// parser agree on the wire shape.
fn mcp_text_schema(restrictions: &TextRestrictions) -> Map<String, Value> {
    let language_code_description = match &restrictions.languages {
        Some(codes) if !codes.is_empty() => {
            format!("Language code. Must be one of: {}", codes.join(", "))
        }
        _ => "Language code".to_string(),
    };
    let mut properties = Map::new();
    properties.insert(
        "data".to_string(),
        obj([
            ("type", Value::String("string".to_string())),
            ("description", Value::String("Text content".to_string())),
        ]),
    );
    properties.insert(
        "languageCode".to_string(),
        obj([
            ("type", Value::String("string".to_string())),
            ("description", Value::String(language_code_description)),
        ]),
    );
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("data".to_string())]),
    );
    m
}

/// MCP content-block shape for `Binary`: `{ data (base64), mimeType }`. The
/// `data` field is standard-base64 encoded to match the invoke-side parser
/// (`base64::engine::general_purpose::STANDARD`).
fn mcp_binary_schema(restrictions: &BinaryRestrictions) -> Map<String, Value> {
    let mime_type_description = match &restrictions.mime_types {
        Some(mimes) if !mimes.is_empty() => {
            format!("MIME type. Must be one of: {}", mimes.join(", "))
        }
        _ => "MIME type".to_string(),
    };
    let mut properties = Map::new();
    properties.insert(
        "data".to_string(),
        obj([
            ("type", Value::String("string".to_string())),
            (
                "description",
                Value::String("Base64-encoded binary data".to_string()),
            ),
        ]),
    );
    properties.insert(
        "mimeType".to_string(),
        obj([
            ("type", Value::String("string".to_string())),
            ("description", Value::String(mime_type_description)),
        ]),
    );
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![
            Value::String("data".to_string()),
            Value::String("mimeType".to_string()),
        ]),
    );
    m
}

fn binary_schema(
    restrictions: &BinaryRestrictions,
    config: JsonSchemaConfig,
) -> Map<String, Value> {
    if config.rich_scalar_shape == RichScalarShape::McpLegacy {
        return mcp_binary_schema(restrictions);
    }
    // Canonical Binary JSON shape: `{ bytes: base64url-string, mime_type?: string }`.
    // `min_bytes` / `max_bytes` count *raw* bytes; the JSON field is
    // base64url-no-pad-encoded, so the on-wire string length is
    // `base64url_no_pad_len(n) = 4*(n/3) + match n%3 { 0=>0, 1=>2, 2=>3 }`.
    let mut bytes_field = Map::new();
    bytes_field.insert("type".to_string(), Value::String("string".to_string()));
    bytes_field.insert(
        "contentEncoding".to_string(),
        Value::String("base64url".to_string()),
    );
    if let Some(min) = restrictions.min_bytes {
        bytes_field.insert(
            "minLength".to_string(),
            Value::Number(base64url_no_pad_len(min).into()),
        );
    }
    if let Some(max) = restrictions.max_bytes {
        bytes_field.insert(
            "maxLength".to_string(),
            Value::Number(base64url_no_pad_len(max).into()),
        );
    }
    let mut mime_field = Map::new();
    mime_field.insert("type".to_string(), Value::String("string".to_string()));
    mime_field.insert(
        "pattern".to_string(),
        Value::String(MIME_TYPE_PATTERN.to_string()),
    );
    let mut properties = Map::new();
    properties.insert("bytes".to_string(), Value::Object(bytes_field));
    properties.insert("mime_type".to_string(), Value::Object(mime_field));
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("bytes".to_string())]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    if let Some(mimes) = &restrictions.mime_types {
        m.insert(
            "description".to_string(),
            Value::String(format!("Allowed MIME types: {}", mimes.join(", "))),
        );
    }
    m
}

fn path_schema(spec: &PathSpec) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("string".to_string()));
    let kind = match spec.kind {
        crate::schema::schema_type::PathKind::File => "file",
        crate::schema::schema_type::PathKind::Directory => "directory",
        crate::schema::schema_type::PathKind::Any => "any",
    };
    let direction = match spec.direction {
        crate::schema::schema_type::PathDirection::Input => "input",
        crate::schema::schema_type::PathDirection::Output => "output",
        crate::schema::schema_type::PathDirection::InOut => "inout",
    };
    m.insert(
        "title".to_string(),
        Value::String(format!("{direction} {kind} path")),
    );
    let mut description = Vec::new();
    if let Some(exts) = &spec.allowed_extensions {
        description.push(format!("Allowed extensions: {}", exts.join(", ")));
    }
    if let Some(mimes) = &spec.allowed_mime_types {
        description.push(format!("Allowed MIME types: {}", mimes.join(", ")));
    }
    if !description.is_empty() {
        m.insert(
            "description".to_string(),
            Value::String(description.join("; ")),
        );
    }
    m
}

fn url_schema(restrictions: &UrlRestrictions) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("string".to_string()));
    m.insert("title".to_string(), Value::String("URL".to_string()));
    let mut description = Vec::new();
    if let Some(schemes) = &restrictions.allowed_schemes {
        description.push(format!("Allowed schemes: {}", schemes.join(", ")));
    }
    if let Some(hosts) = &restrictions.allowed_hosts {
        description.push(format!("Allowed hosts: {}", hosts.join(", ")));
    }
    if !description.is_empty() {
        m.insert(
            "description".to_string(),
            Value::String(description.join("; ")),
        );
    }
    m
}

fn quantity_schema(spec: &QuantitySpec) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert(
        "mantissa".to_string(),
        obj([("type", Value::String("integer".to_string()))]),
    );
    props.insert(
        "scale".to_string(),
        obj([("type", Value::String("integer".to_string()))]),
    );
    props.insert(
        "unit".to_string(),
        obj([("type", Value::String("string".to_string()))]),
    );
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(props));
    m.insert(
        "required".to_string(),
        Value::Array(vec![
            Value::String("mantissa".to_string()),
            Value::String("scale".to_string()),
            Value::String("unit".to_string()),
        ]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    m.insert(
        "title".to_string(),
        Value::String(format!("Quantity ({})", spec.base_unit)),
    );
    let mut description = Vec::new();
    if let Some(min) = &spec.min {
        description.push(format!("min: {}", render_quantity(min)));
    }
    if let Some(max) = &spec.max {
        description.push(format!("max: {}", render_quantity(max)));
    }
    if !description.is_empty() {
        m.insert(
            "description".to_string(),
            Value::String(description.join("; ")),
        );
    }
    m
}

fn render_quantity(q: &QuantityValue) -> String {
    format!("{}e-{} {}", q.mantissa, q.scale, q.unit)
}

fn secret_schema(_spec: &SecretSpec) -> Map<String, Value> {
    // Canonical Secret JSON shape: `{ secret_ref: string-non-empty }`.
    let mut secret_ref = Map::new();
    secret_ref.insert("type".to_string(), Value::String("string".to_string()));
    secret_ref.insert("minLength".to_string(), Value::Number(1.into()));
    let mut properties = Map::new();
    properties.insert("secret_ref".to_string(), Value::Object(secret_ref));
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("secret_ref".to_string())]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    m
}

fn quota_token_schema(_spec: &QuotaTokenSpec) -> Map<String, Value> {
    // Canonical QuotaToken JSON shape: see canonical/quota_token.rs.
    let env_id = obj_inline([
        ("type", Value::String("string".to_string())),
        ("format", Value::String("uuid".to_string())),
    ]);
    let resource_name = obj_inline([("type", Value::String("string".to_string()))]);
    let expected_use = obj_inline([(
        "oneOf",
        Value::Array(vec![
            obj_inline([
                ("type", Value::String("string".to_string())),
                ("pattern", Value::String("^[0-9]+$".to_string())),
            ]),
            obj_inline([
                ("type", Value::String("integer".to_string())),
                ("minimum", Value::Number(0u64.into())),
            ]),
        ]),
    )]);
    let last_credit = obj_inline([(
        "oneOf",
        Value::Array(vec![
            obj_inline([
                ("type", Value::String("string".to_string())),
                ("pattern", Value::String("^-?[0-9]+$".to_string())),
            ]),
            obj_inline([("type", Value::String("integer".to_string()))]),
        ]),
    )]);
    let last_credit_at = obj_inline([
        ("type", Value::String("string".to_string())),
        ("format", Value::String("date-time".to_string())),
    ]);
    let mut properties = Map::new();
    properties.insert("environment_id".to_string(), env_id);
    properties.insert("resource_name".to_string(), resource_name);
    properties.insert("expected_use".to_string(), expected_use);
    properties.insert("last_credit".to_string(), last_credit);
    properties.insert("last_credit_at".to_string(), last_credit_at);
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String("object".to_string()));
    m.insert("properties".to_string(), Value::Object(properties));
    m.insert(
        "required".to_string(),
        Value::Array(vec![
            Value::String("environment_id".to_string()),
            Value::String("resource_name".to_string()),
            Value::String("expected_use".to_string()),
            Value::String("last_credit".to_string()),
            Value::String("last_credit_at".to_string()),
        ]),
    );
    m.insert("additionalProperties".to_string(), Value::Bool(false));
    m
}

fn union_schema(
    graph: &SchemaGraph,
    spec: &UnionSpec,
    table: &BranchNameTable,
    config: JsonSchemaConfig,
) -> Map<String, Value> {
    // Each branch gets a per-branch reference into `$defs` (synthesised
    // by `add_union_branch_defs`), so the `oneOf` and any discriminator
    // mapping resolves against schemas the renderer actually emits. The
    // branch key is resolved through `BranchNameTable` so two unrelated
    // unions sharing a tag get disambiguated names.
    let one_of: Vec<Value> = spec
        .branches
        .iter()
        .map(|b| obj([("$ref", Value::String(ref_to_def_key(table.name_for(b))))]))
        .collect();
    let mut m = Map::new();
    m.insert("oneOf".to_string(), Value::Array(one_of));
    if let Some(disc_field) = openapi_discriminator(spec) {
        let mut mapping = Map::new();
        for branch in spec.branches.iter() {
            let literal = match &branch.discriminator {
                DiscriminatorRule::FieldEquals(disc) => disc.literal.clone(),
                _ => None,
            };
            if let Some(lit) = literal {
                mapping.insert(lit, Value::String(ref_to_def_key(table.name_for(branch))));
            }
        }
        let mut d = Map::new();
        d.insert("propertyName".to_string(), Value::String(disc_field));
        if !mapping.is_empty() {
            d.insert("mapping".to_string(), Value::Object(mapping));
        }
        m.insert("discriminator".to_string(), Value::Object(d));
    }
    let _ = graph;
    let _ = config;
    m
}

fn openapi_discriminator(spec: &UnionSpec) -> Option<String> {
    let mut field: Option<String> = None;
    for branch in spec.branches.iter() {
        match &branch.discriminator {
            DiscriminatorRule::FieldEquals(disc) => {
                let _ = disc.literal.as_ref()?;
                match &field {
                    Some(prev) if prev != &disc.field_name => return None,
                    Some(_) => {}
                    None => field = Some(disc.field_name.clone()),
                }
            }
            _ => return None,
        }
    }
    field
}

fn attach_metadata(target: &mut Value, metadata: &MetadataEnvelope) {
    if metadata.is_empty() {
        return;
    }
    let Some(obj) = target.as_object_mut() else {
        return;
    };
    if let Some(doc) = &metadata.doc {
        obj.entry("description")
            .or_insert(Value::String(doc.clone()));
    }
    if !metadata.examples.is_empty() {
        obj.entry("examples").or_insert_with(|| {
            Value::Array(
                metadata
                    .examples
                    .iter()
                    .map(|e| Value::String(e.clone()))
                    .collect(),
            )
        });
    }
    if let Some(dep) = &metadata.deprecated {
        obj.entry("deprecated").or_insert(Value::Bool(true));
        obj.entry("x-golem-deprecation-note")
            .or_insert(Value::String(dep.clone()));
    }
}

/// Number of base64url-no-pad characters required to encode `n` raw bytes.
/// Matches the alphabet used by [`crate::schema::canonical::binary`] which
/// uses `base64::engine::general_purpose::URL_SAFE_NO_PAD`.
fn base64url_no_pad_len(n: u32) -> u64 {
    let n = n as u64;
    4 * (n / 3)
        + match n % 3 {
            0 => 0,
            1 => 2,
            2 => 3,
            _ => unreachable!(),
        }
}

/// Escape a string for use as a single JSON-Pointer token: `~` becomes `~0`
/// and `/` becomes `~1` per RFC 6901.
fn escape_pointer_token(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            other => out.push(other),
        }
    }
    out
}

/// Escape a string for inclusion as a literal in a basic regex pattern.
fn regex_escape(s: &str) -> String {
    let specials: &[char] = &[
        '\\', '^', '$', '.', '|', '?', '*', '+', '(', ')', '[', ']', '{', '}',
    ];
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if specials.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn obj<I: IntoIterator<Item = (&'static str, Value)>>(entries: I) -> Value {
    let mut map = Map::new();
    for (k, v) in entries {
        map.insert(k.to_string(), v);
    }
    Value::Object(map)
}

fn obj_inline<I: IntoIterator<Item = (&'static str, Value)>>(entries: I) -> Value {
    obj(entries)
}
