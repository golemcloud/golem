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

//! Depth subtyping with semantic-scalar narrowing.
//!
//! `is_assignable(graph, sub, sup)` returns `true` when a value valid under
//! `sub` is also valid under `sup`.
//!
//! Records are compared by **exact** structural match: field counts must
//! agree, and for each position the field names must be equal and the bodies
//! mutually compatible per `is_assignable`. Width-subtyping and field
//! reordering are intentionally rejected because [`SchemaValue::Record`]
//! values are positional — they do not carry field names at runtime — so
//! reading a "wider" record as a narrower one would silently re-interpret
//! values by index.
//!
//! Variant payloads use invariant subtyping: matching cases either both
//! lack a payload, or both carry payloads that are assignable in both
//! directions.

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{
    BinaryRestrictions, PathSpec, QuantitySpec, QuantityValue, SchemaType, TextRestrictions,
    UrlRestrictions,
};
use std::collections::HashSet;

/// Is `sub` assignable to `sup` inside `graph`?
pub fn is_assignable(graph: &SchemaGraph, sub: &SchemaType, sup: &SchemaType) -> bool {
    let mut visited = HashSet::new();
    assignable(graph, sub, sup, &mut visited)
}

fn assignable(
    graph: &SchemaGraph,
    sub: &SchemaType,
    sup: &SchemaType,
    visited: &mut HashSet<(TypeId, TypeId)>,
) -> bool {
    // Resolve refs on both sides while tracking the pair to detect cycles.
    let (sub_resolved, sub_key) = resolve(graph, sub);
    let (sup_resolved, sup_key) = resolve(graph, sup);

    // Coinductive cycle break. The visited-pair set is keyed by the last
    // resolved-ref id on each side so equivalent `(SubRef, SupRef)` pair
    // re-entries terminate with an accept rather than recursing. This is
    // the standard coinductive equirecursive-type rule and is sound because
    // any disagreement on the recursion will be observed on its first
    // (acyclic) visit.
    if let (Some(a), Some(b)) = (sub_key, sup_key)
        && !visited.insert((a, b))
    {
        return true;
    }

    match (sub_resolved, sup_resolved) {
        // Primitives must match exactly.
        (SchemaType::Bool { .. }, SchemaType::Bool { .. })
        | (SchemaType::S8 { .. }, SchemaType::S8 { .. })
        | (SchemaType::S16 { .. }, SchemaType::S16 { .. })
        | (SchemaType::S32 { .. }, SchemaType::S32 { .. })
        | (SchemaType::S64 { .. }, SchemaType::S64 { .. })
        | (SchemaType::U8 { .. }, SchemaType::U8 { .. })
        | (SchemaType::U16 { .. }, SchemaType::U16 { .. })
        | (SchemaType::U32 { .. }, SchemaType::U32 { .. })
        | (SchemaType::U64 { .. }, SchemaType::U64 { .. })
        | (SchemaType::F32 { .. }, SchemaType::F32 { .. })
        | (SchemaType::F64 { .. }, SchemaType::F64 { .. })
        | (SchemaType::Char { .. }, SchemaType::Char { .. })
        | (SchemaType::String { .. }, SchemaType::String { .. })
        | (SchemaType::Datetime { .. }, SchemaType::Datetime { .. })
        | (SchemaType::Duration { .. }, SchemaType::Duration { .. }) => true,

        (
            SchemaType::Text {
                restrictions: a, ..
            },
            SchemaType::Text {
                restrictions: b, ..
            },
        ) => text_narrows(a, b),
        (
            SchemaType::Binary {
                restrictions: a, ..
            },
            SchemaType::Binary {
                restrictions: b, ..
            },
        ) => binary_narrows(a, b),
        (
            SchemaType::Url {
                restrictions: a, ..
            },
            SchemaType::Url {
                restrictions: b, ..
            },
        ) => url_narrows(a, b),
        (SchemaType::Path { spec: a, .. }, SchemaType::Path { spec: b, .. }) => path_narrows(a, b),
        (SchemaType::Quantity { spec: a, .. }, SchemaType::Quantity { spec: b, .. }) => {
            quantity_narrows(a, b)
        }

        // Records: exact structural match (no width / reorder subtyping).
        (SchemaType::Record { fields: a, .. }, SchemaType::Record { fields: b, .. }) => {
            if a.len() != b.len() {
                return false;
            }
            a.iter().zip(b.iter()).all(|(af, bf)| {
                af.name == bf.name && assignable(graph, &af.body, &bf.body, visited)
            })
        }

        // Variants: exact case-name match, invariant on payload.
        (SchemaType::Variant { cases: a, .. }, SchemaType::Variant { cases: b, .. }) => {
            a.len() == b.len()
                && a.iter().zip(b.iter()).all(|(ac, bc)| {
                    ac.name == bc.name
                        && match (&ac.payload, &bc.payload) {
                            (None, None) => true,
                            (Some(ap), Some(bp)) => {
                                assignable(graph, ap, bp, visited)
                                    && assignable(graph, bp, ap, visited)
                            }
                            _ => false,
                        }
                })
        }

        // Enums: exact-match by case names.
        (SchemaType::Enum { cases: a, .. }, SchemaType::Enum { cases: b, .. }) => a == b,

        // Flags: exact-match.
        (SchemaType::Flags { flags: a, .. }, SchemaType::Flags { flags: b, .. }) => a == b,

        // Tuples: same length, depth-subtyping per element.
        (SchemaType::Tuple { elements: a, .. }, SchemaType::Tuple { elements: b, .. }) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(ae, be)| assignable(graph, ae, be, visited))
        }

        (SchemaType::List { element: a, .. }, SchemaType::List { element: b, .. }) => {
            assignable(graph, a, b, visited)
        }
        (
            SchemaType::FixedList {
                element: a,
                length: na,
                ..
            },
            SchemaType::FixedList {
                element: b,
                length: nb,
                ..
            },
        ) => na == nb && assignable(graph, a, b, visited),
        (
            SchemaType::Map {
                key: ak, value: av, ..
            },
            SchemaType::Map {
                key: bk, value: bv, ..
            },
        ) => assignable(graph, ak, bk, visited) && assignable(graph, av, bv, visited),

        (SchemaType::Option { inner: a, .. }, SchemaType::Option { inner: b, .. }) => {
            assignable(graph, a, b, visited)
        }
        (SchemaType::Result { spec: a, .. }, SchemaType::Result { spec: b, .. }) => {
            let ok_ok = match (&a.ok, &b.ok) {
                (None, None) => true,
                (Some(ax), Some(bx)) => assignable(graph, ax, bx, visited),
                _ => false,
            };
            let err_ok = match (&a.err, &b.err) {
                (None, None) => true,
                (Some(ax), Some(bx)) => assignable(graph, ax, bx, visited),
                _ => false,
            };
            ok_ok && err_ok
        }

        // Capabilities are invariant on kind/category; secret payload types are
        // compared recursively so refs in `inner` resolve like every other
        // nested schema type.
        (SchemaType::Secret { spec: a, .. }, SchemaType::Secret { spec: b, .. }) => {
            a.category == b.category && assignable(graph, &a.inner, &b.inner, visited)
        }
        (SchemaType::QuotaToken { spec: a, .. }, SchemaType::QuotaToken { spec: b, .. }) => a == b,

        // Unions: exact match by tag set and per-branch body assignability.
        (SchemaType::Union { spec: a, .. }, SchemaType::Union { spec: b, .. }) => {
            if a.branches.len() != b.branches.len() {
                return false;
            }
            b.branches.iter().all(|sup_branch| {
                a.branches
                    .iter()
                    .find(|sb| sb.tag == sup_branch.tag)
                    .map(|sb| {
                        sb.discriminator == sup_branch.discriminator
                            && assignable(graph, &sb.body, &sup_branch.body, visited)
                    })
                    .unwrap_or(false)
            })
        }

        // Same-id refs that could not be expanded further (dangling or
        // resolved to themselves) are assumed equal.
        (SchemaType::Ref { id: a, .. }, SchemaType::Ref { id: b, .. }) => a == b,

        // Future / Stream stubs: same shape, optional inner type assignable.
        (SchemaType::Future { inner: a, .. }, SchemaType::Future { inner: b, .. }) => {
            match (a, b) {
                (None, None) => true,
                (Some(ai), Some(bi)) => assignable(graph, ai, bi, visited),
                _ => false,
            }
        }
        (SchemaType::Stream { inner: a, .. }, SchemaType::Stream { inner: b, .. }) => {
            match (a, b) {
                (None, None) => true,
                (Some(ai), Some(bi)) => assignable(graph, ai, bi, visited),
                _ => false,
            }
        }

        // Mismatched kinds (post-resolution) are not assignable.
        _ => false,
    }
}

