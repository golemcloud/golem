use crate::type_checker::{check_type_mismatch, check_unresolved_types, TypeCheckError};
use crate::{Expr, InferredType, TypeName};
use golem_wasm_ast::analysis::AnalysedType;

pub fn validate(
    expected_type: &AnalysedType,
    actual_type: &InferredType,
    actual_expr: &Expr,
) -> Result<(), TypeCheckError> {
    let un_inferred = check_unresolved_types(actual_expr);
    if let Err(unresolved_type_error) = un_inferred {
        Err(TypeCheckError::unresolved_types_error(
            unresolved_type_error.add_message(
                format!(
                    "Expected type: {}",
                    TypeName::try_from(expected_type.clone())
                        .map(|type_name| type_name.to_string())
                        .unwrap_or_default()
                )
                .as_str(),
            ),
        ))
    } else {
        check_type_mismatch(expected_type, actual_type).map_err(TypeCheckError::type_mismatch_error)
    }
}
