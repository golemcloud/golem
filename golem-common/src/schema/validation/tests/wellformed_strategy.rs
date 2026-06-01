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

//! Strategies that produce only well-formed [`SchemaGraph`] values, i.e.,
//! graphs that pass [`crate::schema::validation::validate_graph`].
//!
//! Built by post-processing trees from the unconstrained structural
//! strategy: duplicates are dedup'd, `FixedList` lengths are clamped to >= 1,
//! map keys are forced to primitive, quantity ranges are normalised, and
//! union branches are filtered to ones whose body satisfies their
//! discriminator rule.

use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, QuantitySpec,
    SchemaType, TextRestrictions, UnionBranch, UnionSpec, VariantCaseType,
};
use crate::schema::tests::strategies::schema_graph_strategy;
use proptest::prelude::*;
use std::collections::HashSet;

pub fn wellformed_schema_graph_strategy() -> impl Strategy<Value = SchemaGraph> {
    schema_graph_strategy().prop_map(|mut g| {
        // Dedup defs by id; keep first occurrence.
        let mut seen = HashSet::new();
        g.defs.retain(|def| seen.insert(def.id.clone()));

        let known: HashSet<TypeId> = g.defs.iter().map(|d| d.id.clone()).collect();
        let new_defs: Vec<SchemaTypeDef> = g
            .defs
            .iter()
            .map(|d| SchemaTypeDef {
                id: d.id.clone(),
                name: d.name.clone(),
                body: sanitise_type(&d.body, &known),
            })
            .collect();
        g.defs = new_defs;
        g.root = sanitise_type(&g.root, &known);
        // Second pass: now that the full def set is known, walk every type
        // and wrap option<X> whose X (after ref resolution) is nullable. The
        // first sanitise_type pass treats `Ref(id)` conservatively because
        // it does not have the post-sanitise def bodies available.
        let g_snapshot = g.clone();
        g.defs = g_snapshot
            .defs
            .iter()
            .map(|d| SchemaTypeDef {
                id: d.id.clone(),
                name: d.name.clone(),
                body: unnest_nullable_options(&d.body, &g_snapshot),
            })
            .collect();
        g.root = unnest_nullable_options(&g.root, &g_snapshot);
        g
    })
}

fn unnest_nullable_options(ty: &SchemaType, graph: &SchemaGraph) -> SchemaType {
    match ty {
        SchemaType::Option { inner, .. } => {
            let new_inner = unnest_nullable_options(inner, graph);
            if resolved_is_nullable(&new_inner, graph, &mut HashSet::new()) {
                SchemaType::option(wrap_in_variant(new_inner))
            } else {
                SchemaType::option(new_inner)
            }
        }
        SchemaType::Record { fields, .. } => SchemaType::record(
            fields
                .iter()
                .map(|f| NamedFieldType {
                    name: f.name.clone(),
                    body: unnest_nullable_options(&f.body, graph),
                    metadata: f.metadata.clone(),
                })
                .collect(),
        ),
        SchemaType::Variant { cases, .. } => SchemaType::variant(
            cases
                .iter()
                .map(|c| VariantCaseType {
                    name: c.name.clone(),
                    payload: c
                        .payload
                        .as_ref()
                        .map(|p| unnest_nullable_options(p, graph)),
                    metadata: c.metadata.clone(),
                })
                .collect(),
        ),
        SchemaType::Tuple { elements, .. } => SchemaType::tuple(
            elements
                .iter()
                .map(|e| unnest_nullable_options(e, graph))
                .collect(),
        ),
        SchemaType::List { element, .. } => {
            SchemaType::list(unnest_nullable_options(element, graph))
        }
        SchemaType::FixedList {
            element, length, ..
        } => SchemaType::fixed_list(unnest_nullable_options(element, graph), *length),
        SchemaType::Map { key, value, .. } => SchemaType::map(
            unnest_nullable_options(key, graph),
            unnest_nullable_options(value, graph),
        ),
        SchemaType::Result { spec, .. } => {
            SchemaType::result(crate::schema::schema_type::ResultSpec {
                ok: spec
                    .ok
                    .as_ref()
                    .map(|t| Box::new(unnest_nullable_options(t, graph))),
                err: spec
                    .err
                    .as_ref()
                    .map(|t| Box::new(unnest_nullable_options(t, graph))),
            })
        }
        SchemaType::Union { spec, .. } => SchemaType::union(UnionSpec {
            branches: spec
                .branches
                .iter()
                .map(|b| UnionBranch {
                    tag: b.tag.clone(),
                    body: unnest_nullable_options(&b.body, graph),
                    discriminator: b.discriminator.clone(),
                    metadata: b.metadata.clone(),
                })
                .collect(),
        }),
        other => other.clone(),
    }
}

fn resolved_is_nullable(
    ty: &SchemaType,
    graph: &SchemaGraph,
    visited: &mut HashSet<TypeId>,
) -> bool {
    match ty {
        SchemaType::Option { .. } => true,
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter()
            .any(|b| resolved_is_nullable(&b.body, graph, visited)),
        SchemaType::Ref { id, .. } => {
            if !visited.insert(id.clone()) {
                return false;
            }
            let result = match graph.lookup(id) {
                Some(def) => resolved_is_nullable(&def.body, graph, visited),
                None => false,
            };
            visited.remove(id);
            result
        }
        _ => false,
    }
}

