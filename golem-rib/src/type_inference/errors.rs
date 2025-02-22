use crate::{Expr, InferredType};
use crate::type_inference::kind::{GetTypeKind, TypeKind};

// Ambiguous type error occurs when we are unable to push down an inferred type
// to the inner expression since there is an ambiguity between what the expression is
// and what is being pushed down

#[derive(Clone)]
pub struct AmbiguousTypeError {
    pub expr: Expr,
    pub ambiguous_types: Vec<TypeKind>, // At this point, the max resolution is only until a kind
    pub additional_error_details: Vec<String>,
}

impl AmbiguousTypeError {
    pub fn new(inferred_expr: &InferredType, expr: &Expr) -> AmbiguousTypeError {
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

                let possibilities = possibilities
                    .into_iter()
                    .collect::<Vec<_>>();


                AmbiguousTypeError {
                    expr: expr.clone(),
                    ambiguous_types: possibilities,
                    additional_error_details: vec![error_message],
                }
            }
            _ => {
                AmbiguousTypeError {
                    expr: expr.clone(),
                    ambiguous_types: vec![TypeKind::Option, kind],
                    additional_error_details: vec![]
                }
            }
        }
    }

    pub fn with_additional_error_detail(&self, detail: &str) -> AmbiguousTypeError {
        let mut error = self.clone();
        error.additional_error_details.push(detail.to_string());
        error
    }
}


pub enum InvalidPatternMatchError {
    ConstructorMismatch {
        predicate_expr: Expr,
        match_expr: Expr,
        constructor_name: String,
    },
    ArgSizeMismatch {
        predicate_expr: Expr,
        match_expr: Expr,
        expected_arg_size: usize,
        actual_arg_size: usize,
    },
}

impl InvalidPatternMatchError {
    pub fn constructor_type_mismatch(predicate_expr: &Expr, match_expr: &Expr, constructor_name: &str) -> InvalidPatternMatchError {
        InvalidPatternMatchError::ConstructorMismatch {
            predicate_expr: predicate_expr.clone(),
            match_expr: match_expr.clone(),
            constructor_name: constructor_name.to_string()
        }
    }

    pub fn arg_size_mismatch(predicate_expr: &Expr, match_expr: &Expr, expected_arg_size: usize, actual_arg_size: usize) -> InvalidPatternMatchError {
        InvalidPatternMatchError::ArgSizeMismatch {
            predicate_expr: predicate_expr.clone(),
            match_expr: match_expr.clone(),
            expected_arg_size,
            actual_arg_size
        }
    }
}