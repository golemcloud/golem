// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

impl ToolSchemaRepr for SchemaGraph {
    type Value = SchemaValue;

    fn list(&self) -> Self {
        list_wrapper_graph(self)
    }

    fn map_value(&self) -> Option<Self> {
        resolves_to_map(self).then(|| map_value_graph(self))
    }

    fn shape_matches(&self, other: &Self) -> bool {
        schema_shapes_match(self, other)
    }

    fn is_sound(&self) -> bool {
        comparand_graph_is_sound(self)
    }

    fn value_is_literal(
        &self,
        literal: &ToolLiteral,
        whole_or_one_peel: bool,
    ) -> Result<Self::Value, ToolBuildError> {
        let value = value_is_literal_to_schema_value(self, literal)?;
        let comparand = ValueIsComparand {
            graph: self.clone(),
            mode: if whole_or_one_peel {
                ValueIsMode::WholeOrOnePeel
            } else {
                ValueIsMode::Exact
            },
        };
        if value_is_compatible(&comparand, &value) {
            Ok(value)
        } else {
            Err(ToolBuildError::ValueIsTypeMismatch(String::new()))
        }
    }
}

pub(super) fn schema_graph_root(graph: &SchemaGraph) -> SchemaType {
    graph.root.clone()
}

/// The whole collected value type of an option (used to validate an option's
/// `default`): a `repeatable-list` collects into `list<item>`, a
/// `repeatable-map` into its map node; scalar/optional-scalar use the value
/// type directly. Definition graphs are preserved so refs still resolve.
pub fn option_collected_graph<S: ToolSchemaRepr>(s: &ExtendedOptionShape<S>) -> S {
    match s {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g.clone(),
        ExtendedOptionShape::RepeatableList(r) => r.item_type.list(),
        ExtendedOptionShape::RepeatableMap(r) => r.map_type.clone(),
    }
}

pub(super) fn canonical_option_graph(option: &ExtendedOptionSpec) -> SchemaGraph {
    let collected = option_collected_graph(&option.shape);
    if !option.required
        && option.default.is_none()
        && !matches!(
            &option.shape,
            ExtendedOptionShape::RepeatableList(_) | ExtendedOptionShape::RepeatableMap(_)
        )
        && !matches!(&collected.root, SchemaType::Option { .. })
    {
        option_wrapper_graph(&collected)
    } else {
        collected
    }
}

pub(super) fn canonical_positional_graph(positional: &ExtendedPositional) -> SchemaGraph {
    if !positional.required
        && positional.default.is_none()
        && !matches!(&positional.type_.root, SchemaType::Option { .. })
    {
        option_wrapper_graph(&positional.type_)
    } else {
        positional.type_.clone()
    }
}

pub(super) fn option_wrapper_graph(inner: &SchemaGraph) -> SchemaGraph {
    SchemaGraph {
        defs: inner.defs.clone(),
        root: SchemaType::option(inner.root.clone()),
    }
}

/// The full input value type of an option (used for the "no variant in input
/// position" check). A `repeatable-list` stores its element type; a
/// `repeatable-map` stores the whole map node so both key and value are reached.
pub(super) fn option_input_graph(s: &ExtendedOptionShape) -> SchemaGraph {
    match s {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g.clone(),
        ExtendedOptionShape::RepeatableList(r) => r.item_type.clone(),
        ExtendedOptionShape::RepeatableMap(r) => r.map_type.clone(),
    }
}

/// Wrap a graph's root in a `list`, preserving the original definitions so any
/// `Ref` in the element type still resolves.
pub(super) fn list_wrapper_graph(item: &SchemaGraph) -> SchemaGraph {
    SchemaGraph {
        defs: item.defs.clone(),
        root: SchemaType::list(item.root.clone()),
    }
}

/// The derived input-record field type for a flag (`bool` for a bool-flag,
/// `u32` for a count-flag). Flags carry no author-supplied value type, so this
/// is used only by [`ExtendedToolType::canonical_input_fields`]; a `value-is`
/// literal against a flag is rejected rather than checked against this type.
pub(super) fn flag_graph(f: &FlagSpec) -> SchemaGraph {
    let ty = match f.shape {
        wire::FlagShape::BoolFlag(_) => SchemaType::bool(),
        wire::FlagShape::CountFlag(_) => SchemaType::u32(),
    };
    SchemaGraph::anonymous(ty)
}