fn sanitise_type(ty: &SchemaType, known: &HashSet<TypeId>) -> SchemaType {
    match ty {
        SchemaType::Ref { id, .. } => {
            if known.contains(id) {
                SchemaType::ref_to(id.clone())
            } else {
                // Replace dangling refs with a harmless primitive.
                SchemaType::bool()
            }
        }
        SchemaType::Record { fields, .. } => {
            let mut seen = HashSet::new();
            let new_fields: Vec<NamedFieldType> = fields
                .iter()
                .filter(|f| seen.insert(f.name.clone()))
                .map(|f| NamedFieldType {
                    name: f.name.clone(),
                    body: sanitise_type(&f.body, known),
                    metadata: f.metadata.clone(),
                })
                .collect();
            SchemaType::record(new_fields)
        }
        SchemaType::Variant { cases, .. } => {
            let mut seen = HashSet::new();
            let mut new_cases: Vec<VariantCaseType> = cases
                .iter()
                .filter(|c| seen.insert(c.name.clone()))
                .map(|c| VariantCaseType {
                    name: c.name.clone(),
                    payload: c.payload.as_ref().map(|p| sanitise_type(p, known)),
                    metadata: c.metadata.clone(),
                })
                .collect();
            if new_cases.is_empty() {
                new_cases.push(VariantCaseType {
                    name: "default".to_string(),
                    payload: None,
                    metadata: Default::default(),
                });
            }
            SchemaType::variant(new_cases)
        }
        SchemaType::Enum { cases, .. } => {
            let mut seen = HashSet::new();
            let mut new_cases: Vec<String> = cases
                .iter()
                .filter(|c| seen.insert((*c).clone()))
                .cloned()
                .collect();
            if new_cases.is_empty() {
                new_cases.push("default".to_string());
            }
            SchemaType::r#enum(new_cases)
        }
        SchemaType::Flags { flags, .. } => {
            let mut seen = HashSet::new();
            let mut new_flags: Vec<String> = flags
                .iter()
                .filter(|f| seen.insert((*f).clone()))
                .cloned()
                .collect();
            if new_flags.is_empty() {
                new_flags.push("default".to_string());
            }
            SchemaType::flags(new_flags)
        }
        SchemaType::Tuple { elements, .. } => {
            SchemaType::tuple(elements.iter().map(|e| sanitise_type(e, known)).collect())
        }
        SchemaType::List { element, .. } => SchemaType::list(sanitise_type(element, known)),
        SchemaType::FixedList {
            element, length, ..
        } => SchemaType::fixed_list(sanitise_type(element, known), (*length).max(1)),
        SchemaType::Map { value, .. } => {
            // Force the key to a fixed primitive type to satisfy the map-key
            // policy.
            SchemaType::map(SchemaType::string(), sanitise_type(value, known))
        }
        SchemaType::Option { inner, .. } => {
            let inner_sanitised = sanitise_type(inner, known);
            // Reject nullable-nesting at the strategy level: an `option<X>`
            // where `X` can itself encode as `null` collapses on the canonical
            // wire form. Wrap the inner type in a single-case variant so the
            // resulting `option<variant{wrap(X)}>` is well-formed.
            let inner_body = if would_be_nullable(&inner_sanitised, known) {
                wrap_in_variant(inner_sanitised)
            } else {
                inner_sanitised
            };
            SchemaType::option(inner_body)
        }
        SchemaType::Result { spec, .. } => {
            SchemaType::result(crate::schema::schema_type::ResultSpec {
                ok: spec.ok.as_ref().map(|t| Box::new(sanitise_type(t, known))),
                err: spec.err.as_ref().map(|t| Box::new(sanitise_type(t, known))),
            })
        }
        SchemaType::Quantity { spec, .. } => SchemaType::quantity(sanitise_quantity(spec)),
        SchemaType::Text { restrictions, .. } => SchemaType::text(sanitise_text(restrictions)),
        SchemaType::Binary { restrictions, .. } => {
            SchemaType::binary(sanitise_binary(restrictions))
        }
        SchemaType::Union { spec, .. } => SchemaType::union(sanitise_union(spec, known)),
        SchemaType::Future { inner, .. } => {
            SchemaType::future(inner.as_ref().map(|t| sanitise_type(t, known)))
        }
        SchemaType::Stream { inner, .. } => {
            SchemaType::stream(inner.as_ref().map(|t| sanitise_type(t, known)))
        }
        other => other.clone(),
    }
}

