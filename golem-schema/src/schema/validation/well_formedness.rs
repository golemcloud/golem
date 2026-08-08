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

//! Structural well-formedness checks for a [`SchemaGraph`].

use crate::schema::graph::{RefResolutionError, SchemaGraph};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NumericRestrictionError, PathSpec, QuantitySpec,
    QuantityValue, SchemaType, TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions,
};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// All structural errors that can be raised by [`validate_graph`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    DuplicateTypeId(TypeId),
    DanglingRef(TypeId),
    /// A named reference whose alias chain is a pure cycle
    /// (`A -> B -> ... -> A`) that never bottoms out in a concrete type, so it
    /// can never resolve to a usable `SchemaType`.
    RecursiveAlias(TypeId),
    EmptyVariant,
    EmptyEnum,
    EmptyUnion,
    EmptyFlags,
    DuplicateFieldName(String),
    DuplicateVariantCase(String),
    DuplicateEnumCase(String),
    DuplicateFlagName(String),
    DuplicateUnionTag(String),
    MapKeyNotPrimitive,
    FixedListZeroLength,
    QuantityMinGreaterThanMax,
    QuantityMinUnitMismatch {
        base_unit: String,
        min_unit: String,
    },
    QuantityMaxUnitMismatch {
        base_unit: String,
        max_unit: String,
    },
    QuantityComparisonOverflow {
        base_unit: String,
    },
    UnionStringRuleOnNonStringBody {
        tag: String,
    },
    UnionFieldRuleOnNonRecordBody {
        tag: String,
    },
    UnionFieldEqualsLiteralOnNonStringField {
        tag: String,
        field_name: String,
    },
    UnionFieldRuleMissingField {
        tag: String,
        field_name: String,
    },
    UnionAmbiguousDiscriminators {
        tag_a: String,
        tag_b: String,
        reason: String,
    },
    UnionUnsatisfiableFieldAbsent {
        tag: String,
        field_name: String,
    },
    InvalidRegex {
        tag: String,
        regex: String,
        message: String,
    },
    InvalidTextRegex {
        regex: String,
        message: String,
    },
    TextLengthRangeInverted,
    BinaryByteRangeInverted,
    /// A numeric type's inline restrictions are not well-formed.
    InvalidNumericRestriction {
        error: NumericRestrictionError,
    },
    /// An `Option<X>` was declared where `X` is itself nullable on the
    /// canonical JSON wire (option-of-option, option-of-union-with-nullable-
    /// branch, option-of-ref-resolving-to-nullable). The canonical JSON
    /// encoding `null | inner` collapses `Some(None)` and `None`, so the
    /// nesting is rejected at construction time.
    NullableNesting {
        inner: String,
    },
}

