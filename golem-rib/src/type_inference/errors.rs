use crate::{Expr, InferredType};
use crate::type_inference::kind::{GetTypeKind, TypeKind};

// Ambiguous type error occurs when we are unable to push down an inferred type
// to the inner expression since there is an ambiguity between what the expression is
// and what is being pushed down
pub struct AmbiguousTypeError {
    pub expr: Expr,
    pub message: String,
}

impl AmbiguousTypeError {
    pub fn from(inferred_expr: &InferredType, expr: &Expr) -> AmbiguousTypeError {
        let kind = inferred_expr.get_type_kind();

        match kind {
            TypeKind::Ambiguous { possibilities } => {
                let error_message = format!(
                    "ambiguous types inferred {}",
                    possibilities
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                AmbiguousTypeError {
                    expr: expr.clone(),
                    message: error_message,
                }
            }
            _ => {
                let error_message =
                    format!("ambiguous types inferred , {},{}", TypeKind::Option, kind);
                AmbiguousTypeError {
                    expr: expr.clone(),
                    message: error_message,
                }
            }
        }
    }
}

pub struct InvalidPatternMatchError {
    pub predicate_expr: Expr,
    pub expected_kind: TypeKind
}

impl InvalidPatternMatchError {
    pub fn from(predicate_expr: &Expr, expected_kind: &TypeKind) -> InvalidPatternMatchError {
        InvalidPatternMatchError {
            predicate_expr: predicate_expr.clone(),
            expected_kind: expected_kind.clone()
        }
    }
}