/// Whether a comparand graph is structurally sound (no dangling references or
/// pure-alias cycles). When it is not, [`validate_tool`] reports the schema
/// error (an [`ToolBuildError::UnresolvedTypeRef`] / [`ToolBuildError::IllFormedSchema`]),
/// so `value-is` resolution must not also report a cascading mismatch against a
/// graph that cannot be resolved.
fn comparand_graph_is_sound(graph: &SchemaGraph) -> bool {
    golem_schema::schema::validation::validate_graph(graph).is_ok()
}

/// The per-entry value comparand for a map: a graph whose root is the map's
/// value type, with the map graph's definitions preserved so any `Ref` in the
/// value type still resolves. Falls back to the original graph when the root
/// does not resolve to a `Map` (a not-a-map repeatable-map type is reported
/// separately, so this avoids fabricating the value comparand).
fn map_value_graph(map: &SchemaGraph) -> SchemaGraph {
    match map.resolve_ref(&map.root) {
        Ok(SchemaType::Map { value, .. }) => SchemaGraph {
            defs: map.defs.clone(),
            root: (**value).clone(),
        },
        _ => map.clone(),
    }
}

/// Returns true if the graph's root resolves (through any `Ref`s) to a `Map`.
pub(super) fn resolves_to_map(map: &SchemaGraph) -> bool {
    matches!(map.resolve_ref(&map.root), Ok(SchemaType::Map { .. }))
}

/// Maximum recursion depth for structural shape comparison; deeper than this the
/// comparison gives up and reports "not a match". This keeps the comparison
/// terminating on pathological (deeply nested or recursive) types. Reporting
/// "not a match" on exhaustion is the safe direction: a non-match between two
/// same-named declarations surfaces as an explicit
/// [`ToolBuildError::InheritedGlobalConflict`] rather than silently dropping a
/// local parameter that might actually differ. Real CLI input schemas (strings,
/// numbers, bools, lists, maps, small records) are far shallower than this.
const SHAPE_MATCH_MAX_DEPTH: u32 = 32;

/// Whether two canonical input value graphs describe the same value *shape*,
/// ignoring metadata and validation restrictions (docs, numeric/text bounds,
/// etc.) but honoring structure and exact primitive representation. References
/// are resolved against their respective graphs.
///
/// Recursive (cyclic) graphs are compared coinductively: when the same pair of
/// referenced definitions is reached again along a path, the two shapes are
/// assumed to match (the cycle has already been established structurally). This
/// makes an identical recursive type re-declaration compare equal instead of
/// being falsely rejected once the structural recursion bottoms out. The
/// per-pair memo is what guarantees termination; the depth counter is a defensive
/// secondary guard for pathologically deep finite types.
pub(super) fn schema_shapes_match(a: &SchemaGraph, b: &SchemaGraph) -> bool {
    let mut visiting = std::collections::HashSet::new();
    schema_types_match(a, &a.root, b, &b.root, SHAPE_MATCH_MAX_DEPTH, &mut visiting)
}

