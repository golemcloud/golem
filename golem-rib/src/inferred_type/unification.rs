use crate::inferred_type::{flatten_all_of_list, TypeOrigin};
use crate::{InferredType, TypeInternal};
use std::fmt::{Display, Formatter};
use std::ops::Deref;

pub fn try_unify_type(
    inferred_type: &InferredType,
) -> Result<InferredType, UnificationFailureInternal> {
    match inferred_type.inner.deref() {
        TypeInternal::AllOf(types) => {
            let flattened_all_ofs = flatten_all_of_list(types);
            unify_all_inferred_types(&flattened_all_ofs)
        }

        TypeInternal::Option(inner_type) => {
            let unified_inner_type = try_unify_type(inner_type)?;
            Ok(InferredType::option(unified_inner_type))
        }

        TypeInternal::Result { ok, error } => {
            let unified_ok = match ok {
                Some(ok) => Some(try_unify_type(ok)),
                None => None,
            };

            let unified_error = match error {
                Some(error) => Some(try_unify_type(error)),
                None => None,
            };

            handle_result(unified_ok, unified_error)
        }

        TypeInternal::Record(fields) => {
            let mut unified_fields = vec![];
            for (field, typ) in fields {
                let unified_type = try_unify_type(typ)?;
                unified_fields.push((field.clone(), unified_type));
            }
            Ok(InferredType::resolved(TypeInternal::Record(unified_fields)))
        }

        TypeInternal::Tuple(types) => {
            let mut unified_types = vec![];
            for typ in types {
                let unified_type = try_unify_type(typ)?;
                unified_types.push(unified_type);
            }
            Ok(InferredType::resolved(TypeInternal::Tuple(unified_types)))
        }

        TypeInternal::List(typ) => {
            let unified_type = try_unify_type(typ)?;
            Ok(InferredType::resolved(TypeInternal::List(unified_type)))
        }

        TypeInternal::Range {
            from: start,
            to: end,
        } => {
            let unified_start = try_unify_type(start)?;
            let unified_end = end.as_ref().map(|end| try_unify_type(end)).transpose()?;
            Ok(InferredType::resolved(TypeInternal::Range {
                from: unified_start,
                to: unified_end,
            }))
        }

        TypeInternal::Flags(flags) => {
            Ok(InferredType::resolved(TypeInternal::Flags(flags.clone())))
        }

        TypeInternal::Enum(variants) => {
            Ok(InferredType::resolved(TypeInternal::Enum(variants.clone())))
        }

        TypeInternal::Variant(variants) => {
            let mut unified_variants = vec![];
            for (variant, typ) in variants {
                let unified_type = match typ {
                    Some(typ) => Some(Box::new(try_unify_type(typ)?)),
                    None => None,
                };
                unified_variants.push((variant.clone(), unified_type.as_deref().cloned()));
            }
            Ok(InferredType::resolved(TypeInternal::Variant(
                unified_variants,
            )))
        }

        TypeInternal::Resource {
            resource_id,
            resource_mode,
        } => Ok(InferredType::resolved(TypeInternal::Resource {
            resource_id: *resource_id,
            resource_mode: *resource_mode,
        })),

        TypeInternal::Unknown => Ok(inferred_type.clone()),
        TypeInternal::Bool => Ok(inferred_type.clone()),
        TypeInternal::S8 => Ok(inferred_type.clone()),
        TypeInternal::U8 => Ok(inferred_type.clone()),
        TypeInternal::S16 => Ok(inferred_type.clone()),
        TypeInternal::U16 => Ok(inferred_type.clone()),
        TypeInternal::S32 => Ok(inferred_type.clone()),
        TypeInternal::U32 => Ok(inferred_type.clone()),
        TypeInternal::S64 => Ok(inferred_type.clone()),
        TypeInternal::U64 => Ok(inferred_type.clone()),
        TypeInternal::F32 => Ok(inferred_type.clone()),
        TypeInternal::F64 => Ok(inferred_type.clone()),
        TypeInternal::Chr => Ok(inferred_type.clone()),
        TypeInternal::Str => Ok(inferred_type.clone()),
        TypeInternal::Instance { .. } => Ok(inferred_type.clone()),
        TypeInternal::Sequence(_) => Ok(inferred_type.clone()),
    }
}
// An internal error that has partial information of the errors
pub enum UnificationFailureInternal {
    TypeMisMatch {
        expected: InferredType,
        found: InferredType,
        additional_error_detail: Vec<String>,
    },

