use crate::inferred_type::{flatten_all_of_list, flatten_one_of_list};
use crate::InferredType;
use std::collections::{HashMap, HashSet};

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
    match inferred_type {
        InferredType::AllOf(types) => {
            let flattened_all_ofs = flatten_all_of_list(types);
            unify_all_required_types(&flattened_all_ofs)
        }

        InferredType::OneOf(one_of_types) => {
            let flattened_one_ofs = flatten_one_of_list(one_of_types);
            Ok(unify_all_alternative_types(&flattened_one_ofs))
        }
        InferredType::Option(inner_type) => {
            let unified_inner_type = inner_type.try_unify()?;
            Ok(InferredType::Option(Box::new(unified_inner_type)))
        }

        InferredType::Result { ok, error } => {
            let unified_ok = match ok {
                Some(ok) => Some(Box::new(ok.try_unify()?)),
                None => None,
            };

            let unified_error = match error {
                Some(error) => Some(Box::new(error.try_unify()?)),
                None => None,
            };

            Ok(InferredType::Result {
                ok: unified_ok,
                error: unified_error,
            })
        }

        InferredType::Record(fields) => {
            let mut unified_fields = vec![];
            for (field, typ) in fields {
                let unified_type = typ.try_unify()?;
                unified_fields.push((field.clone(), unified_type));
            }
            Ok(InferredType::Record(unified_fields))
        }

        InferredType::Tuple(types) => {
            let mut unified_types = vec![];
            for typ in types {
                let unified_type = typ.try_unify()?;
                unified_types.push(unified_type);
            }
            Ok(InferredType::Tuple(unified_types))
        }

        InferredType::List(typ) => {
            let unified_type = typ.try_unify()?;
            Ok(InferredType::List(Box::new(unified_type)))
        }

        InferredType::Flags(flags) => Ok(InferredType::Flags(flags.clone())),

        InferredType::Enum(variants) => Ok(InferredType::Enum(variants.clone())),

        InferredType::Variant(variants) => {
            let mut unified_variants = vec![];
            for (variant, typ) in variants {
                let unified_type = match typ {
                    Some(typ) => Some(Box::new(typ.try_unify()?)),
                    None => None,
                };
                unified_variants.push((variant.clone(), unified_type.as_deref().cloned()));
            }
            Ok(InferredType::Variant(unified_variants))
        }

        InferredType::Resource {
            resource_id,
            resource_mode,
        } => Ok(InferredType::Resource {
            resource_id: *resource_id,
            resource_mode: *resource_mode,
        }),

        _ => Ok(inferred_type.clone()),
    }
}

pub fn unify_all_alternative_types(types: &Vec<InferredType>) -> InferredType {
    let mut unified_type = InferredType::Unknown;

    let mut one_ofs = vec![];
    for typ in types {
        let unified = typ.try_unify().unwrap_or(typ.clone());
        match unify_with_alternative(&unified_type, &unified) {
            Ok(t) => {
                unified_type = t.clone();
            }
            Err(_) => {
                if !unified_type.is_unknown() {
                    unified_type = InferredType::OneOf(flatten_one_of_list(&vec![
                        unified_type.clone(),
                        unified.clone(),
                    ]));
                }
                one_ofs.push(unified);
            }
        };
    }
    unified_type
}

pub fn unify_all_required_types(types: &Vec<InferredType>) -> Result<InferredType, String> {
    let mut unified_type = InferredType::Unknown;
    for typ in types {
        let unified = typ.try_unify().unwrap_or(typ.clone());
        unified_type = unified_type.unify_with_required(&unified)?;
    }
    Ok(unified_type)
}

