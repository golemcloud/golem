use crate::inferred_type::{UnificationResult, Unified};
use crate::InferredType;

pub fn validate_unified_type(inferred_type: &InferredType) -> UnificationResult {
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
            Ok(Unified(
                (InferredType::List(Box::new(verified.inferred_type()))),
            ))
        }
        InferredType::Tuple(types) => {
            let mut verified_types = vec![];

            for typ in types {
                let verified = validate_unified_type(typ)?;
                verified_types.push(verified.inferred_type());
            }

            Ok(Unified((InferredType::Tuple(verified_types))))
        }
        InferredType::Record(field) => {
            for (field, typ) in field {
                if let Err(unresolved) = validate_unified_type(typ) {
                    return Err(format!(
                        "Un-inferred type for field {} in record: {}",
                        field, unresolved
                    ));
                }
            }

            Ok(Unified(InferredType::Record(field.clone())))
        }
        InferredType::Flags(flags) => Ok(Unified((InferredType::Flags(flags.clone())))),
        InferredType::Enum(enums) => Ok(Unified((InferredType::Enum(enums.clone())))),
        InferredType::Option(inferred_type) => {
            let result = validate_unified_type(inferred_type)?;
            Ok(Unified(
                (InferredType::Option(Box::new(result.inferred_type()))),
            ))
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
                        (_, _) => Ok(Unified((result.clone()))),
                    }
                }

                (Some(ok), None) => {
                    let ok_unified = validate_unified_type(ok);
                    match ok_unified {
                        Err(ok_err) => Err(ok_err),
                        _ => Ok(Unified((result.clone()))),
                    }
                }

                (None, Some(err)) => {
                    let err_unified = validate_unified_type(err);
                    match err_unified {
                        Err(err_err) => Err(err_err),
                        _ => Ok(Unified((result.clone()))),
                    }
                }

                (None, None) => Ok(Unified((result.clone()))),
            }
        }
        inferred_type @ InferredType::Variant(variant) => {
            for (_, typ) in variant {
                if let Some(typ) = typ {
                    validate_unified_type(typ)?;
                }
            }
            Ok(Unified((inferred_type.clone())))
        }
        resource @ InferredType::Resource { .. } => Ok(Unified((resource.clone()))),
        InferredType::OneOf(possibilities) => Err(format!("Cannot resolve {:?}", possibilities)),
        InferredType::AllOf(possibilities) => Err(format!("Cannot be all of {:?}", possibilities)),
        InferredType::Unknown => Err("Unknown".to_string()),
        inferred_type @ InferredType::Sequence(inferred_types) => {
            for typ in inferred_types {
                validate_unified_type(typ)?;
            }

            Ok(Unified(inferred_type.clone()))
        }
    }
}