impl Display for SchemaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::DuplicateTypeId(id) => write!(f, "duplicate type id `{id}`"),
            SchemaError::DanglingRef(id) => write!(f, "dangling type reference `{id}`"),
            SchemaError::RecursiveAlias(id) => {
                write!(
                    f,
                    "type reference `{id}` forms a reference cycle with no concrete type"
                )
            }
            SchemaError::EmptyVariant => write!(f, "variant has no cases"),
            SchemaError::EmptyEnum => write!(f, "enum has no cases"),
            SchemaError::EmptyUnion => write!(f, "union has no branches"),
            SchemaError::EmptyFlags => write!(f, "flags has no entries"),
            SchemaError::DuplicateFieldName(name) => write!(f, "duplicate field `{name}`"),
            SchemaError::DuplicateVariantCase(name) => {
                write!(f, "duplicate variant case `{name}`")
            }
            SchemaError::DuplicateEnumCase(name) => write!(f, "duplicate enum case `{name}`"),
            SchemaError::DuplicateFlagName(name) => write!(f, "duplicate flag `{name}`"),
            SchemaError::DuplicateUnionTag(tag) => write!(f, "duplicate union branch tag `{tag}`"),
            SchemaError::MapKeyNotPrimitive => {
                write!(f, "map key must be a primitive type")
            }
            SchemaError::FixedListZeroLength => write!(f, "fixed-list length must be > 0"),
            SchemaError::QuantityMinGreaterThanMax => {
                write!(f, "quantity min is greater than max")
            }
            SchemaError::QuantityMinUnitMismatch {
                base_unit,
                min_unit,
            } => write!(
                f,
                "quantity min unit mismatch: base `{base_unit}`, min `{min_unit}`"
            ),
            SchemaError::QuantityMaxUnitMismatch {
                base_unit,
                max_unit,
            } => write!(
                f,
                "quantity max unit mismatch: base `{base_unit}`, max `{max_unit}`"
            ),
            SchemaError::QuantityComparisonOverflow { base_unit } => write!(
                f,
                "quantity range comparison overflowed in base unit `{base_unit}`"
            ),
            SchemaError::UnionStringRuleOnNonStringBody { tag } => write!(
                f,
                "union branch `{tag}` uses a string-pattern rule but body is not string-shaped"
            ),
            SchemaError::UnionFieldRuleOnNonRecordBody { tag } => write!(
                f,
                "union branch `{tag}` uses a field rule but body is not record-shaped"
            ),
            SchemaError::UnionFieldEqualsLiteralOnNonStringField { tag, field_name } => write!(
                f,
                "union branch `{tag}` references field `{field_name}` for a literal comparison \
                 but the field is not string-shaped"
            ),
            SchemaError::UnionFieldRuleMissingField { tag, field_name } => write!(
                f,
                "union branch `{tag}` references record field `{field_name}` that does not exist"
            ),
            SchemaError::UnionAmbiguousDiscriminators {
                tag_a,
                tag_b,
                reason,
            } => write!(
                f,
                "union branches `{tag_a}` and `{tag_b}` have overlapping discriminators ({reason})"
            ),
            SchemaError::UnionUnsatisfiableFieldAbsent { tag, field_name } => write!(
                f,
                "union branch `{tag}` uses field-absent on `{field_name}` but the record body \
                 declares that field"
            ),
            SchemaError::InvalidRegex {
                tag,
                regex,
                message,
            } => write!(
                f,
                "union branch `{tag}` regex `{regex}` failed to compile: {message}"
            ),
            SchemaError::InvalidTextRegex { regex, message } => {
                write!(f, "text regex `{regex}` failed to compile: {message}")
            }
            SchemaError::TextLengthRangeInverted => {
                write!(f, "text min-length is greater than max-length")
            }
            SchemaError::InvalidNumericRestriction { error } => {
                write!(f, "invalid numeric restriction: {error}")
            }
            SchemaError::BinaryByteRangeInverted => {
                write!(f, "binary min-bytes is greater than max-bytes")
            }
            SchemaError::NullableNesting { inner } => write!(
                f,
                "option<{inner}> is invalid because the inner type is also nullable; \
                 use a variant with explicit cases to distinguish absence from explicit none"
            ),
        }
    }
}

impl Error for SchemaError {}