/// Resolve `Ref` chains. Returns `(resolved_type, Some(last_ref_id))` when
/// any ref was followed; the ref id is used as part of the cycle-detection
/// key.
fn resolve<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> (&'a SchemaType, Option<TypeId>) {
    let mut last_ref: Option<TypeId> = None;
    let mut visited_ids: HashSet<TypeId> = HashSet::new();
    loop {
        match ty {
            SchemaType::Ref { id, .. } => {
                if !visited_ids.insert(id.clone()) {
                    return (ty, Some(id.clone()));
                }
                last_ref = Some(id.clone());
                match graph.lookup(id) {
                    Some(def) => ty = &def.body,
                    None => return (ty, last_ref),
                }
            }
            other => return (other, last_ref),
        }
    }
}

// --- Scalar narrowing rules ---

fn text_narrows(sub: &TextRestrictions, sup: &TextRestrictions) -> bool {
    // sub.min_length >= sup.min_length (sub is at least as constrained)
    if !u32_min_at_least(sub.min_length, sup.min_length) {
        return false;
    }
    // sub.max_length <= sup.max_length
    if !u32_max_at_most(sub.max_length, sup.max_length) {
        return false;
    }
    if !subset_languages(&sub.languages, &sup.languages) {
        return false;
    }
    // For regex we require equality or sup unconstrained; we cannot decide
    // regex subset structurally.
    match (&sub.regex, &sup.regex) {
        (_, None) => true,
        (Some(a), Some(b)) => a == b,
        (None, Some(_)) => false,
    }
}