    ConflictingTypes {
        conflicting_types: Vec<InferredType>,
        additional_error_detail: Vec<String>,
    },

    UnknownType,
}

impl Display for UnificationFailureInternal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UnificationFailureInternal::TypeMisMatch {
                expected,
                found,
                additional_error_detail,
            } => {
                let additional_error_details = additional_error_detail
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>();
                write!(
                    f,
                    "type mismatch: expected {}, found {}. {}",
                    expected.printable(),
                    found.printable(),
                    additional_error_details.join(", ")
                )
            }
            UnificationFailureInternal::ConflictingTypes {
                conflicting_types,
                additional_error_detail,
            } => {
                let conflicting_types = conflicting_types
                    .iter()
                    .map(|x| x.printable())
                    .collect::<Vec<_>>();
                let additional_error_details = additional_error_detail
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>();
                write!(
                    f,
                    "conflicting types: {}. {}",
                    conflicting_types.join(", "),
                    additional_error_details.join(", ")
                )
            }
            UnificationFailureInternal::UnknownType => {
                write!(f, "cannot determine type")
            }
        }
    }
}

impl UnificationFailureInternal {
    pub fn type_mismatch(
        expected: InferredType,
        found: InferredType,
        additional_error_detail: Vec<String>,
    ) -> Self {
        UnificationFailureInternal::TypeMisMatch {
            expected,
            found,
            additional_error_detail,
        }
    }

    pub fn conflicting_types(
        conflicting_types: Vec<InferredType>,
        additional_error_detail: Vec<String>,
    ) -> Self {
        UnificationFailureInternal::ConflictingTypes {
            conflicting_types,
            additional_error_detail,
        }
    }
}

pub fn unify_all_inferred_types(
    types: &Vec<InferredType>,
) -> Result<InferredType, UnificationFailureInternal> {
    let mut final_unified = InferredType::unknown();
    for typ in types {
        let next_unified = try_unify_type(typ).unwrap_or(typ.clone());
        final_unified = unify_both_inferred_types(&final_unified, &next_unified)?;
    }

    Ok(final_unified)
}

