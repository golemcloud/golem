use crate::inferred_type::UnificationResult;
use crate::InferredType;

pub fn validate_unified_type(inferred_type: &InferredType) -> UnificationResult{
    match inferred_type {
        InferredType::Bool => UnificationResult::unified(InferredType::Bool),
        InferredType::S8 => UnificationResult::unified(InferredType::S8),
        InferredType::U8 => UnificationResult::unified(InferredType::U8),
        InferredType::S16 => UnificationResult::unified(InferredType::S16),
        InferredType::U16 =>  UnificationResult::unified(InferredType::U16),
        InferredType::S32 => UnificationResult::unified(InferredType::S32),
        InferredType::U32 => UnificationResult::unified(InferredType::U32),
        InferredType::S64 => UnificationResult::unified(InferredType::S64),
        InferredType::U64 => UnificationResult::unified(InferredType::U64),
        InferredType::F32 => UnificationResult::unified(InferredType::F32),
        InferredType::F64 => UnificationResult::unified(InferredType::F64),
        InferredType::Chr => UnificationResult::unified(InferredType::Chr),
        InferredType::Str => UnificationResult::unified(InferredType::Str),
        InferredType::List(inferred_type) => validate_unified_type(inferred_type),
        InferredType::Tuple(types) => {
            for typ in types {
                if let UnificationResult::Failed(unresolved) = validate_unified_type(typ) {
                    return UnificationResult::Failed(unresolved);
                }
            }
            UnificationResult::unified(InferredType::Tuple(types.clone()))
        }
        InferredType::Record(field) => {
            for (field, typ) in field {
                if let UnificationResult::Failed(unresolved) = validate_unified_type(typ) {
                    return UnificationResult::Failed(format!(
                        "Un-inferred type for field {} in record: {}",
                        field, unresolved
                    ));
                }
            }
            UnificationResult::unified(InferredType::Record(field.clone()))
        }
        InferredType::Flags(_) => None,
        InferredType::Enum(_) => None,
        InferredType::Option(inferred_type) => {
            if let Some(unresolved) = inferred_type.un_resolved() {
                return Some(unresolved);
            }
            None
        }
        InferredType::Result { ok, error } => {
            // Check unresolved status for `ok` and `error`
            let unresolved_ok = ok.clone().and_then(|o| o.un_resolved());
            let unresolved_error = error.clone().and_then(|e| e.un_resolved());

            // If `ok` is unresolved
            if unresolved_ok.is_some() {
                if error.is_some() && unresolved_error.is_none() {
                    // If `error` is known, return `None`
                    return None;
                }
                return unresolved_ok;
            }

            // If `error` is unresolved
            if unresolved_error.is_some() {
                if ok.is_some() && ok.as_ref().unwrap().un_resolved().is_none() {
                    // If `ok` is known, return `None`
                    return None;
                }
                return unresolved_error;
            }

            // Both `ok` and `error` are resolved or not present
            None
        }
        InferredType::Variant(variant) => {
            for (_, typ) in variant {
                if let Some(typ) = typ {
                    if let Some(unresolved) = typ.un_resolved() {
                        return Some(unresolved);
                    }
                }
            }
            None
        }
        InferredType::Resource { .. } => None,
        InferredType::OneOf(possibilities) => {
            Some(format!("Cannot resolve {:?}", possibilities))
        }
        InferredType::AllOf(possibilities) => {
            Some(format!("Cannot be all of {:?}", possibilities))
        }
        InferredType::Unknown => Some("Unknown".to_string()),
        InferredType::Sequence(inferred_types) => {
            for typ in inferred_types {
                if let Some(unresolved) = typ.un_resolved() {
                    return Some(unresolved);
                }
            }
            None
        }
    }
}

pub fn unify_types_and_verify(inferred_type: &InferredType) -> Result<InferredType, String> {
    let unified = inferred_type.try_unify()?;
    if let Some(unresolved) = unified.un_resolved() {
        return Err(unresolved);
    }
    Ok(unified)
}