/// Validate a [`SchemaGraph`] for structural well-formedness.
///
/// Returns the full list of collected errors. Ordering is deterministic:
/// errors are reported in the order they are discovered while walking the
/// graph.
pub fn validate_graph(graph: &SchemaGraph) -> Result<(), Vec<SchemaError>> {
    let mut errors = Vec::new();

    let mut seen_ids: HashSet<&TypeId> = HashSet::new();
    for def in &graph.defs {
        if !seen_ids.insert(&def.id) {
            errors.push(SchemaError::DuplicateTypeId(def.id.clone()));
        }
    }

    let known_ids: HashSet<TypeId> = graph.defs.iter().map(|d| d.id.clone()).collect();

    for def in &graph.defs {
        check_type(graph, &def.body, &known_ids, &mut errors);
    }
    check_type(graph, &graph.root, &known_ids, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate a single [`SchemaType`] as a root against an existing graph's
/// definitions for structural well-formedness.
///
/// Unlike [`validate_graph`], this does not validate the graph's own
/// [`SchemaGraph::root`] or its definition bodies; only `ty` is walked, with
/// [`SchemaType::Ref`]s resolved against `graph.defs`. This is for callers (such
/// as the tool validator) that embed many bare `SchemaType` roots which share a
/// single definitions carrier whose own `root` is an unused placeholder.
///
/// Errors are returned in deterministic discovery order.
pub fn validate_root_type(graph: &SchemaGraph, ty: &SchemaType) -> Result<(), Vec<SchemaError>> {
    let known_ids: HashSet<TypeId> = graph.defs.iter().map(|d| d.id.clone()).collect();
    let mut errors = Vec::new();
    check_type(graph, ty, &known_ids, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_type(
    graph: &SchemaGraph,
    ty: &SchemaType,
    known: &HashSet<TypeId>,
    errors: &mut Vec<SchemaError>,
) {
    match ty {
        SchemaType::Ref { id, .. } => {
            if !known.contains(id) {
                errors.push(SchemaError::DanglingRef(id.clone()));
            } else {
                match graph.resolve_ref(ty) {
                    Ok(_) => {}
                    // The id exists, but its alias chain is a pure cycle that
                    // never resolves to a concrete type. A legitimate recursive
                    // type (whose cycle passes through a value-shrinking
                    // constructor such as `list`/`record`) resolves to that
                    // constructor at the top-level alias step and is unaffected.
                    Err(RefResolutionError::RecursiveRef(cycle_id)) => {
                        errors.push(SchemaError::RecursiveAlias(cycle_id));
                    }
                    // The id exists, but following its alias chain reaches a
                    // reference to a definition that is absent from the graph.
                    // Report the dangling target so a root alias chain ending in
                    // a missing definition is rejected at the chain's tail.
                    Err(RefResolutionError::DanglingRef(target)) => {
                        errors.push(SchemaError::DanglingRef(target));
                    }
                }
            }
        }

        SchemaType::Record { fields, .. } => {
            let mut seen: HashSet<&str> = HashSet::new();
            for field in fields {
                if !seen.insert(field.name.as_str()) {
                    errors.push(SchemaError::DuplicateFieldName(field.name.clone()));
                }
                check_type(graph, &field.body, known, errors);
            }
        }

        SchemaType::Variant { cases, .. } => {
            if cases.is_empty() {
                errors.push(SchemaError::EmptyVariant);
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for case in cases {
                if !seen.insert(case.name.as_str()) {
                    errors.push(SchemaError::DuplicateVariantCase(case.name.clone()));
                }
                if let Some(p) = &case.payload {
                    check_type(graph, p, known, errors);
                }
            }
        }

        SchemaType::Enum { cases, .. } => {
            if cases.is_empty() {
                errors.push(SchemaError::EmptyEnum);
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for case in cases {
                if !seen.insert(case.as_str()) {
                    errors.push(SchemaError::DuplicateEnumCase(case.clone()));
                }
            }
        }

        SchemaType::Flags { flags, .. } => {
            if flags.is_empty() {
                errors.push(SchemaError::EmptyFlags);
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for flag in flags {
                if !seen.insert(flag.as_str()) {
                    errors.push(SchemaError::DuplicateFlagName(flag.clone()));
                }
            }
        }

        SchemaType::Tuple { elements, .. } => {
            for e in elements {
                check_type(graph, e, known, errors);
            }
        }
        SchemaType::List { element, .. } => check_type(graph, element, known, errors),
        SchemaType::FixedList {
            element, length, ..
        } => {
            if *length == 0 {
                errors.push(SchemaError::FixedListZeroLength);
            }
            check_type(graph, element, known, errors);
        }
        SchemaType::Map { key, value, .. } => {
            // A key that resolves to a concrete non-primitive type is rejected.
            // A key that does not resolve to a concrete type (a dangling
            // reference or a pure alias cycle) is *not* reported here: that
            // failure is reported by the `check_type(key, ...)` recursion below,
            // and a `MapKeyNotPrimitive` on top of it would be misleading cascade
            // noise.
            if let MapKeyKind::NonPrimitive = classify_map_key(graph, key) {
                errors.push(SchemaError::MapKeyNotPrimitive);
            }
            check_type(graph, key, known, errors);
            check_type(graph, value, known, errors);
        }
        SchemaType::Option { inner, .. } => {
            if is_nullable(graph, inner, &mut HashSet::new()) {
                errors.push(SchemaError::NullableNesting {
                    inner: describe_nullable(inner),
                });
            }
            check_type(graph, inner, known, errors);
        }
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &spec.ok {
                check_type(graph, t, known, errors);
            }
            if let Some(t) = &spec.err {
                check_type(graph, t, known, errors);
            }
        }

        SchemaType::Quantity { spec, .. } => check_quantity(spec, errors),

        SchemaType::Text { restrictions, .. } => check_text_restrictions(restrictions, errors),
        SchemaType::Binary { restrictions, .. } => check_binary_restrictions(restrictions, errors),
        SchemaType::Path { spec, .. } => check_path_spec(spec, errors),
        SchemaType::Url { restrictions, .. } => check_url_spec(restrictions, errors),

        SchemaType::Union { spec, .. } => {
            validate_union(graph, spec, known, errors);
        }

        SchemaType::Future { inner, .. } => {
            if let Some(t) = inner {
                check_type(graph, t, known, errors);
            }
        }
        SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                check_type(graph, t, known, errors);
            }
        }

        SchemaType::S8 { restrictions, .. }
        | SchemaType::S16 { restrictions, .. }
        | SchemaType::S32 { restrictions, .. }
        | SchemaType::S64 { restrictions, .. }
        | SchemaType::U8 { restrictions, .. }
        | SchemaType::U16 { restrictions, .. }
        | SchemaType::U32 { restrictions, .. }
        | SchemaType::U64 { restrictions, .. }
        | SchemaType::F32 { restrictions, .. }
        | SchemaType::F64 { restrictions, .. } => {
            if let Some(restrictions) = restrictions {
                let repr = ty.numeric_repr().expect("numeric variant => numeric repr");
                if let Err(error) = restrictions.validate_for_repr(repr) {
                    errors.push(SchemaError::InvalidNumericRestriction { error });
                }
            }
        }

        SchemaType::Secret { spec, .. } => {
            check_type(graph, &spec.inner, known, errors);
        }

        SchemaType::Bool { .. }
        | SchemaType::Char { .. }
        | SchemaType::String { .. }
        | SchemaType::Datetime { .. }
        | SchemaType::Duration { .. }
        | SchemaType::QuotaToken { .. }
        | SchemaType::PermissionCard { .. } => {}
    }
}

/// Classification of a map key type after resolving named references (with
/// cycle detection).
enum MapKeyKind {
    /// The key resolves to a primitive type and is valid.
    Primitive,
    /// The key resolves to a concrete non-primitive type (or a reference
    /// cycle, which can never be primitive) and is invalid.
    NonPrimitive,
    /// The key is (or resolves through) a dangling reference, so its
    /// primitiveness cannot be determined; the dangling reference is reported
    /// separately.
    Unresolved,
}

fn classify_map_key(graph: &SchemaGraph, ty: &SchemaType) -> MapKeyKind {
    let mut visited: HashSet<TypeId> = HashSet::new();
    let mut current = ty;
    loop {
        match current {
            SchemaType::Ref { id, .. } => {
                if !visited.insert(id.clone()) {
                    // A pure alias cycle never resolves to a concrete type, so
                    // its primitiveness is unknown. It is reported as a
                    // `RecursiveAlias` by the `check_type(key, ...)` recursion; a
                    // `MapKeyNotPrimitive` on top of that would be misleading
                    // cascade noise.
                    return MapKeyKind::Unresolved;
                }
                match graph.lookup(id) {
                    Some(def) => current = &def.body,
                    None => return MapKeyKind::Unresolved,
                }
            }
            other => {
                return if is_primitive_key(other) {
                    MapKeyKind::Primitive
                } else {
                    MapKeyKind::NonPrimitive
                };
            }
        }
    }
}

fn is_primitive_key(ty: &SchemaType) -> bool {
    matches!(
        ty,
        SchemaType::Bool { .. }
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
    )
}

fn check_quantity(spec: &QuantitySpec, errors: &mut Vec<SchemaError>) {
    if let Some(min) = &spec.min
        && min.unit != spec.base_unit
    {
        errors.push(SchemaError::QuantityMinUnitMismatch {
            base_unit: spec.base_unit.clone(),
            min_unit: min.unit.clone(),
        });
    }
    if let Some(max) = &spec.max
        && max.unit != spec.base_unit
    {
        errors.push(SchemaError::QuantityMaxUnitMismatch {
            base_unit: spec.base_unit.clone(),
            max_unit: max.unit.clone(),
        });
    }

    if let (Some(min), Some(max)) = (&spec.min, &spec.max)
        && min.unit == spec.base_unit
        && max.unit == spec.base_unit
    {
        match quantity_le(min, max) {
            Some(true) => {}
            Some(false) => errors.push(SchemaError::QuantityMinGreaterThanMax),
            None => errors.push(SchemaError::QuantityComparisonOverflow {
                base_unit: spec.base_unit.clone(),
            }),
        }
    }
}

/// Compare two [`QuantityValue`]s with the same unit, treating each as
/// `mantissa * 10^(-scale)`. Returns `Some(true)` iff `a <= b`, `Some(false)`
/// iff `a > b`, and `None` if rescaling overflows.
fn quantity_le(a: &QuantityValue, b: &QuantityValue) -> Option<bool> {
    let common = a.scale.max(b.scale);
    let a_shift = (common - a.scale).max(0) as u32;
    let b_shift = (common - b.scale).max(0) as u32;

    let ten: i128 = 10;
    let a_factor = ten.checked_pow(a_shift)?;
    let b_factor = ten.checked_pow(b_shift)?;
    let a_canon = (a.mantissa as i128).checked_mul(a_factor)?;
    let b_canon = (b.mantissa as i128).checked_mul(b_factor)?;
    Some(a_canon <= b_canon)
}

fn check_text_restrictions(restrictions: &TextRestrictions, errors: &mut Vec<SchemaError>) {
    if let (Some(min), Some(max)) = (restrictions.min_length, restrictions.max_length)
        && min > max
    {
        errors.push(SchemaError::TextLengthRangeInverted);
    }
    if let Some(regex) = &restrictions.regex
        && let Err(e) = regex::Regex::new(regex.as_str())
    {
        errors.push(SchemaError::InvalidTextRegex {
            regex: regex.clone(),
            message: e.to_string(),
        });
    }
}

fn check_binary_restrictions(restrictions: &BinaryRestrictions, errors: &mut Vec<SchemaError>) {
    if let (Some(min), Some(max)) = (restrictions.min_bytes, restrictions.max_bytes)
        && min > max
    {
        errors.push(SchemaError::BinaryByteRangeInverted);
    }
}

fn check_path_spec(_spec: &PathSpec, _errors: &mut Vec<SchemaError>) {
    // PathSpec has no regex today; nothing to validate beyond structural
    // shape.
}

fn check_url_spec(_spec: &UrlRestrictions, _errors: &mut Vec<SchemaError>) {
    // UrlRestrictions has no regex today; nothing to validate beyond
    // structural shape.
}

fn validate_union(
    graph: &SchemaGraph,
    spec: &UnionSpec,
    known: &HashSet<TypeId>,
    errors: &mut Vec<SchemaError>,
) {
    if spec.branches.is_empty() {
        errors.push(SchemaError::EmptyUnion);
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for branch in &spec.branches {
        if !seen.insert(branch.tag.as_str()) {
            errors.push(SchemaError::DuplicateUnionTag(branch.tag.clone()));
        }
        check_union_branch(graph, branch, errors);
        check_type(graph, &branch.body, known, errors);
    }

    // Discriminator ambiguity check.
    for i in 0..spec.branches.len() {
        for j in (i + 1)..spec.branches.len() {
            let a = &spec.branches[i];
            let b = &spec.branches[j];
            if let Some(reason) = discriminators_overlap(&a.discriminator, &b.discriminator) {
                errors.push(SchemaError::UnionAmbiguousDiscriminators {
                    tag_a: a.tag.clone(),
                    tag_b: b.tag.clone(),
                    reason,
                });
            }
        }
    }
}

fn check_union_branch(graph: &SchemaGraph, branch: &UnionBranch, errors: &mut Vec<SchemaError>) {
    let shape = resolved_shape(graph, &branch.body, &mut HashSet::new());
    // The branch body is a dangling/recursive ref: its shape is unknown, so any
    // shape-vs-discriminator mismatch would be misleading noise on top of the
    // unresolved-reference error `check_type` already reports. Body-shape-
    // independent problems (an invalid regex) are still checked below.
    let shape_known = !matches!(shape, BodyShape::Unresolved);
    match &branch.discriminator {
        DiscriminatorRule::Prefix { .. }
        | DiscriminatorRule::Suffix { .. }
        | DiscriminatorRule::Contains { .. } => {
            if shape_known && !matches!(shape, BodyShape::String) {
                errors.push(SchemaError::UnionStringRuleOnNonStringBody {
                    tag: branch.tag.clone(),
                });
            }
        }
        DiscriminatorRule::Regex { regex } => {
            if shape_known && !matches!(shape, BodyShape::String) {
                errors.push(SchemaError::UnionStringRuleOnNonStringBody {
                    tag: branch.tag.clone(),
                });
            }
            if regex.is_empty() {
                errors.push(SchemaError::InvalidRegex {
                    tag: branch.tag.clone(),
                    regex: regex.clone(),
                    message: "regex must be non-empty".to_string(),
                });
            } else if let Err(e) = regex::Regex::new(regex.as_str()) {
                errors.push(SchemaError::InvalidRegex {
                    tag: branch.tag.clone(),
                    regex: regex.clone(),
                    message: e.to_string(),
                });
            }
        }
        DiscriminatorRule::FieldEquals(field_disc) => match shape {
            BodyShape::Record(fields) => {
                match fields.iter().find(|(n, _)| n == &field_disc.field_name) {
                    None => errors.push(SchemaError::UnionFieldRuleMissingField {
                        tag: branch.tag.clone(),
                        field_name: field_disc.field_name.clone(),
                    }),
                    Some((_, ty)) => {
                        let field_shape = resolved_shape(graph, ty, &mut HashSet::new());
                        if field_disc.literal.is_some()
                            && !matches!(field_shape, BodyShape::String | BodyShape::Unresolved)
                        {
                            errors.push(SchemaError::UnionFieldEqualsLiteralOnNonStringField {
                                tag: branch.tag.clone(),
                                field_name: field_disc.field_name.clone(),
                            });
                        }
                    }
                }
            }
            BodyShape::Unresolved => {}
            _ => errors.push(SchemaError::UnionFieldRuleOnNonRecordBody {
                tag: branch.tag.clone(),
            }),
        },
        DiscriminatorRule::FieldAbsent { field_name } => match shape {
            BodyShape::Record(fields) => {
                if fields.iter().any(|(n, _)| n == field_name) {
                    errors.push(SchemaError::UnionUnsatisfiableFieldAbsent {
                        tag: branch.tag.clone(),
                        field_name: field_name.clone(),
                    });
                }
            }
            BodyShape::Unresolved => {}
            _ => errors.push(SchemaError::UnionFieldRuleOnNonRecordBody {
                tag: branch.tag.clone(),
            }),
        },
    }
}

/// Returns `Some(reason)` if rules `a` and `b` can structurally overlap on
/// at least one value, `None` otherwise. Regex overlap is undecidable in
/// general; only byte-equal patterns are reported.
fn discriminators_overlap(a: &DiscriminatorRule, b: &DiscriminatorRule) -> Option<String> {
    match (a, b) {
        (DiscriminatorRule::Prefix { prefix: pa }, DiscriminatorRule::Prefix { prefix: pb }) => {
            if pa.is_empty() && pb.is_empty() {
                return Some("both prefixes are empty".to_string());
            }
            if pa.is_empty() {
                return Some(format!("empty prefix overlaps any other prefix `{pb}`"));
            }
            if pb.is_empty() {
                return Some(format!("empty prefix overlaps any other prefix `{pa}`"));
            }
            if pa.starts_with(pb.as_str()) || pb.starts_with(pa.as_str()) {
                return Some(format!("prefix `{pa}` and prefix `{pb}` overlap"));
            }
            None
        }
        (DiscriminatorRule::Suffix { suffix: sa }, DiscriminatorRule::Suffix { suffix: sb }) => {
            if sa.is_empty() && sb.is_empty() {
                return Some("both suffixes are empty".to_string());
            }
            if sa.is_empty() {
                return Some(format!("empty suffix overlaps any other suffix `{sb}`"));
            }
            if sb.is_empty() {
                return Some(format!("empty suffix overlaps any other suffix `{sa}`"));
            }
            if sa.ends_with(sb.as_str()) || sb.ends_with(sa.as_str()) {
                return Some(format!("suffix `{sa}` and suffix `{sb}` overlap"));
            }
            None
        }
        (
            DiscriminatorRule::Contains { substring: ca },
            DiscriminatorRule::Contains { substring: cb },
        ) => {
            if ca.is_empty() || cb.is_empty() {
                return Some("empty contains substring matches every string".to_string());
            }
            None
        }
        (DiscriminatorRule::Prefix { prefix }, _) | (_, DiscriminatorRule::Prefix { prefix })
            if prefix.is_empty() =>
        {
            Some("empty prefix matches every string".to_string())
        }
        (DiscriminatorRule::Suffix { suffix }, _) | (_, DiscriminatorRule::Suffix { suffix })
            if suffix.is_empty() =>
        {
            Some("empty suffix matches every string".to_string())
        }
        (DiscriminatorRule::Contains { substring }, _)
        | (_, DiscriminatorRule::Contains { substring })
            if substring.is_empty() =>
        {
            Some("empty contains substring matches every string".to_string())
        }
        (DiscriminatorRule::Regex { regex: ra }, DiscriminatorRule::Regex { regex: rb }) => {
            if ra == rb {
                Some(format!("both branches share regex `{ra}`"))
            } else {
                None
            }
        }
        (DiscriminatorRule::FieldEquals(fa), DiscriminatorRule::FieldEquals(fb)) => {
            if fa.field_name != fb.field_name {
                return None;
            }
            match (&fa.literal, &fb.literal) {
                (None, _) | (_, None) => Some(format!(
                    "field-equals on `{}` without literal overlaps another field-equals on the \
                     same field",
                    fa.field_name
                )),
                (Some(la), Some(lb)) if la == lb => Some(format!(
                    "two field-equals on `{}` share literal `{la}`",
                    fa.field_name
                )),
                _ => None,
            }
        }
        (
            DiscriminatorRule::FieldAbsent { field_name: fa },
            DiscriminatorRule::FieldAbsent { field_name: fb },
        ) => {
            if fa == fb {
                Some(format!("two field-absent rules share field `{fa}`"))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether `ty` can be encoded as JSON `null` on the canonical wire.
///
/// True when `ty` is an `Option<_>`, a `Union` whose any branch body is
/// nullable, or a `Ref` resolving (after cycle-aware traversal) to a
/// nullable type. The traversal terminates on cycles by treating any
/// re-entered [`TypeId`] as non-nullable.
fn is_nullable(graph: &SchemaGraph, ty: &SchemaType, visited: &mut HashSet<TypeId>) -> bool {
    match ty {
        SchemaType::Option { .. } => true,
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter()
            .any(|b| is_nullable(graph, &b.body, visited)),
        SchemaType::Ref { id, .. } => {
            if !visited.insert(id.clone()) {
                return false;
            }
            let result = match graph.lookup(id) {
                Some(def) => is_nullable(graph, &def.body, visited),
                None => false,
            };
            visited.remove(id);
            result
        }
        _ => false,
    }
}

/// Short textual description of a nullable type used in the
/// [`SchemaError::NullableNesting`] message.
fn describe_nullable(ty: &SchemaType) -> String {
    match ty {
        SchemaType::Option { .. } => "option<_>".to_string(),
        SchemaType::Union { .. } => "union".to_string(),
        SchemaType::Ref { id, .. } => format!("ref `{id}`"),
        _ => "nullable".to_string(),
    }
}

#[derive(Clone, Debug)]
enum BodyShape<'a> {
    String,
    Record(Vec<(String, &'a SchemaType)>),
    Other,
    /// The body is a [`SchemaType::Ref`] that does not resolve to a concrete
    /// type (a dangling reference or a pure alias cycle). The shape is unknown,
    /// so shape-dependent discriminator checks are skipped; the unresolved
    /// reference itself is reported separately by [`check_type`].
    Unresolved,
}

fn resolved_shape<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
    visited: &mut HashSet<TypeId>,
) -> BodyShape<'a> {
    match ty {
        SchemaType::Ref { id, .. } => {
            if !visited.insert(id.clone()) {
                return BodyShape::Unresolved;
            }
            match graph.lookup(id) {
                Some(def) => resolved_shape(graph, &def.body, visited),
                None => BodyShape::Unresolved,
            }
        }
        SchemaType::String { .. }
        | SchemaType::Text { .. }
        | SchemaType::Url { .. }
        | SchemaType::Path { .. } => BodyShape::String,
        SchemaType::Record { fields, .. } => BodyShape::Record(
            fields
                .iter()
                .map(|f| (f.name.clone(), &f.body))
                .collect::<Vec<_>>(),
        ),
        _ => BodyShape::Other,
    }
}
