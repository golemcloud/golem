use crate::inferred_type::{flatten_all_of_list, flatten_one_of_list};
use crate::{InferredType, TypeInternal, TypeOrigin};
use std::collections::HashSet;
use std::ops::Deref;

#[derive(Clone, Debug)]
pub struct Unified(InferredType);

impl Unified {
    pub fn inferred_type(&self) -> InferredType {
        self.0.clone()
    }
}

pub fn unify(inferred_type: &InferredType) -> Result<Unified, String> {
    let possibly_unified_type = try_unify_type(inferred_type)?;

    internal::validate_unified_type(&possibly_unified_type)
}

pub fn try_unify_type(inferred_type: &InferredType) -> Result<InferredType, String> {
    match inferred_type.inner.deref() {
        TypeInternal::AllOf(types) => {
            let flattened_all_ofs = flatten_all_of_list(types);
            unify_all_required_types(&flattened_all_ofs)
        }

        TypeInternal::OneOf(one_of_types) => {
            let flattened_one_ofs = flatten_one_of_list(one_of_types);
            Ok(unify_all_alternative_types(&flattened_one_ofs))
        }
        TypeInternal::Option(inner_type) => {
            let unified_inner_type = inner_type.try_unify()?;
            Ok(InferredType::resolved(TypeInternal::Option(
                unified_inner_type,
            )))
        }

        TypeInternal::Result { ok, error } => {
            let unified_ok = match ok {
                Some(ok) => Some(ok.try_unify()?),
                None => None,
            };

            let unified_error = match error {
                Some(error) => Some(error.try_unify()?),
                None => None,
            };

            Ok(InferredType::resolved(TypeInternal::Result {
                ok: unified_ok,
                error: unified_error,
            }))
        }

        TypeInternal::Record(fields) => {
            let mut unified_fields = vec![];
            for (field, typ) in fields {
                let unified_type = typ.try_unify()?;
                unified_fields.push((field.clone(), unified_type));
            }
            Ok(InferredType::resolved(TypeInternal::Record(unified_fields)))
        }

        TypeInternal::Tuple(types) => {
            let mut unified_types = vec![];
            for typ in types {
                let unified_type = typ.try_unify()?;
                unified_types.push(unified_type);
            }
            Ok(InferredType::resolved(TypeInternal::Tuple(unified_types)))
        }

        TypeInternal::List(typ) => {
            let unified_type = typ.try_unify()?;
            Ok(InferredType::resolved(TypeInternal::List(unified_type)))
        }

        TypeInternal::Range {
            from: start,
            to: end,
        } => {
            let unified_start = start.try_unify()?;
            let unified_end = end.as_ref().map(|end| end.try_unify()).transpose()?;
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
                    Some(typ) => Some(Box::new(typ.try_unify()?)),
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

        _ => Ok(inferred_type.clone()),
    }
}

pub fn unify_all_alternative_types(types: &Vec<InferredType>) -> InferredType {
    let mut unified_type = InferredType::unknown();

    let mut one_ofs = vec![];
    for typ in types {
        let unified = typ.try_unify().unwrap_or(typ.clone());
        match unify_with_alternative(&unified_type, &unified) {
            Ok(t) => {
                unified_type = t.clone();
            }
            Err(_) => {
                if !unified_type.is_unknown() {
                    unified_type =
                        InferredType::resolved(TypeInternal::OneOf(flatten_one_of_list(&vec![
                            unified_type.clone(),
                            unified.clone(),
                        ])));
                }
                one_ofs.push(unified);
            }
        };
    }
    unified_type
}

pub fn unify_all_required_types(types: &Vec<InferredType>) -> Result<InferredType, String> {
    let mut unified_type = InferredType::unknown();
    for typ in types {
        let unified = typ.try_unify().unwrap_or(typ.clone());
        unified_type = unified_type.unify_with_required(&unified)?;
    }
    Ok(unified_type)
}

pub fn unify_with_alternative(
    inferred_type: &InferredType,
    other: &InferredType,
) -> Result<InferredType, String> {
    if inferred_type.is_unknown() {
        Ok(other.clone())
    } else if other.is_unknown() || inferred_type == other {
        Ok(inferred_type.clone())
    } else {
        let inferred_type_printable = inferred_type.printable();
        let other_printable = other.printable();

        match (inferred_type.inner.deref(), other.inner.deref()) {
            (TypeInternal::Record(a_fields), TypeInternal::Record(b_fields)) => {
                if a_fields.len() != b_fields.len() {
                    return Err(format!("conflicting record types inferred  {}, {}. the size of the members in the records don't match", inferred_type_printable, other_printable));
                }

                let mut fields = a_fields.clone();

                for (field, typ) in fields.iter_mut() {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_field, _)| b_field == field)
                    {
                        let unified_b_type = b_type.try_unify()?;
                        let unified_a_type = typ.try_unify()?;
                        if unified_a_type == unified_b_type {
                            *typ = unified_a_type
                        } else {
                            return Err(format!(
                                "conflicting record types inferred: {}, {}",
                                inferred_type_printable, other_printable
                            ));
                        }
                    } else {
                        return Err(format!(
                            "conflicting record types inferred: {}, {}",
                            inferred_type_printable, other_printable
                        ));
                    }
                }

                Ok(InferredType::new(
                    TypeInternal::Record(fields),
                    TypeOrigin::NoOrigin,
                ))
            }
            (TypeInternal::Tuple(a_types), TypeInternal::Tuple(b_types)) => {
                if a_types.len() != b_types.len() {
                    return Err(format!(
                        "conflicting tuple types inferred: {}, {}. lengths don't match",
                        inferred_type_printable, other_printable
                    ));
                }

                let mut types = a_types.clone();

                for (a_type, b_type) in types.iter_mut().zip(b_types) {
                    let unified_b_type = b_type.try_unify()?;
                    let unified_a_type = a_type.try_unify()?;
                    if unified_a_type == unified_b_type {
                        *a_type = unified_a_type
                    } else {
                        return Err(format!(
                            "conflicting tuple types inferred: {}, {}",
                            inferred_type_printable, other_printable
                        ));
                    }
                }

                Ok(InferredType::new(
                    TypeInternal::Tuple(types),
                    TypeOrigin::NoOrigin,
                ))
            }

            (TypeInternal::List(a_type), TypeInternal::List(b_type)) => {
                let unified_b_type = b_type.try_unify()?;
                let unified_a_type = a_type.try_unify()?;
                if unified_a_type == unified_b_type {
                    Ok(InferredType::resolved(TypeInternal::List(unified_a_type)))
                } else {
                    return Err(format!(
                        "conflicting list types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ));
                }
            }

            (TypeInternal::Flags(a_flags), TypeInternal::Flags(b_flags)) => {
                // Semantics of alternative for a flag is, pick the one with the largest size
                // This is again giving users more flexibility with flags literals without the need to call a worker function
                // Also, it is impossible to pick and choose flags from both sides since the order of flags is important
                // at wasm side when calling a worker function, as they get converted to a vector of booleans zipped
                // with the actual flag names
                if a_flags.len() >= b_flags.len() {
                    Ok(InferredType::resolved(TypeInternal::Flags(a_flags.clone())))
                } else {
                    Ok(InferredType::resolved(TypeInternal::Flags(b_flags.clone())))
                }
            }

            (TypeInternal::Enum(a_variants), TypeInternal::Enum(b_variants)) => {
                if a_variants == b_variants {
                    Ok(InferredType::resolved(TypeInternal::Enum(
                        a_variants.clone(),
                    )))
                } else {
                    Err(format!(
                        "conflicting enum types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ))
                }
            }

            (TypeInternal::Option(a_type), TypeInternal::Option(b_type)) => {
                let unified_b_type = b_type.try_unify()?;
                let unified_a_type = a_type.try_unify()?;
                let combined = unified_a_type.unify_with_alternative(&unified_b_type)?;
                Ok(InferredType::resolved(TypeInternal::Option(combined)))
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
                let unified_b_ok = match (a_ok, b_ok) {
                    (Some(a_inner), Some(b_inner)) => {
                        let unified_b_inner = b_inner.try_unify()?;
                        let unified_a_inner = a_inner.try_unify()?;
                        if unified_a_inner == unified_b_inner {
                            Some(unified_a_inner)
                        } else {
                            return Err(format!(
                                "conflicting result types inferred: {}, {}",
                                inferred_type_printable, other_printable
                            ));
                        }
                    }
                    (None, None) => None,
                    (Some(ok), None) => Some(ok.clone()),
                    (None, Some(ok)) => Some(ok.clone()),
                };

                let unified_b_error = match (a_error, b_error) {
                    (Some(a_inner), Some(b_inner)) => {
                        let unified_b_inner = b_inner.try_unify()?;
                        let unified_a_inner = a_inner.try_unify()?;
                        if unified_a_inner == unified_b_inner {
                            Some(unified_a_inner)
                        } else {
                            return Err(format!(
                                "conflicting result types inferred: {}, {}",
                                inferred_type_printable, other_printable
                            ));
                        }
                    }
                    (None, None) => None,
                    (Some(ok), None) => Some(ok.clone()),
                    (None, Some(ok)) => Some(ok.clone()),
                };

                Ok(InferredType::resolved(TypeInternal::Result {
                    ok: unified_b_ok,
                    error: unified_b_error,
                }))
            }

            // We hardly reach a situation where a variant can be OneOf, but if we happen to encounter this
            // the only way to merge them is to make sure all the variants types are matching
            (TypeInternal::Variant(a_variants), TypeInternal::Variant(b_variants)) => {
                if a_variants.len() != b_variants.len() {
                    return Err(format!("conflicting variant types inferred: {}, {}. size of variant fields don't match", inferred_type_printable, other_printable));
                }

                let mut variants = a_variants.clone();

                for (variant, a_type) in variants.iter_mut() {
                    if let Some((_, b_type)) = b_variants
                        .iter()
                        .find(|(b_variant, _)| b_variant == variant)
                    {
                        let x = match b_type {
                            Some(x) => Some(x.try_unify()?),
                            None => None,
                        };

                        let y = match a_type {
                            Some(y) => Some(y.try_unify()?),
                            None => None,
                        };
                        if x == y {
                            *a_type = x
                        } else {
                            return Err(format!(
                                "conflicting result types inferred: {}, {}",
                                inferred_type_printable, other_printable
                            ));
                        }
                    } else {
                        return Err(format!(
                            "conflicting result types inferred: {}, {}",
                            inferred_type_printable, other_printable
                        ));
                    }
                }

                Ok(InferredType::resolved(TypeInternal::Variant(variants)))
            }

            // We shouldn't get into a situation where we have OneOf 2 different resource handles.
            // The only possibility of unification there is only if they match exact
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
                if a_id == b_id && a_mode == b_mode {
                    Ok(InferredType::resolved(TypeInternal::Resource {
                        resource_id: *a_id,
                        resource_mode: *a_mode,
                    }))
                } else {
                    Err(format!(
                        "conflicting resource types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ))
                }
            }

            (TypeInternal::AllOf(a_types), _) => {
                let unified_all_types = unify_all_required_types(a_types)?;
                let alternative_type = other.try_unify()?;

                if unified_all_types == alternative_type {
                    Ok(unified_all_types)
                } else {
                    Err(format!(
                        "ambiguous types inferred: {}, {}",
                        unified_all_types.printable(),
                        alternative_type.printable()
                    ))
                }
            }

            (_, TypeInternal::AllOf(b_types)) => {
                let unified_all_types = unify_all_required_types(b_types)?;
                let alternative_type = inferred_type.try_unify()?;

                if unified_all_types == alternative_type {
                    Ok(unified_all_types)
                } else {
                    Err(format!(
                        "ambiguous types inferred: {}, {}",
                        unified_all_types.printable(),
                        alternative_type.printable()
                    ))
                }
            }

            (a, b) => {
                if a == b {
                    Ok(inferred_type.clone())
                } else {
                    Err(format!(
                        "ambiguous types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ))
                }
            }
        }
    }
}

pub fn unify_with_required(
    inferred_type: &InferredType,
    other: &InferredType,
) -> Result<InferredType, String> {
    if other.is_unknown() {
        inferred_type.try_unify()
    } else if inferred_type.is_unknown() {
        other.try_unify()
    } else if inferred_type == other {
        inferred_type.try_unify()
    } else {
        let inferred_type_printable = inferred_type.printable();
        let other_printable = other.printable();

        match (inferred_type.inner.deref(), other.inner.deref()) {
            (TypeInternal::Record(a_fields), TypeInternal::Record(b_fields)) => {
                let mut fields: Vec<(String, InferredType)> = vec![];
                // Common fields unified else kept it as it is
                for (a_name, a_type) in a_fields {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        fields.push((a_name.clone(), a_type.unify_with_required(b_type)?));
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
                    return Err(format!(
                        "conflicting tuple types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    ));
                }
                let mut types = Vec::new();
                for (a_type, b_type) in a_types.iter().zip(b_types) {
                    types.push(a_type.unify_with_required(b_type)?);
                }
                Ok(InferredType::resolved(TypeInternal::Tuple(types)))
            }
            (TypeInternal::List(a_type), TypeInternal::List(b_type)) => Ok(InferredType::resolved(
                TypeInternal::List(a_type.unify_with_required(b_type)?),
            )),
            (TypeInternal::Flags(a_flags), TypeInternal::Flags(b_flags)) => {
                if a_flags.len() >= b_flags.len() {
                    if b_flags.iter().all(|b| a_flags.contains(b)) {
                        Ok(InferredType::resolved(TypeInternal::Flags(a_flags.clone())))
                    } else {
                        Err(format!(
                            "conflicting flag types inferred. {}, {}",
                            inferred_type_printable, other_printable
                        ))
                    }
                } else if a_flags.iter().all(|a| b_flags.contains(a)) {
                    Ok(InferredType::resolved(TypeInternal::Flags(b_flags.clone())))
                } else {
                    Err(format!(
                        "conflicting tuple types inferred. {}, {}",
                        inferred_type_printable, other_printable
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
                let unified_start = a_start.unify_with_required(b_start)?;
                let unified_end = match (a_end, b_end) {
                    (Some(a_end), Some(b_end)) => Some(a_end.unify_with_required(b_end)?),
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
                    return Err(format!(
                        "conflicting enum types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    ));
                }
                Ok(InferredType::resolved(TypeInternal::Enum(
                    a_variants.clone(),
                )))
            }
            (TypeInternal::Option(a_type), TypeInternal::Option(b_type)) => Ok(
                InferredType::resolved(TypeInternal::Option(a_type.unify_with_required(b_type)?)),
            ),

            (TypeInternal::Option(a_type), _) => {
                let unified_left = a_type.try_unify()?;
                let unified_right = other.try_unify()?;
                let combined = unified_left.unify_with_required(&unified_right)?;
                Ok(InferredType::option(combined))
            }
            (_, TypeInternal::Option(a_type)) => {
                let unified_left = a_type.try_unify()?;
                let unified_right = inferred_type.try_unify()?;
                let combined = unified_left.unify_with_required(&unified_right)?;
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
                    (Some(a_inner), Some(b_inner)) => Some(a_inner.unify_with_required(b_inner)?),
                    (None, None) => None,
                    (Some(ok), None) => Some(ok.clone()),
                    (None, Some(ok)) => Some(ok.clone()),
                };

                let error = match (a_error, b_error) {
                    (Some(a_inner), Some(b_inner)) => Some(a_inner.unify_with_required(b_inner)?),
                    (None, None) => None,
                    (Some(ok), None) => Some(ok.clone()),
                    (None, Some(ok)) => Some(ok.clone()),
                };
                Ok(InferredType::result(ok, error))
            }
            (TypeInternal::Variant(a_variants), TypeInternal::Variant(b_variants)) => {
                let mut variants = vec![];
                for (a_name, a_type) in a_variants {
                    if let Some((_, b_type)) =
                        b_variants.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        let unified_type = match (a_type, b_type) {
                            (Some(a_inner), Some(b_inner)) => {
                                Some(Box::new(a_inner.unify_with_required(b_inner)?))
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
                    return Err(format!(
                        "conflicting resource types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    ));
                }

                Ok(InferredType::resource(*a_id, *a_mode))
            }

            (TypeInternal::AllOf(types), TypeInternal::OneOf(one_of_types)) => {
                for typ in types {
                    if !one_of_types.contains(typ) {
                        let ambiguous_one_of = one_of_types
                            .iter()
                            .map(|x| x.printable())
                            .collect::<Vec<_>>();

                        return Err(format!(
                            "conflicting types inferred. {}, {}",
                            typ.printable(),
                            ambiguous_one_of.join(", ")
                        ));
                    }
                }
                unify_all_required_types(types)
            }

            (TypeInternal::OneOf(one_of_types), TypeInternal::AllOf(all_of_types)) => {
                for required_type in all_of_types {
                    if !one_of_types.contains(required_type) {
                        let ambiguous_one_of = one_of_types
                            .iter()
                            .map(|x| x.printable())
                            .collect::<Vec<_>>();
                        return Err(format!(
                            "conflicting types inferred. {}, {}",
                            required_type.printable(),
                            ambiguous_one_of.join(", ")
                        ));
                    }
                }
                unify_all_required_types(all_of_types)
            }

            (TypeInternal::OneOf(types), _) => {
                let mut unified = None;
                for typ in types {
                    match typ.unify_with_alternative(other) {
                        Ok(result) => {
                            unified = Some(result);
                            break;
                        }
                        Err(_) => continue,
                    }
                }

                if let Some(unified) = unified {
                    Ok(unified)
                } else {
                    let type_set: HashSet<_> = types.iter().collect::<HashSet<_>>();
                    Err(format!(
                        "conflicting types inferred. {}, {}",
                        type_set
                            .iter()
                            .map(|x| x.printable())
                            .collect::<Vec<_>>()
                            .join(", "),
                        inferred_type.printable()
                    ))
                }
            }

            (_, TypeInternal::OneOf(types)) => {
                if types.contains(inferred_type) {
                    Ok(inferred_type.clone())
                } else {
                    let type_set: HashSet<_> = types.iter().collect::<HashSet<_>>();
                    Err(format!(
                        "conflicting types inferred. {}, {}",
                        type_set
                            .iter()
                            .map(|x| x.printable())
                            .collect::<Vec<_>>()
                            .join(", "),
                        inferred_type.printable()
                    ))
                }
            }

            (TypeInternal::AllOf(types), _) => {
                let x = types
                    .iter()
                    .filter(|x| !x.is_unknown())
                    .map(|t| t.unify_with_required(other).unwrap())
                    .collect::<Vec<_>>();

                unify_all_required_types(&x)
            }

            (_, TypeInternal::AllOf(types)) => {
                let result =
                    InferredType::resolved(TypeInternal::AllOf(types.clone())).try_unify()?;

                result.unify_with_required(inferred_type)
            }

            (inferred_type_left, inferred_type_right) => {
                if inferred_type_left == inferred_type_right {
                    Ok(inferred_type.clone())
                } else if inferred_type.is_number() && other.is_number() {
                    Ok(InferredType::new(
                        TypeInternal::AllOf(vec![inferred_type.clone(), other.clone()]),
                        TypeOrigin::NoOrigin,
                    ))
                } else {
                    Err(format!(
                        "conflicting types inferred. {}, {}",
                        inferred_type.printable(),
                        other.printable()
                    ))
                }
            }
        }
    }
}

mod internal {
    use crate::inferred_type::unification::Unified;
    use crate::{InferredType, TypeInternal};
    use std::ops::Deref;

    pub(crate) fn validate_unified_type(inferred_type: &InferredType) -> Result<Unified, String> {
        match inferred_type.inner.deref() {
            TypeInternal::Bool
            | TypeInternal::S8
            | TypeInternal::U8
            | TypeInternal::S16
            | TypeInternal::U16
            | TypeInternal::S32
            | TypeInternal::U32
            | TypeInternal::S64
            | TypeInternal::U64
            | TypeInternal::F32
            | TypeInternal::F64
            | TypeInternal::Chr
            | TypeInternal::Str => Ok(Unified(inferred_type.clone())),
            TypeInternal::List(inferred_type) => {
                let verified = validate_unified_type(inferred_type)?;
                Ok(Unified(InferredType::resolved(TypeInternal::List(
                    verified.inferred_type(),
                ))))
            }
            TypeInternal::Tuple(types) => {
                let mut verified_types = vec![];

                for typ in types {
                    let verified = validate_unified_type(typ)?;
                    verified_types.push(verified.inferred_type());
                }

                Ok(Unified(InferredType::resolved(TypeInternal::Tuple(
                    verified_types,
                ))))
            }
            TypeInternal::Record(field) => {
                for (field, typ) in field {
                    if let Err(unresolved) = validate_unified_type(typ) {
                        return Err(format!(
                            "cannot determine the type of field {} in record: {}",
                            field, unresolved
                        ));
                    }
                }

                Ok(Unified(InferredType::resolved(TypeInternal::Record(
                    field.clone(),
                ))))
            }
            TypeInternal::Flags(_) => Ok(Unified(inferred_type.clone())),
            TypeInternal::Enum(_) => Ok(Unified(inferred_type.clone())),
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
                            (Err(ok_err), Err(err_err)) => {
                                let err = format!("Ok: {}, Error: {}", ok_err, err_err);
                                Err(err)
                            }
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
            TypeInternal::OneOf(possibilities) => Err(format!(
                "conflicting types inferred: {}",
                display_multiple_types(possibilities)
            )),
            TypeInternal::AllOf(possibilities) => Err(format!(
                "conflicting types inferred:  {}",
                display_multiple_types(possibilities)
            )),

            TypeInternal::Unknown => Err("cannot determine the type".to_string()),
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
}