pub fn unify_with_alternative(
    interred_type: &InferredType,
    other: &InferredType,
) -> Result<InferredType, String> {
    if interred_type == &InferredType::Unknown {
        Ok(other.clone())
    } else if other.is_unknown() || interred_type == other {
        Ok(interred_type.clone())
    } else {
        let inferred_type_printable = interred_type.printable();
        let other_printable = other.printable();

        match (interred_type, other) {
            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
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

                Ok(InferredType::Record(fields))
            }
            (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
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

                Ok(InferredType::Tuple(types))
            }

            (InferredType::List(a_type), InferredType::List(b_type)) => {
                let unified_b_type = b_type.try_unify()?;
                let unified_a_type = a_type.try_unify()?;
                if unified_a_type == unified_b_type {
                    Ok(InferredType::List(Box::new(unified_a_type)))
                } else {
                    return Err(format!(
                        "conflicting list types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ));
                }
            }

            (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => {
                // Semantics of alternative for a flag is, pick the one with the largest size
                // This is again giving users more flexibility with flags literals without the need to call a worker function
                // Also, it is impossible to pick and choose flags from both sides since the order of flags is important
                // at wasm side when calling a worker function, as they get converted to a vector of booleans zipped
                // with the actual flag names
                if a_flags.len() >= b_flags.len() {
                    Ok(InferredType::Flags(a_flags.clone()))
                } else {
                    Ok(InferredType::Flags(b_flags.clone()))
                }
            }

            (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                if a_variants == b_variants {
                    Ok(InferredType::Enum(a_variants.clone()))
                } else {
                    Err(format!(
                        "conflicting enum types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ))
                }
            }

            (InferredType::Option(a_type), InferredType::Option(b_type)) => {
                let unified_b_type = b_type.try_unify()?;
                let unified_a_type = a_type.try_unify()?;
                let combined = unified_a_type.unify_with_alternative(&unified_b_type)?;
                Ok(InferredType::Option(Box::new(combined)))
            }

            (
                InferredType::Result {
                    ok: a_ok,
                    error: a_error,
                },
                InferredType::Result {
                    ok: b_ok,
                    error: b_error,
                },
            ) => {
                let unified_b_ok = match (a_ok, b_ok) {
                    (Some(a_inner), Some(b_inner)) => {
                        let unified_b_inner = b_inner.try_unify()?;
                        let unified_a_inner = a_inner.try_unify()?;
                        if unified_a_inner == unified_b_inner {
                            Some(Box::new(unified_a_inner))
                        } else {
                            return Err(format!(
                                "conflicting result types inferred: {}, {}",
                                inferred_type_printable, other_printable
                            ));
                        }
                    }
                    (None, None) => None,
                    (Some(ok), None) => Some(Box::new(*ok.clone())),
                    (None, Some(ok)) => Some(Box::new(*ok.clone())),
                };

                let unified_b_error = match (a_error, b_error) {
                    (Some(a_inner), Some(b_inner)) => {
                        let unified_b_inner = b_inner.try_unify()?;
                        let unified_a_inner = a_inner.try_unify()?;
                        if unified_a_inner == unified_b_inner {
                            Some(Box::new(unified_a_inner))
                        } else {
                            return Err(format!(
                                "conflicting result types inferred: {}, {}",
                                inferred_type_printable, other_printable
                            ));
                        }
                    }
                    (None, None) => None,
                    (Some(ok), None) => Some(Box::new(*ok.clone())),
                    (None, Some(ok)) => Some(Box::new(*ok.clone())),
                };

                Ok(InferredType::Result {
                    ok: unified_b_ok,
                    error: unified_b_error,
                })
            }

            // We hardly reach a situation where a variant can be OneOf, but if we happen to encounter this
            // the only way to merge them is to make sure all the variants types are matching
            (InferredType::Variant(a_variants), InferredType::Variant(b_variants)) => {
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

                Ok(InferredType::Variant(variants))
            }

            // We shouldn't get into a situation where we have OneOf 2 different resource handles.
            // The only possibility of unification there is only if they match exact
            (
                InferredType::Resource {
                    resource_id: a_id,
                    resource_mode: a_mode,
                },
                InferredType::Resource {
                    resource_id: b_id,
                    resource_mode: b_mode,
                },
            ) => {
                if a_id == b_id && a_mode == b_mode {
                    Ok(InferredType::Resource {
                        resource_id: *a_id,
                        resource_mode: *a_mode,
                    })
                } else {
                    Err(format!(
                        "conflicting resource types inferred: {}, {}",
                        inferred_type_printable, other_printable
                    ))
                }
            }

            (InferredType::AllOf(a_types), inferred_types) => {
                let unified_all_types = unify_all_required_types(a_types)?;
                let alternative_type = inferred_types.try_unify()?;

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

            (inferred_types, InferredType::AllOf(b_types)) => {
                let unified_all_types = unify_all_required_types(b_types)?;
                let alternative_type = inferred_types.try_unify()?;

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
                    Ok(a.clone())
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

        match (inferred_type, other) {
            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                let mut fields: HashMap<String, InferredType> = HashMap::new();
                // Common fields unified else kept it as it is
                for (a_name, a_type) in a_fields {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        fields.insert(a_name.clone(), a_type.unify_with_required(b_type)?);
                    } else {
                        fields.insert(a_name.clone(), a_type.clone());
                    }
                }

                for (a_name, a_type) in b_fields {
                    if !a_fields.iter().any(|(b_name, _)| b_name == a_name) {
                        fields.insert(a_name.clone(), a_type.clone());
                    }
                }

                Ok(InferredType::Record(internal::sort_and_convert(fields)))
            }
            (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
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
                Ok(InferredType::Tuple(types))
            }
            (InferredType::List(a_type), InferredType::List(b_type)) => Ok(InferredType::List(
                Box::new(a_type.unify_with_required(b_type)?),
            )),
            (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => {
                if a_flags.len() >= b_flags.len() {
                    if b_flags.iter().all(|b| a_flags.contains(b)) {
                        Ok(InferredType::Flags(a_flags.clone()))
                    } else {
                        Err(format!(
                            "conflicting flag types inferred. {}, {}",
                            inferred_type_printable, other_printable
                        ))
                    }
                } else if a_flags.iter().all(|a| b_flags.contains(a)) {
                    Ok(InferredType::Flags(b_flags.clone()))
                } else {
                    Err(format!(
                        "conflicting tuple types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    ))
                }
            }
            (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                if a_variants != b_variants {
                    return Err(format!(
                        "conflicting enum types inferred. {}, {}",
                        inferred_type_printable, other_printable
                    ));
                }
                Ok(InferredType::Enum(a_variants.clone()))
            }
            (InferredType::Option(a_type), InferredType::Option(b_type)) => Ok(
                InferredType::Option(Box::new(a_type.unify_with_required(b_type)?)),
            ),

            (InferredType::Option(a_type), inferred_type) => {
                let unified_left = a_type.try_unify()?;
                let unified_right = inferred_type.try_unify()?;
                let combined = unified_left.unify_with_required(&unified_right)?;
                Ok(InferredType::Option(Box::new(combined)))
            }
            (inferred_type, InferredType::Option(a_type)) => {
                let unified_left = a_type.try_unify()?;
                let unified_right = inferred_type.try_unify()?;
                let combined = unified_left.unify_with_required(&unified_right)?;
                Ok(InferredType::Option(Box::new(combined)))
            }

            (
                InferredType::Result {
                    ok: a_ok,
                    error: a_error,
                },
                InferredType::Result {
                    ok: b_ok,
                    error: b_error,
                },
            ) => {
                let ok = match (a_ok, b_ok) {
                    (Some(a_inner), Some(b_inner)) => {
                        Some(Box::new(a_inner.unify_with_required(b_inner)?))
                    }
                    (None, None) => None,
                    (Some(ok), None) => Some(Box::new(*ok.clone())),
                    (None, Some(ok)) => Some(Box::new(*ok.clone())),
                };

                let error = match (a_error, b_error) {
                    (Some(a_inner), Some(b_inner)) => {
                        Some(Box::new(a_inner.unify_with_required(b_inner)?))
                    }
                    (None, None) => None,
                    (Some(ok), None) => Some(Box::new(*ok.clone())),
                    (None, Some(ok)) => Some(Box::new(*ok.clone())),
                };
                Ok(InferredType::Result { ok, error })
            }
            (InferredType::Variant(a_variants), InferredType::Variant(b_variants)) => {
                let mut variants = HashMap::new();
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
                        variants.insert(a_name.clone(), unified_type);
                    }
                }
                Ok(InferredType::Variant(
                    variants
                        .iter()
                        .map(|(n, t)| (n.clone(), t.clone().map(|v| *v)))
                        .collect(),
                ))
            }
            (
                InferredType::Resource {
                    resource_id: a_id,
                    resource_mode: a_mode,
                },
                InferredType::Resource {
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
                Ok(InferredType::Resource {
                    resource_id: *a_id,
                    resource_mode: *a_mode,
                })
            }

            (InferredType::AllOf(types), InferredType::OneOf(one_of_types)) => {
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

            (InferredType::OneOf(one_of_types), InferredType::AllOf(all_of_types)) => {
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

            (InferredType::OneOf(types), inferred_type) => {
                let mut unified = None;
                for typ in types {
                    match typ.unify_with_alternative(inferred_type) {
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

            (inferred_type, InferredType::OneOf(types)) => {
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

            (InferredType::AllOf(types), inferred_type) => {
                let x = types
                    .iter()
                    .filter(|x| !x.is_unknown())
                    .map(|t| t.unify_with_required(inferred_type).unwrap())
                    .collect::<Vec<_>>();

                unify_all_required_types(&x)
            }

            (inferred_type, InferredType::AllOf(types)) => {
                let result = InferredType::AllOf(types.clone()).try_unify()?;

                result.unify_with_required(inferred_type)
            }

            (inferred_type_left, inferred_type_right) => {
                if inferred_type_left == inferred_type_right {
                    Ok(inferred_type_left.clone())
                } else if inferred_type_left.is_number() && inferred_type_right.is_number() {
                    Ok(InferredType::AllOf(vec![
                        inferred_type_left.clone(),
                        inferred_type_right.clone(),
                    ]))
                } else if inferred_type_left.is_string() && inferred_type_right.is_number() {
                    Ok(inferred_type_right.clone())
                } else if inferred_type_left.is_number() && inferred_type_right.is_string() {
                    Ok(inferred_type_left.clone())
                } else {
                    Err(format!(
                        "conflicting types inferred. {}, {}",
                        inferred_type_left.printable(),
                        inferred_type_right.printable()
                    ))
                }
            }
        }
    }
}

mod internal {
    use crate::inferred_type::unification::Unified;
    use crate::{InferredType, TypeName};
    use std::collections::HashMap;

    pub(crate) fn sort_and_convert(
        hashmap: HashMap<String, InferredType>,
    ) -> Vec<(String, InferredType)> {
        let mut vec: Vec<(String, InferredType)> = hashmap.into_iter().collect();
        vec.sort_by(|a, b| a.0.cmp(&b.0));
        vec
    }

    pub(crate) fn validate_unified_type(inferred_type: &InferredType) -> Result<Unified, String> {
        match inferred_type {
            InferredType::Bool => Ok(Unified(InferredType::Bool)),
            InferredType::S8 => Ok(Unified(InferredType::S8)),
            InferredType::U8 => Ok(Unified(InferredType::U8)),
            InferredType::S16 => Ok(Unified(InferredType::S16)),
            InferredType::U16 => Ok(Unified(InferredType::U16)),
            InferredType::S32 => Ok(Unified(InferredType::S32)),
            InferredType::U32 => Ok(Unified(InferredType::U32)),
            InferredType::S64 => Ok(Unified(InferredType::S64)),
            InferredType::U64 => Ok(Unified(InferredType::U64)),
            InferredType::F32 => Ok(Unified(InferredType::F32)),
            InferredType::F64 => Ok(Unified(InferredType::F64)),
            InferredType::Chr => Ok(Unified(InferredType::Chr)),
            InferredType::Str => Ok(Unified(InferredType::Str)),
            InferredType::List(inferred_type) => {
                let verified = validate_unified_type(inferred_type)?;
                Ok(Unified(InferredType::List(Box::new(
                    verified.inferred_type(),
                ))))
            }
            InferredType::Tuple(types) => {
                let mut verified_types = vec![];

                for typ in types {
                    let verified = validate_unified_type(typ)?;
                    verified_types.push(verified.inferred_type());
                }

                Ok(Unified(InferredType::Tuple(verified_types)))
            }
            InferredType::Record(field) => {
                for (field, typ) in field {
                    if let Err(unresolved) = validate_unified_type(typ) {
                        return Err(format!(
                            "cannot determine the type of field {} in record: {}",
                            field, unresolved
                        ));
                    }
                }

                Ok(Unified(InferredType::Record(field.clone())))
            }
            InferredType::Flags(flags) => Ok(Unified(InferredType::Flags(flags.clone()))),
            InferredType::Enum(enums) => Ok(Unified(InferredType::Enum(enums.clone()))),
            InferredType::Option(inferred_type) => {
                let result = validate_unified_type(inferred_type)?;
                Ok(Unified(InferredType::Option(Box::new(
                    result.inferred_type(),
                ))))
            }
            result @ InferredType::Result { ok, error } => {
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
                            (_, _) => Ok(Unified(result.clone())),
                        }
                    }

                    (Some(ok), None) => {
                        let ok_unified = validate_unified_type(ok);
                        match ok_unified {
                            Err(ok_err) => Err(ok_err),
                            _ => Ok(Unified(result.clone())),
                        }
                    }

                    (None, Some(err)) => {
                        let err_unified = validate_unified_type(err);
                        match err_unified {
                            Err(err_err) => Err(err_err),
                            _ => Ok(Unified(result.clone())),
                        }
                    }

                    (None, None) => Ok(Unified(result.clone())),
                }
            }
            inferred_type @ InferredType::Variant(variant) => {
                for (_, typ) in variant {
                    if let Some(typ) = typ {
                        validate_unified_type(typ)?;
                    }
                }
                Ok(Unified(inferred_type.clone()))
            }
            instance @ InferredType::Instance { .. } => Ok(Unified(instance.clone())),
            resource @ InferredType::Resource { .. } => Ok(Unified(resource.clone())),
            InferredType::OneOf(possibilities) => Err(format!(
                "conflicting types inferred: {}",
                display_multiple_types(possibilities)
            )),
            InferredType::AllOf(possibilities) => Err(format!(
                "conflicting types inferred:  {}",
                display_multiple_types(possibilities)
            )),

            InferredType::Unknown => Err("cannot determine the type".to_string()),
            inferred_type @ InferredType::Sequence(inferred_types) => {
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
