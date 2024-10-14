use golem_wasm_ast::analysis::AnalysedType;
use crate::{Expr, InferredType};
use crate::type_checker::{check_type_mismatch, check_unresolved_types, TypeCheckError, TypeMismatchError};

pub fn validate(
    expected_type: &AnalysedType,
    actual_type: &InferredType,
    actual_expr: &Expr,
) -> Result<(), TypeCheckError> {
    let un_inferred = check_unresolved_types(actual_expr);
    if let Err(msg) = un_inferred {
        Err(TypeCheckError::unresolved_types_error(msg))
    } else {
        check_type_mismatch(expected_type, actual_type).map_err(|e| {
            TypeCheckError::type_mismatch_error(
                expected_type.clone(),
                actual_type.clone(),
            )
        })
    }
}