fn schema_types_match(
    a_graph: &SchemaGraph,
    a_ty: &SchemaType,
    b_graph: &SchemaGraph,
    b_ty: &SchemaType,
    depth: u32,
    visiting: &mut std::collections::HashSet<(
        golem_schema::schema::TypeId,
        golem_schema::schema::TypeId,
    )>,
) -> bool {
    // Break recursion at reference boundaries before resolving: revisiting the
    // same pair of named definitions means we have already entered comparing
    // them, so the recursive shapes coincide along this path.
    if let (SchemaType::Ref { id: a_id, .. }, SchemaType::Ref { id: b_id, .. }) = (a_ty, b_ty)
        && !visiting.insert((a_id.clone(), b_id.clone()))
    {
        return true;
    }
    let (Ok(a_ty), Ok(b_ty)) = (a_graph.resolve_ref(a_ty), b_graph.resolve_ref(b_ty)) else {
        return false;
    };
    if depth == 0 {
        return false;
    }
    let next = depth - 1;
    match (a_ty, b_ty) {
        (SchemaType::List { element: ea, .. }, SchemaType::List { element: eb, .. }) => {
            schema_types_match(a_graph, ea, b_graph, eb, next, visiting)
        }
        (
            SchemaType::FixedList {
                element: ea,
                length: la,
                ..
            },
            SchemaType::FixedList {
                element: eb,
                length: lb,
                ..
            },
        ) => la == lb && schema_types_match(a_graph, ea, b_graph, eb, next, visiting),
        (SchemaType::Option { inner: ia, .. }, SchemaType::Option { inner: ib, .. }) => {
            schema_types_match(a_graph, ia, b_graph, ib, next, visiting)
        }
        (
            SchemaType::Map {
                key: ka, value: va, ..
            },
            SchemaType::Map {
                key: kb, value: vb, ..
            },
        ) => {
            schema_types_match(a_graph, ka, b_graph, kb, next, visiting)
                && schema_types_match(a_graph, va, b_graph, vb, next, visiting)
        }
        (SchemaType::Tuple { elements: ea, .. }, SchemaType::Tuple { elements: eb, .. }) => {
            ea.len() == eb.len()
                && ea
                    .iter()
                    .zip(eb)
                    .all(|(x, y)| schema_types_match(a_graph, x, b_graph, y, next, visiting))
        }
        (SchemaType::Record { fields: fa, .. }, SchemaType::Record { fields: fb, .. }) => {
            fa.len() == fb.len()
                && fa.iter().zip(fb).all(|(x, y)| {
                    x.name == y.name
                        && schema_types_match(a_graph, &x.body, b_graph, &y.body, next, visiting)
                })
        }
        (SchemaType::Variant { cases: ca, .. }, SchemaType::Variant { cases: cb, .. }) => {
            ca.len() == cb.len()
                && ca.iter().zip(cb).all(|(x, y)| {
                    x.name == y.name
                        && match (&x.payload, &y.payload) {
                            (None, None) => true,
                            (Some(px), Some(py)) => {
                                schema_types_match(a_graph, px, b_graph, py, next, visiting)
                            }
                            _ => false,
                        }
                })
        }
        (SchemaType::Union { spec: sa, .. }, SchemaType::Union { spec: sb, .. }) => {
            sa.branches.len() == sb.branches.len()
                && sa.branches.iter().zip(&sb.branches).all(|(x, y)| {
                    x.tag == y.tag
                        && x.discriminator == y.discriminator
                        && schema_types_match(a_graph, &x.body, b_graph, &y.body, next, visiting)
                })
        }
        (SchemaType::Enum { cases: ca, .. }, SchemaType::Enum { cases: cb, .. }) => ca == cb,
        (SchemaType::Flags { flags: fa, .. }, SchemaType::Flags { flags: fb, .. }) => fa == fb,
        // Rich leaf types whose spec carries *type identity* (not just refinable
        // validation restrictions) must compare those identity fields. Otherwise
        // a leaf could de-project an inherited global onto a genuinely different
        // Rust type — e.g. `Quantity<Meters>` vs `Quantity<Seconds>`, which
        // share the `Quantity` discriminant but are not interchangeable values.
        // Identity here means the parts of the spec derived from the Rust type
        // (not overlaid by `#[arg]`): the quantity unit set, the secret payload
        // type and category, the quota-token resource, and the unstructured text
        // languages / binary MIME sets. Pure validation restrictions
        // (numeric/text/url bounds, path direction/kind — all `#[arg]`-refinable)
        // stay ignored per this function's contract.
        (SchemaType::Quantity { spec: sa, .. }, SchemaType::Quantity { spec: sb, .. }) => {
            sa.base_unit == sb.base_unit
                && str_sets_match(
                    &effective_quantity_units(&sa.base_unit, &sa.allowed_suffixes),
                    &effective_quantity_units(&sb.base_unit, &sb.allowed_suffixes),
                )
        }
        (SchemaType::Secret { spec: sa, .. }, SchemaType::Secret { spec: sb, .. }) => {
            sa.category == sb.category
                && schema_types_match(a_graph, &sa.inner, b_graph, &sb.inner, next, visiting)
        }
        (SchemaType::QuotaToken { spec: sa, .. }, SchemaType::QuotaToken { spec: sb, .. }) => {
            sa.resource_name == sb.resource_name
        }
        (
            SchemaType::PermissionCard { spec: sa, .. },
            SchemaType::PermissionCard { spec: sb, .. },
        ) => sa.polymorphic == sb.polymorphic,
        (
            SchemaType::Text {
                restrictions: ra, ..
            },
            SchemaType::Text {
                restrictions: rb, ..
            },
        ) => opt_str_sets_match(&ra.languages, &rb.languages),
        (
            SchemaType::Binary {
                restrictions: ra, ..
            },
            SchemaType::Binary {
                restrictions: rb, ..
            },
        ) => opt_str_sets_match(&ra.mime_types, &rb.mime_types),
        // A plain `String` and a `Text` differ only by the latter carrying
        // refinable restrictions (regex/min/max), which `#[arg]` overlays via
        // `refine_text` (the only `String`→`Text` promotion). So an inherited
        // refined-`String` global (`Text`) and a leaf redeclaring the same plain
        // `String` describe the same shape and must de-project. The exception is
        // a `languages`-restricted `Text`, which reflects a different Rust type
        // (`UnstructuredText<…>`) and stays incompatible with a plain `String`.
        (SchemaType::String { .. }, SchemaType::Text { restrictions, .. })
        | (SchemaType::Text { restrictions, .. }, SchemaType::String { .. }) => {
            restrictions.languages.is_none()
        }
        (SchemaType::Result { spec: sa, .. }, SchemaType::Result { spec: sb, .. }) => {
            schema_opt_box_match(
                a_graph,
                sa.ok.as_deref(),
                b_graph,
                sb.ok.as_deref(),
                next,
                visiting,
            ) && schema_opt_box_match(
                a_graph,
                sa.err.as_deref(),
                b_graph,
                sb.err.as_deref(),
                next,
                visiting,
            )
        }
        (SchemaType::Future { inner: ia, .. }, SchemaType::Future { inner: ib, .. })
        | (SchemaType::Stream { inner: ia, .. }, SchemaType::Stream { inner: ib, .. }) => {
            schema_opt_box_match(
                a_graph,
                ia.as_deref(),
                b_graph,
                ib.as_deref(),
                next,
                visiting,
            )
        }
        // Primitives and the remaining rich leaf types (incl. distinct numeric
        // widths/signs, `Url`, `Path`, `Datetime`, `Duration`) are compared by
        // kind, which already ignores their refinable restrictions.
        _ => std::mem::discriminant(a_ty) == std::mem::discriminant(b_ty),
    }
}