fn binary_narrows(sub: &BinaryRestrictions, sup: &BinaryRestrictions) -> bool {
    if !u32_min_at_least(sub.min_bytes, sup.min_bytes) {
        return false;
    }
    if !u32_max_at_most(sub.max_bytes, sup.max_bytes) {
        return false;
    }
    subset_strings(&sub.mime_types, &sup.mime_types)
}

fn url_narrows(sub: &UrlRestrictions, sup: &UrlRestrictions) -> bool {
    subset_strings(&sub.allowed_schemes, &sup.allowed_schemes)
        && subset_strings(&sub.allowed_hosts, &sup.allowed_hosts)
}

/// Path direction is **invariant** between sub and sup on purpose: an
/// `Input` path cannot be substituted for an `Output` path (and vice versa)
/// without changing the data-flow direction the consumer relies on.
fn path_narrows(sub: &PathSpec, sup: &PathSpec) -> bool {
    sub.direction == sup.direction
        && (sup.kind == crate::schema::schema_type::PathKind::Any || sub.kind == sup.kind)
        && subset_strings(&sub.allowed_mime_types, &sup.allowed_mime_types)
        && subset_strings(&sub.allowed_extensions, &sup.allowed_extensions)
}

fn quantity_narrows(sub: &QuantitySpec, sup: &QuantitySpec) -> bool {
    if sub.base_unit != sup.base_unit {
        return false;
    }
    // `allowed_suffixes` must be a subset (set semantics).
    let sub_suffixes: HashSet<&String> = sub.allowed_suffixes.iter().collect();
    let sup_suffixes: HashSet<&String> = sup.allowed_suffixes.iter().collect();
    if !sub_suffixes.is_subset(&sup_suffixes) {
        return false;
    }
    // sub's min >= sup's min, sub's max <= sup's max, both compared as
    // canonical fixed-point in `base_unit`.
    match (&sub.min, &sup.min) {
        (_, None) => {}
        (Some(a), Some(b)) => {
            if !quantity_le(b, a) {
                return false;
            }
        }
        (None, Some(_)) => return false,
    }
    match (&sub.max, &sup.max) {
        (_, None) => {}
        (Some(a), Some(b)) => {
            if !quantity_le(a, b) {
                return false;
            }
        }
        (None, Some(_)) => return false,
    }
    true
}

fn u32_min_at_least(sub: Option<u32>, sup: Option<u32>) -> bool {
    match (sub, sup) {
        (_, None) => true,
        (Some(a), Some(b)) => a >= b,
        (None, Some(_)) => false,
    }
}

fn u32_max_at_most(sub: Option<u32>, sup: Option<u32>) -> bool {
    match (sub, sup) {
        (_, None) => true,
        (Some(a), Some(b)) => a <= b,
        (None, Some(_)) => false,
    }
}

/// `sub` is a subset of `sup` for an `Option<Vec<String>>` allow-list where
/// `None` means "unrestricted".
fn subset_strings(sub: &Option<Vec<String>>, sup: &Option<Vec<String>>) -> bool {
    match (sub, sup) {
        (_, None) => true,
        (Some(a), Some(b)) => a.iter().all(|x| b.contains(x)),
        (None, Some(_)) => false,
    }
}

fn subset_languages(sub: &Option<Vec<String>>, sup: &Option<Vec<String>>) -> bool {
    subset_strings(sub, sup)
}

fn quantity_le(a: &QuantityValue, b: &QuantityValue) -> bool {
    let common = a.scale.max(b.scale);
    let a_shift = (common - a.scale).max(0) as u32;
    let b_shift = (common - b.scale).max(0) as u32;

    let ten: i128 = 10;
    let a_factor = match ten.checked_pow(a_shift) {
        Some(v) => v,
        None => return a.scale == b.scale && a.mantissa <= b.mantissa,
    };
    let b_factor = match ten.checked_pow(b_shift) {
        Some(v) => v,
        None => return a.scale == b.scale && a.mantissa <= b.mantissa,
    };
    let a_canon = match (a.mantissa as i128).checked_mul(a_factor) {
        Some(v) => v,
        None => return a.scale == b.scale && a.mantissa <= b.mantissa,
    };
    let b_canon = match (b.mantissa as i128).checked_mul(b_factor) {
        Some(v) => v,
        None => return a.scale == b.scale && a.mantissa <= b.mantissa,
    };
    a_canon <= b_canon
}
