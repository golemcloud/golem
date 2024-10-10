use std::collections::{HashMap, HashSet};
use crate::{InferredType};
use crate::inferred_type::{flatten_all_of_list, flatten_one_of_list, UnificationResult, validate_unified_type};

pub fn unify(inferred_type: &InferredType) -> Result<InferredType, String> {
    let possibly_unified_type = try_unify_type(inferred_type)?;

    match validate_unified_type(&possibly_unified_type) {
        UnificationResult::Success(inferred_type) => Ok(inferred_type),
        UnificationResult::Failed(e) => Err(e),
    }

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

pub fn unify_with_alternative(interred_type: &InferredType, other: &InferredType) -> Result<InferredType, String> {
    if interred_type == &InferredType::Unknown {
        Ok(other.clone())
    } else if other.is_unknown() || interred_type == other {
        Ok(interred_type.clone())
    } else {
        match (interred_type, other) {
            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                if a_fields.len() != b_fields.len() {
                    return Err("Record fields do not match".to_string());
                }

                let mut fields = a_fields.clone();

                for (field, typ) in fields.iter_mut() {
                    if let Some((_, b_type)) =
                        b_fields.iter().find(|(b_field, _)| b_field == field)
                    {
                        let unified_b_type = b_type.try_unify()?;
                        let unified_a_type = typ.try_unify()?;
                        if unified_a_type == unified_b_type {
                            *typ = unified_a_type
                        } else {
                            return Err("Record fields do not match".to_string());
                        }
                    } else {
                        return Err("Record fields do not match".to_string());
                    }
                }

                Ok(InferredType::Record(fields))
            }
            (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
                if a_types.len() != b_types.len() {
                    return Err("Tuple lengths do not match".to_string());
                }

                let mut types = a_types.clone();

                for (a_type, b_type) in types.iter_mut().zip(b_types) {
                    let unified_b_type = b_type.try_unify()?;
                    let unified_a_type = a_type.try_unify()?;
                    if unified_a_type == unified_b_type {
                        *a_type = unified_a_type
                    } else {
                        return Err("Record fields do not match".to_string());
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
                    Err("Record fields do not match".to_string())
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
                    Err("Enum variants do not match".to_string())
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
                            return Err("Record fields do not match".to_string());
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
                            return Err("Record fields do not match".to_string());
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
                    return Err("Variant fields do not match".to_string());
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
                            return Err("Variant fields do not match".to_string());
                        }
                    } else {
                        return Err("Variant fields do not match".to_string());
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
                    Err("Resource id or mode do not match".to_string())
                }
            }

            (InferredType::AllOf(a_types), inferred_types) => {
                let unified_all_types = unify_all_required_types(a_types)?;
                let alternative_type = inferred_types.try_unify()?;

                if unified_all_types == alternative_type {
                    Ok(unified_all_types)
                } else {
                    Err("AllOf types do not match".to_string())
                }
            }

            (inferred_types, InferredType::AllOf(b_types)) => {
                let unified_all_types = unify_all_required_types(b_types)?;
                let alternative_type = inferred_types.try_unify()?;

                if unified_all_types == alternative_type {
                    Ok(unified_all_types)
                } else {
                    Err("AllOf types do not match".to_string())
                }
            }

            (a, b) => {
                if a == b {
                    Ok(a.clone())
                } else {
                    Err(format!(
                        "Types do not match. Inferred to be both {:?} and {:?}",
                        a, b
                    ))
                }
            }
        }
    }
}


pub fn unify_with_required(inferred_type: &InferredType, other: &InferredType) -> Result<InferredType, String> {
    if other.is_unknown() {
        inferred_type.try_unify()
    } else if inferred_type.is_unknown() {
        other.try_unify()
    } else if inferred_type == other {
        inferred_type.try_unify()
    } else {
        match (inferred_type, other) {
            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                let mut fields: HashMap<String, InferredType> = HashMap::new();
                // Common fields unified else kept it as it is
                for (a_name, a_type) in a_fields {
                    if let Some((_, b_type)) =
                        b_fields.iter().find(|(b_name, _)| b_name == a_name)
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
                    return Err("Tuple lengths do not match".to_string());
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
                        Err("Flags do not match".to_string())
                    }
                } else if a_flags.iter().all(|a| b_flags.contains(a)) {
                    Ok(InferredType::Flags(b_flags.clone()))
                } else {
                    Err("Flags do not match".to_string())
                }
            }
            (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                if a_variants != b_variants {
                    return Err("Enum variants do not match".to_string());
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
                    return Err("Resource id or mode do not match".to_string());
                }
                Ok(InferredType::Resource {
                    resource_id: *a_id,
                    resource_mode: *a_mode,
                })
            }

            (InferredType::AllOf(types), InferredType::OneOf(one_of_types)) => {
                for typ in types {
                    if !one_of_types.contains(typ) {
                        return Err(
                            "AllOf types are not part of OneOf types".to_string(),
                        );
                    }
                }
                unify_all_required_types(types)
            }

            (InferredType::OneOf(one_of_types), InferredType::AllOf(all_of_types)) => {
                for required_type in all_of_types {
                    if !one_of_types.contains(required_type) {
                        return Err(
                            "OneOf types are not part of AllOf types".to_string(),
                        );
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
                    Err(format!("Types do not match. Inferred to be any of {:?}, but found (or used as) {:?} ",  type_set, inferred_type))
                }
            }

            (inferred_type, InferredType::OneOf(types)) => {
                if types.contains(inferred_type) {
                    Ok(inferred_type.clone())
                } else {
                    let type_set: HashSet<_> = types.iter().collect::<HashSet<_>>();
                    Err(format!("Types do not match. Inferred to be any of {:?}, but found (or used as) {:?} ",  type_set, inferred_type))
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

            (inferred_type1, inferred_type2) => {
                if inferred_type1 == inferred_type2 {
                    Ok(inferred_type1.clone())
                } else if inferred_type1.is_number() && inferred_type2.is_number() {
                    Ok(InferredType::AllOf(vec![
                        inferred_type1.clone(),
                        inferred_type2.clone(),
                    ]))
                } else {
                    Err(format!(
                        "Types do not match. Inferred to be both {:?} and {:?}",
                        inferred_type1, inferred_type2
                    ))
                }
            }
        }
    }
}

mod internal {
    use std::collections::HashMap;
    use crate::InferredType;

    pub(crate) fn sort_and_convert(
        hashmap: HashMap<String, InferredType>,
    ) -> Vec<(String, InferredType)> {
        let mut vec: Vec<(String, InferredType)> = hashmap.into_iter().collect();
        vec.sort_by(|a, b| a.0.cmp(&b.0));
        vec
    }
}