fn sanitise_quantity(spec: &QuantitySpec) -> QuantitySpec {
    // Force min/max to the spec's base unit, clamp to a safe scale range to
    // avoid pow10 overflow during the canonical comparison, and ensure
    // min <= max.
    let mut out = spec.clone();
    let clamp_scale = |s: i32| s.clamp(-9, 9);
    if let Some(min) = out.min.as_mut() {
        min.scale = clamp_scale(min.scale);
        min.unit = spec.base_unit.clone();
    }
    if let Some(max) = out.max.as_mut() {
        max.scale = clamp_scale(max.scale);
        max.unit = spec.base_unit.clone();
    }
    if let (Some(min), Some(max)) = (out.min.as_mut(), out.max.as_mut()) {
        let common = min.scale.max(max.scale);
        let min_canon = (min.mantissa as i128).saturating_mul(pow10((common - min.scale) as u32));
        let max_canon = (max.mantissa as i128).saturating_mul(pow10((common - max.scale) as u32));
        if min_canon > max_canon {
            std::mem::swap(min, max);
        }
    }
    out
}

fn pow10(shift: u32) -> i128 {
    let mut acc: i128 = 1;
    for _ in 0..shift {
        acc = acc.saturating_mul(10);
    }
    acc
}

fn sanitise_union(spec: &UnionSpec, known: &HashSet<TypeId>) -> UnionSpec {
    // Keep duplicate-free tags and force discriminator/body shape compatibility.
    let mut seen = HashSet::new();
    let mut branches: Vec<UnionBranch> = spec
        .branches
        .iter()
        .filter(|b| seen.insert(b.tag.clone()))
        .enumerate()
        .map(|(i, b)| sanitise_union_branch(i, b, known))
        .collect();
    if branches.is_empty() {
        branches.push(UnionBranch {
            tag: "default".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Prefix {
                prefix: "default".to_string(),
            },
            metadata: Default::default(),
        });
    }
    UnionSpec { branches }
}

fn sanitise_union_branch(
    index: usize,
    branch: &UnionBranch,
    known: &HashSet<TypeId>,
) -> UnionBranch {
    let sanitised_body = sanitise_type(&branch.body, known);
    // Force unique discriminators by stamping the branch index into the
    // string-shape rules / field names. Property tests should not have to
    // worry about discriminator overlap.
    let (body, discriminator) = match &branch.discriminator {
        DiscriminatorRule::Prefix { .. } => (
            SchemaType::string(),
            DiscriminatorRule::Prefix {
                prefix: format!("prefix-{index}-"),
            },
        ),
        DiscriminatorRule::Suffix { .. } => (
            SchemaType::string(),
            DiscriminatorRule::Suffix {
                suffix: format!("-{index}-suffix"),
            },
        ),
        DiscriminatorRule::Contains { .. } => (
            SchemaType::string(),
            DiscriminatorRule::Contains {
                substring: format!("contains-{index}"),
            },
        ),
        DiscriminatorRule::Regex { .. } => (
            SchemaType::string(),
            DiscriminatorRule::Regex {
                regex: format!("re-{index}-pattern"),
            },
        ),
        DiscriminatorRule::FieldEquals(disc) => {
            let field_name = format!("disc{index}");
            let body = SchemaType::record(vec![NamedFieldType {
                name: field_name.clone(),
                body: SchemaType::string(),
                metadata: Default::default(),
            }]);
            (
                body,
                DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name,
                    literal: disc.literal.clone(),
                }),
            )
        }
        DiscriminatorRule::FieldAbsent { .. } => {
            // Record-shape required; field-absent must reference a field not
            // present in the body, so use a unique sentinel name.
            let body = SchemaType::record(vec![]);
            (
                body,
                DiscriminatorRule::FieldAbsent {
                    field_name: format!("absent{index}"),
                },
            )
        }
    };
    let _ = sanitised_body;
    UnionBranch {
        tag: branch.tag.clone(),
        body,
        discriminator,
        metadata: branch.metadata.clone(),
    }
}

fn sanitise_text(restrictions: &TextRestrictions) -> TextRestrictions {
    let mut out = restrictions.clone();
    // Drop the random regex; the proptest strategy cannot guarantee
    // it parses.
    out.regex = None;
    // Ensure min <= max.
    if let (Some(min), Some(max)) = (out.min_length, out.max_length)
        && min > max
    {
        out.min_length = Some(max);
        out.max_length = Some(min);
    }
    out
}

fn sanitise_binary(restrictions: &BinaryRestrictions) -> BinaryRestrictions {
    let mut out = restrictions.clone();
    if let (Some(min), Some(max)) = (out.min_bytes, out.max_bytes)
        && min > max
    {
        out.min_bytes = Some(max);
        out.max_bytes = Some(min);
    }
    out
}

/// Local copy of the nullability predicate. The sanitiser does not yet have
/// a back-reference to a fully constructed graph, so we treat `Ref(id)` as
/// non-nullable here — the validator will catch any genuinely cyclic
/// nullable refs at the graph level.
fn would_be_nullable(ty: &SchemaType, _known: &HashSet<TypeId>) -> bool {
    match ty {
        SchemaType::Option { .. } => true,
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter()
            .any(|b| would_be_nullable(&b.body, _known)),
        _ => false,
    }
}

fn wrap_in_variant(inner: SchemaType) -> SchemaType {
    SchemaType::variant(vec![VariantCaseType {
        name: "value".to_string(),
        payload: Some(inner),
        metadata: Default::default(),
    }])
}