/// Whether two string collections describe the same *set* of values (order- and
/// duplicate-insensitive). Used to compare rich-type identity fields that are
/// authored as ordered lists but semantically unordered (quantity units, allowed
/// languages, allowed MIME types).
fn str_sets_match(a: &[String], b: &[String]) -> bool {
    let sa: std::collections::HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: std::collections::HashSet<&str> = b.iter().map(String::as_str).collect();
    sa == sb
}

/// Set comparison for an optional restriction (`None` = unrestricted). An
/// unrestricted side never matches a restricted side: they describe different
/// accepted value sets.
fn opt_str_sets_match(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => str_sets_match(a, b),
        _ => false,
    }
}

/// The set of units a [`SchemaType::Quantity`] accepts: its explicit
/// `allowed_suffixes`, or just the canonical `base_unit` when no suffixes are
/// declared. Two quantities with the same base unit but different accepted unit
/// sets are not interchangeable, so de-projection must treat them as distinct.
fn effective_quantity_units(base_unit: &str, allowed_suffixes: &[String]) -> Vec<String> {
    if allowed_suffixes.is_empty() {
        vec![base_unit.to_string()]
    } else {
        allowed_suffixes.to_vec()
    }
}

fn schema_opt_box_match(
    a_graph: &SchemaGraph,
    a_ty: Option<&SchemaType>,
    b_graph: &SchemaGraph,
    b_ty: Option<&SchemaType>,
    depth: u32,
    visiting: &mut std::collections::HashSet<(
        golem_schema::schema::TypeId,
        golem_schema::schema::TypeId,
    )>,
) -> bool {
    match (a_ty, b_ty) {
        (None, None) => true,
        (Some(x), Some(y)) => schema_types_match(a_graph, x, b_graph, y, depth, visiting),
        _ => false,
    }
}
