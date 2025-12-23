// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::inferred_type::flatten_all_of_list;
use crate::{InferredType, TypeInternal};
use std::fmt::{Display, Formatter};
use std::ops::Deref;

#[derive(Clone, Debug)]
pub struct Unified(InferredType);

impl Unified {
    pub fn inferred_type(&self) -> InferredType {
        self.0.clone()
    }
}

pub fn unify(inferred_type: &InferredType) -> Result<Unified, UnificationFailureInternal> {
    let possibly_unified_type = try_unify_type(inferred_type)?;

    validate_unified_type(&possibly_unified_type)
}

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
            Ok(InferredType::option(unified_inner_type).override_origin(inferred_type.origin()))
        }

        TypeInternal::Result { ok, error } => {
            let unified_ok = ok.as_ref().map(try_unify_type);

            let unified_error = error.as_ref().map(try_unify_type);

            Ok(InferredType::resolved(TypeInternal::Result {
                ok: unified_ok.transpose()?,
                error: unified_error.transpose()?,
            })
            .override_origin(inferred_type.origin()))
        }

        TypeInternal::Record(fields) => {
            let mut unified_fields = vec![];
            for (field, typ) in fields {
                let unified_type = try_unify_type(typ)?;
                unified_fields.push((field.clone(), unified_type));
            }
            Ok(InferredType::resolved(TypeInternal::Record(unified_fields))
                .override_origin(inferred_type.origin()))
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
            let unified_end = end.as_ref().map(try_unify_type).transpose()?;
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
            name,
            owner,
        } => Ok(InferredType::resolved(TypeInternal::Resource {
            resource_id: *resource_id,
            resource_mode: *resource_mode,
            name: name.clone(),
            owner: owner.clone(),
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
        left: InferredType,
        right: InferredType,
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
                left: expected,
                right: found,
            } => {
                write!(
                    f,
                    "type mismatch: expected {}, found {}",
                    expected.printable(),
                    found.printable(),
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
    pub fn type_mismatch(expected: InferredType, found: InferredType) -> Self {
        UnificationFailureInternal::TypeMisMatch {
            left: expected,
            right: found,
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
                    fields.push((a_name.clone(), unify_both_inferred_types(a_type, b_type)?));
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
                types.push(unify_both_inferred_types(a_type, b_type)?);
            }
            Ok(InferredType::resolved(TypeInternal::Tuple(types)))
        }
        (TypeInternal::List(a_type), TypeInternal::List(b_type)) => Ok(InferredType::resolved(
            TypeInternal::List(unify_both_inferred_types(a_type, b_type)?),
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
            let unified_start = unify_both_inferred_types(a_start, b_start)?;
            let unified_end = match (a_end, b_end) {
                (Some(a_end), Some(b_end)) => Some(unify_both_inferred_types(a_end, b_end)?),
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
            TypeInternal::Option(unify_both_inferred_types(a_type, b_type)?),
        )),

        (TypeInternal::Option(a_type), _) => {
            let unified_left = a_type.unify()?;
            let unified_right = right_inferred_type.unify()?;
            let combined = unify_both_inferred_types(&unified_left, &unified_right)?;
            Ok(InferredType::option(combined))
        }
        (_, TypeInternal::Option(a_type)) => {
            let unified_left = a_type.unify()?;
            let unified_right = left_inferred_type.unify()?;
            let combined = unify_both_inferred_types(&unified_left, &unified_right)?;
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
                (Some(a_inner), Some(b_inner)) => Some(unify_both_inferred_types(a_inner, b_inner)),
                (None, None) => None,
                (Some(ok), None) => Some(Ok(ok.clone())),
                (None, Some(ok)) => Some(Ok(ok.clone())),
            };

            let error = match (a_error, b_error) {
                (Some(a_inner), Some(b_inner)) => Some(unify_both_inferred_types(a_inner, b_inner)),
                (None, None) => None,
                (Some(ok), None) => Some(Ok(ok.clone())),
                (None, Some(ok)) => Some(Ok(ok.clone())),
            };

            Ok(InferredType::resolved(TypeInternal::Result {
                ok: ok.transpose()?,
                error: error.transpose()?,
            }))
        }
        (TypeInternal::Variant(a_variants), TypeInternal::Variant(b_variants)) => {
            let mut variants = vec![];
            for (a_name, a_type) in a_variants {
                if let Some((_, b_type)) = b_variants.iter().find(|(b_name, _)| b_name == a_name) {
                    let unified_type = match (a_type, b_type) {
                        (Some(a_inner), Some(b_inner)) => {
                            Some(Box::new(unify_both_inferred_types(a_inner, b_inner)?))
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
                name: a_name,
                owner: a_owner,
            },
            TypeInternal::Resource {
                resource_id: b_id,
                resource_mode: b_mode,
                name: b_name,
                owner: b_owner,
            },
        ) => {
            if a_id != b_id || a_mode != b_mode || a_owner != b_owner || a_name != b_name {
                return Err(UnificationFailureInternal::conflicting_types(
                    vec![left_inferred_type.clone(), right_inferred_type.clone()],
                    vec![format!(
                        "conflicting resource types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    )],
                ));
            }

            Ok(InferredType::resource(
                *a_id,
                *a_mode,
                a_owner.clone(),
                a_name.clone(),
            ))
        }

        (TypeInternal::AllOf(types), _) => {
            let mut unified_types = vec![];

            for typ in types {
                let unified = unify_both_inferred_types(typ, &right_inferred_type)?;
                unified_types.push(unified);
            }

            unify_all_inferred_types(&unified_types)
        }

        (_, TypeInternal::AllOf(types)) => {
            let mut unified_types = vec![];

            for typ in types {
                let unified = unify_both_inferred_types(typ, &left_inferred_type)?;
                unified_types.push(unified);
            }

            unify_all_inferred_types(&unified_types)
        }

        (inferred_type_left, inferred_type_right) => {
            if left_inferred_type.is_number() && right_inferred_type.is_number() {
                let eliminated = InferredType::eliminate_default(vec![
                    &left_inferred_type,
                    &right_inferred_type,
                ]);

                if eliminated.len() == 1 {
                    return Ok(eliminated[0].clone());
                }
            }

            // both of them are equal
            if inferred_type_left == inferred_type_right {
                Ok(left_inferred_type.clone())
            } else {
                Err(UnificationFailureInternal::type_mismatch(
                    right_inferred_type.clone(),
                    left_inferred_type.clone(),
                ))
            }
        }
    }
}

pub(crate) fn validate_unified_type(
    inferred_type: &InferredType,
) -> Result<Unified, UnificationFailureInternal> {
    match inferred_type.internal_type() {
        TypeInternal::Bool => Ok(Unified(InferredType::bool())),
        TypeInternal::S8 => Ok(Unified(InferredType::s8())),
        TypeInternal::U8 => Ok(Unified(InferredType::u8())),
        TypeInternal::S16 => Ok(Unified(InferredType::s16())),
        TypeInternal::U16 => Ok(Unified(InferredType::u16())),
        TypeInternal::S32 => Ok(Unified(InferredType::s32())),
        TypeInternal::U32 => Ok(Unified(InferredType::u32())),
        TypeInternal::S64 => Ok(Unified(InferredType::s64())),
        TypeInternal::U64 => Ok(Unified(InferredType::u64())),
        TypeInternal::F32 => Ok(Unified(InferredType::f32())),
        TypeInternal::F64 => Ok(Unified(InferredType::f64())),
        TypeInternal::Chr => Ok(Unified(InferredType::char())),
        TypeInternal::Str => Ok(Unified(InferredType::string())),
        TypeInternal::List(inferred_type) => {
            let verified = validate_unified_type(inferred_type)?;
            Ok(Unified(InferredType::list(verified.inferred_type())))
        }
        TypeInternal::Tuple(types) => {
            let mut verified_types = vec![];

            for typ in types {
                let verified = validate_unified_type(typ)?;
                verified_types.push(verified.inferred_type());
            }

            Ok(Unified(InferredType::tuple(verified_types)))
        }
        TypeInternal::Record(field) => {
            for (field, typ) in field {
                if let Err(unresolved) = validate_unified_type(typ) {
                    return Err(UnificationFailureInternal::conflicting_types(
                        vec![inferred_type.clone()],
                        vec![format!(
                            "cannot determine the type of field {} in record: {}",
                            field, unresolved
                        )],
                    ));
                }
            }

            Ok(Unified(InferredType::record(field.clone())))
        }
        TypeInternal::Flags(flags) => Ok(Unified(InferredType::flags(flags.clone()))),
        TypeInternal::Enum(enums) => Ok(Unified(InferredType::enum_(enums.clone()))),
        TypeInternal::Option(inferred_type) => {
            let result = validate_unified_type(inferred_type)?;
            Ok(Unified(InferredType::option(result.inferred_type())))
        }
        TypeInternal::Result { ok, error } => {
            // For Result, we try to be flexible with types
            // Example: Allow Rib script to simply return ok(x) as the final output, even if it doesn't know anything about error
            match (ok, error) {
                (Some(ok), Some(err)) => {
                    let ok_unified = validate_unified_type(ok);
                    let err_unified = validate_unified_type(err);

                    match (ok_unified, err_unified) {
                        // We fail only if both are unknown
                        (Err(ok_err), Err(_)) => Err(ok_err),
                        (_, _) => Ok(Unified(inferred_type.clone())),
                    }
                }

                (Some(ok), None) => {
                    let ok_unified = validate_unified_type(ok);
                    match ok_unified {
                        Err(ok_err) => Err(ok_err),
                        _ => Ok(Unified(inferred_type.clone())),
                    }
                }

                (None, Some(err)) => {
                    let err_unified = validate_unified_type(err);
                    match err_unified {
                        Err(err_err) => Err(err_err),
                        _ => Ok(Unified(inferred_type.clone())),
                    }
                }

                (None, None) => Ok(Unified(inferred_type.clone())),
            }
        }
        TypeInternal::Variant(variant) => {
            for (_, typ) in variant {
                if let Some(typ) = typ {
                    validate_unified_type(typ)?;
                }
            }
            Ok(Unified(inferred_type.clone()))
        }
        TypeInternal::Range {
            from: start,
            to: end,
        } => {
            let unified_start = validate_unified_type(start)?;
            let unified_end = end
                .clone()
                .map(|end| validate_unified_type(&end))
                .transpose()?;

            Ok(Unified(InferredType::range(
                unified_start.inferred_type(),
                unified_end.map(|end| end.inferred_type()),
            )))
        }

        TypeInternal::Instance { .. } => Ok(Unified(inferred_type.clone())),
        TypeInternal::Resource { .. } => Ok(Unified(inferred_type.clone())),
        TypeInternal::AllOf(possibilities) => Err(UnificationFailureInternal::conflicting_types(
            possibilities.clone(),
            vec![format!(
                "conflicting types: {}",
                display_multiple_types(possibilities)
            )],
        )),

        TypeInternal::Unknown => Err(UnificationFailureInternal::ConflictingTypes {
            conflicting_types: vec![inferred_type.clone()],
            additional_error_detail: vec!["cannot determine type".to_string()],
        }),
        TypeInternal::Sequence(inferred_types) => {
            for typ in inferred_types {
                validate_unified_type(typ)?;
            }

            Ok(Unified(inferred_type.clone()))
        }
    }
}

fn display_multiple_types(types: &[InferredType]) -> String {
    let types = types.iter().map(|x| x.printable()).collect::<Vec<_>>();

    types.join(", ")
}