pub fn unify_both_inferred_types(
    left_inferred_type: &InferredType,
    right_inferred_type: &InferredType,
) -> Result<InferredType, UnificationFailureInternal> {
    // either of them unknown
    if right_inferred_type.is_unknown() {
        return Ok(left_inferred_type.clone());
    }

    if left_inferred_type.is_unknown() {
        return Ok(right_inferred_type.clone());
    }

    if left_inferred_type == right_inferred_type {
        return Ok(left_inferred_type.clone());
    }

    let left_inferred_type = try_unify_type(left_inferred_type)?;
    let right_inferred_type = try_unify_type(right_inferred_type)?;

    let inferred_type_printable = left_inferred_type.printable();
    let other_printable = right_inferred_type.printable();

    match (
        left_inferred_type.inner.deref(),
        right_inferred_type.inner.deref(),
    ) {
        (TypeInternal::Record(a_fields), TypeInternal::Record(b_fields)) => {
            let mut fields: Vec<(String, InferredType)> = vec![];
            for (a_name, a_type) in a_fields {
                if let Some((_, b_type)) = b_fields.iter().find(|(b_name, _)| b_name == a_name) {
                    fields.push((a_name.clone(), a_type.unify_with(b_type)?));
                } else {
                    fields.push((a_name.clone(), a_type.clone()));
                }
            }

            for (a_name, a_type) in b_fields {
                if !a_fields.iter().any(|(b_name, _)| b_name == a_name) {
                    fields.push((a_name.clone(), a_type.clone()));
                }
            }

            Ok(InferredType::resolved(TypeInternal::Record(fields)))
        }
        (TypeInternal::Tuple(a_types), TypeInternal::Tuple(b_types)) => {
            if a_types.len() != b_types.len() {
                return Err(UnificationFailureInternal::conflicting_types(
                    vec![left_inferred_type.clone(), right_inferred_type.clone()],
                    vec![format!(
                        "tuple length don't match. {}, {}",
                        a_types.len(),
                        b_types.len()
                    )],
                ));
            }
            let mut types = Vec::new();
            for (a_type, b_type) in a_types.iter().zip(b_types) {
                types.push(a_type.unify_with(b_type)?);
            }
            Ok(InferredType::resolved(TypeInternal::Tuple(types)))
        }
        (TypeInternal::List(a_type), TypeInternal::List(b_type)) => Ok(InferredType::resolved(
            TypeInternal::List(a_type.unify_with(b_type)?),
        )),
        (TypeInternal::Flags(a_flags), TypeInternal::Flags(b_flags)) => {
            if a_flags.len() >= b_flags.len() {
                if b_flags.iter().all(|b| a_flags.contains(b)) {
                    Ok(InferredType::resolved(TypeInternal::Flags(a_flags.clone())))
                } else {
                    Err(UnificationFailureInternal::conflicting_types(
                        vec![left_inferred_type.clone(), right_inferred_type.clone()],
                        vec![format!(
                            "conflicting flag types inferred. {}, {}",
                            inferred_type_printable, other_printable
                        )],
                    ))
                }
            } else if a_flags.iter().all(|a| b_flags.contains(a)) {
                Ok(InferredType::resolved(TypeInternal::Flags(b_flags.clone())))
            } else {
                Err(UnificationFailureInternal::conflicting_types(
                    vec![left_inferred_type.clone(), right_inferred_type.clone()],
                    vec![format!(
                        "conflicting flag types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    )],
                ))
            }
        }
        (
            TypeInternal::Range {
                from: a_start,
                to: a_end,
            },
            TypeInternal::Range {
                from: b_start,
                to: b_end,
            },
        ) => {
            let unified_start = a_start.unify_with(b_start)?;
            let unified_end = match (a_end, b_end) {
                (Some(a_end), Some(b_end)) => Some(a_end.unify_with(b_end)?),
                (None, None) => None,
                (Some(end), None) => Some(end.clone()),
                (None, Some(end)) => Some(end.clone()),
            };

            Ok(InferredType::resolved(TypeInternal::Range {
                from: unified_start,
                to: unified_end,
            }))
        }

        (TypeInternal::Enum(a_variants), TypeInternal::Enum(b_variants)) => {
            if a_variants != b_variants {
                return Err(UnificationFailureInternal::conflicting_types(
                    vec![left_inferred_type.clone(), right_inferred_type.clone()],
                    vec![format!(
                        "conflicting enum types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    )],
                ));
            }
            Ok(InferredType::resolved(TypeInternal::Enum(
                a_variants.clone(),
            )))
        }
        (TypeInternal::Option(a_type), TypeInternal::Option(b_type)) => Ok(InferredType::resolved(
            TypeInternal::Option(a_type.unify_with(b_type)?),
        )),

        (TypeInternal::Option(a_type), _) => {
            let unified_left = a_type.unify()?;
            let unified_right = right_inferred_type.unify()?;
            let combined = unified_left.unify_with(&unified_right)?;
            Ok(InferredType::option(combined))
        }
        (_, TypeInternal::Option(a_type)) => {
            let unified_left = a_type.unify()?;
            let unified_right = left_inferred_type.unify()?;
            let combined = unified_left.unify_with(&unified_right)?;
            Ok(InferredType::option(combined))
        }

        (
            TypeInternal::Result {
                ok: a_ok,
                error: a_error,
            },
            TypeInternal::Result {
                ok: b_ok,
                error: b_error,
            },
        ) => {
            let ok = match (a_ok, b_ok) {
                (Some(a_inner), Some(b_inner)) => Some(a_inner.unify_with(b_inner)),
                (None, None) => None,
                (Some(ok), None) => Some(Ok(ok.clone())),
                (None, Some(ok)) => Some(Ok(ok.clone())),
            };

            let error = match (a_error, b_error) {
                (Some(a_inner), Some(b_inner)) => Some(a_inner.unify_with(b_inner)),
                (None, None) => None,
                (Some(ok), None) => Some(Ok(ok.clone())),
                (None, Some(ok)) => Some(Ok(ok.clone())),
            };

            handle_result(ok, error)
        }
        (TypeInternal::Variant(a_variants), TypeInternal::Variant(b_variants)) => {
            let mut variants = vec![];
            for (a_name, a_type) in a_variants {
                if let Some((_, b_type)) = b_variants.iter().find(|(b_name, _)| b_name == a_name) {
                    let unified_type = match (a_type, b_type) {
                        (Some(a_inner), Some(b_inner)) => {
                            Some(Box::new(a_inner.unify_with(b_inner)?))
                        }
                        (None, None) => None,
                        (Some(_), None) => None,
                        (None, Some(_)) => None,
                    };
                    variants.push((a_name.clone(), unified_type));
                }
            }

            let cases = variants
                .iter()
                .map(|(n, t)| (n.clone(), t.clone().map(|v| *v)))
                .collect::<Vec<_>>();

            Ok(InferredType::from_variant_cases(cases))
        }
        (
            TypeInternal::Resource {
                resource_id: a_id,
                resource_mode: a_mode,
            },
            TypeInternal::Resource {
                resource_id: b_id,
                resource_mode: b_mode,
            },
        ) => {
            if a_id != b_id || a_mode != b_mode {
                return Err(UnificationFailureInternal::conflicting_types(
                    vec![left_inferred_type.clone(), right_inferred_type.clone()],
                    vec![format!(
                        "conflicting resource types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    )],
                ));
            }

            Ok(InferredType::resource(*a_id, *a_mode))
        }

        (TypeInternal::AllOf(types), _) => {
            let mut unified_types = vec![];

            for typ in types {
                let unified = typ.unify_with(&right_inferred_type)?;
                unified_types.push(unified);
            }

            unify_all_inferred_types(&unified_types)
        }

        (_, TypeInternal::AllOf(types)) => {
            let mut unified_types = vec![];

            for typ in types {
                let unified = typ.unify_with(&left_inferred_type)?;
                unified_types.push(unified);
            }

            unify_all_inferred_types(&unified_types)
        }

        (inferred_type_left, inferred_type_right) => {
            // one of them was just originated _by default_
            let eliminated =
                InferredType::eliminate_default(vec![&left_inferred_type, &right_inferred_type]);

            if eliminated.len() == 1 {
                return Ok(eliminated[0].clone());
            }

            // both of them are equal
            if inferred_type_left == inferred_type_right {
                Ok(left_inferred_type.clone())
            } else {
                Err(conflict_error(&left_inferred_type, &right_inferred_type))
            }
        }
    }
}

fn handle_result(
    unified_ok: Option<Result<InferredType, UnificationFailureInternal>>,
    unified_error: Option<Result<InferredType, UnificationFailureInternal>>,
) -> Result<InferredType, UnificationFailureInternal> {
    match (unified_ok, unified_error) {
        // only right is known
        (Some(Err(UnificationFailureInternal::UnknownType)), Some(Ok(typ))) => {
            Ok(InferredType::resolved(TypeInternal::Result {
                ok: None,
                error: Some(typ),
            }))
        }

        // only left is known
        (Some(Ok(typ)), Some(Err(UnificationFailureInternal::UnknownType))) => {
            Ok(InferredType::resolved(TypeInternal::Result {
                ok: Some(typ),
                error: None,
            }))
        }

        // both are known
        (Some(Ok(typ1)), Some(Ok(typ2))) => Ok(InferredType::resolved(TypeInternal::Result {
            ok: Some(typ1),
            error: Some(typ2),
        })),

        // both are unknown
        (None, None) => Ok(InferredType::resolved(TypeInternal::Result {
            ok: None,
            error: None,
        })),

        // only left is known
        (Some(Ok(typ)), None) => Ok(InferredType::resolved(TypeInternal::Result {
            ok: Some(typ),
            error: None,
        })),

        // only right is unknown
        (None, Some(Ok(typ))) => Ok(InferredType::resolved(TypeInternal::Result {
            ok: None,
            error: Some(typ),
        })),

        (Some(Err(err)), _) => Err(err),

        (_, Some(Err(err))) => Err(err),
    }
}
fn conflict_error(left: &InferredType, right: &InferredType) -> UnificationFailureInternal {
    let right_origin = &right.origin;

    let expected = right.printable();
    match right_origin {
        TypeOrigin::PatternMatch(source_span) => UnificationFailureInternal::type_mismatch(
            left.clone(),
            right.clone(),
            vec![format!(
                "expected {} based on pattern match branch at line {} column {}",
                expected,
                source_span.start_line(),
                source_span.start_column()
            )],
        ),
        _ => {
            UnificationFailureInternal::conflicting_types(vec![left.clone(), right.clone()], vec![])
        }
    }
}
