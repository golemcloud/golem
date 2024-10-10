use crate::inferred_type::validation::internal::{failed, unified};
use crate::inferred_type::UnificationResult;
use crate::InferredType;

pub fn validate_unified_type(inferred_type: &InferredType) -> UnificationResult {
    match inferred_type {
        InferredType::Bool => unified(InferredType::Bool),
        InferredType::S8 => unified(InferredType::S8),
        InferredType::U8 => unified(InferredType::U8),
        InferredType::S16 => unified(InferredType::S16),
        InferredType::U16 => unified(InferredType::U16),
        InferredType::S32 => unified(InferredType::S32),
        InferredType::U32 => unified(InferredType::U32),
        InferredType::S64 => unified(InferredType::S64),
        InferredType::U64 => unified(InferredType::U64),
        InferredType::F32 => unified(InferredType::F32),
        InferredType::F64 => unified(InferredType::F64),
        InferredType::Chr => unified(InferredType::Chr),
        InferredType::Str => unified(InferredType::Str),
        InferredType::List(inferred_type) => {
            let verified = validate_unified_type(inferred_type)?;
            unified(InferredType::List(Box::new(verified.inferred_type())))
        }
        InferredType::Tuple(types) => {
            let mut verified_types = vec![];

            for typ in types {
                let verified = validate_unified_type(typ)?;
                verified_types.push(verified.inferred_type());
            }

            unified(InferredType::Tuple(verified_types))
        }
        InferredType::Record(field) => {
            for (field, typ) in field {
                if let Err(unresolved) = validate_unified_type(typ) {
                    return failed(format!(
                        "Un-inferred type for field {} in record: {}",
                        field, unresolved
                    ));
                }
            }

            unified(InferredType::Record(field.clone()))
        }
        InferredType::Flags(flags) => unified(InferredType::Flags(flags.clone())),
        InferredType::Enum(enums) => unified(InferredType::Enum(enums.clone())),
        InferredType::Option(inferred_type) => {
            let result = validate_unified_type(inferred_type)?;
            unified(InferredType::Option(Box::new(result.inferred_type())))
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
                            failed(err)
                        }
                        (_, _) => unified(result.clone()),
                    }
                }

                (Some(ok), None) => {
                    let ok_unified = validate_unified_type(ok);
                    match ok_unified {
                        Err(ok_err) => failed(ok_err),
                        _ => unified(result.clone()),
                    }
                }

                (None, Some(err)) => {
                    let err_unified = validate_unified_type(err);
                    match err_unified {
                        Err(err_err) => failed(err_err),
                        _ => unified(result.clone()),
                    }
                }

                (None, None) => unified(result.clone()),
            }
        }
        inferred_type @ InferredType::Variant(variant) => {
            for (_, typ) in variant {
                if let Some(typ) = typ {
                    validate_unified_type(typ)?;
                }
            }
            unified(inferred_type.clone())
        }
        resource @ InferredType::Resource { .. } => unified(resource.clone()),
        InferredType::OneOf(possibilities) => failed(format!("Cannot resolve {:?}", possibilities)),
        InferredType::AllOf(possibilities) => {
            failed(format!("Cannot be all of {:?}", possibilities))
        }
        InferredType::Unknown => failed("Unknown".to_string()),
        inferred_type @ InferredType::Sequence(inferred_types) => {
            for typ in inferred_types {
                validate_unified_type(typ)?;
            }

            unified(inferred_type.clone())
        }
    }
}

mod internal {
    use crate::inferred_type::{validate_unified_type, UnificationResult, Unified};
    use crate::InferredType;

    pub(crate) fn unified(inferred_type: InferredType) -> UnificationResult {
        Ok(Unified(inferred_type))
    }

    pub(crate) fn failed(unresolved: String) -> UnificationResult {
        Err(unresolved)
    }
}
