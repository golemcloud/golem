use crate::type_inference::kind::{GetTypeKind, TypeKind};
use crate::{Expr, InferredType};

#[derive(Debug, Clone)]
pub struct AmbiguousTypeError {
    pub expr: Expr,
    pub ambiguous_types: Vec<TypeKind>, // At this point, the max resolution is only until a kind
    pub additional_error_details: Vec<String>,
}

impl AmbiguousTypeError {
    pub fn new(
        inferred_expr: &InferredType,
        expr: &Expr,
        expected: &TypeKind,
    ) -> AmbiguousTypeError {
        let actual_kind = inferred_expr.get_type_kind();
        match actual_kind {
            TypeKind::Ambiguous { possibilities } => {
                let possibilities = possibilities.into_iter().collect::<Vec<_>>();

                AmbiguousTypeError {
                    expr: expr.clone(),
                    ambiguous_types: possibilities,
                    additional_error_details: vec![],
                }
            }
            actual_kind => AmbiguousTypeError {
                expr: expr.clone(),
                ambiguous_types: vec![expected.clone(), actual_kind],
                additional_error_details: vec![],
            },
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
        constructor_name: String,
        expected_arg_size: usize,
        actual_arg_size: usize,
    },
}

impl InvalidPatternMatchError {
    pub fn constructor_type_mismatch(
        predicate_expr: &Expr,
        match_expr: &Expr,
        constructor_name: &str,
    ) -> InvalidPatternMatchError {
        InvalidPatternMatchError::ConstructorMismatch {
            predicate_expr: predicate_expr.clone(),
            match_expr: match_expr.clone(),
            constructor_name: constructor_name.to_string(),
        }
    }

    pub fn arg_size_mismatch(
        predicate_expr: &Expr,
        match_expr: &Expr,
        constructor_name: &str,
        expected_arg_size: usize,
        actual_arg_size: usize,
    ) -> InvalidPatternMatchError {
        InvalidPatternMatchError::ArgSizeMismatch {
            predicate_expr: predicate_expr.clone(),
            match_expr: match_expr.clone(),
            expected_arg_size,
            actual_arg_size,
            constructor_name: constructor_name.to_string(),
        }
    }
}
