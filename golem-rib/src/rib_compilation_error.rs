use std::fmt;
use std::fmt::{format, Display};
use crate::{Expr, TypeName};
use crate::type_checker::{ExhaustivePatternMatchError, FunctionCallTypeError, InvalidExpr, InvalidMathExprError, InvalidProgramReturn, InvalidWorkerName, TypeMismatchError, UnResolvedTypesError};

pub struct RibCompilationError {
    pub cause: String,
    pub expr: Expr,
    pub immediate_parent: Option<Expr>,
    pub additional_error_details: Vec<String>,
    pub help_messages: Vec<String>
}

impl Display for RibCompilationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.expr.source_span();

        writeln!(
            f,
            "error in the following rib found at line {}, column {}",
            span.start_line(), span.start_column()
        )?;

        writeln!(f, "`{}`", self.expr)?;

        if let Some(parent) = &self.immediate_parent {
            writeln!(f, "found within:")?;
            writeln!(f, "`{}`", parent)?;
        }

        writeln!(f, "cause: {}", self.cause)?;

        if !self.additional_error_details.is_empty() {
            for message in &self.additional_error_details {
                writeln!(f, "{}", message)?;
            }
        }


        if !self.help_messages.is_empty() {
            for message in &self.help_messages {
                writeln!(f, "help: {}", message)?;
            }
        }

        Ok(())
    }
}

impl From<UnResolvedTypesError> for RibCompilationError {
    fn from(value: UnResolvedTypesError) -> Self {

        let mut rib_compilation_error = RibCompilationError {
            cause: "cannot determine the type".to_string(),
            expr: value.unresolved_expr,
            immediate_parent: value.parent_expr,
            additional_error_details: value.additional_messages,
            help_messages: value.help_messages
        };

        if !value.path.is_empty() {
            rib_compilation_error.additional_error_details.push(format!("unresolved type at path: `{}`", value.path));
        }

        rib_compilation_error
    }
}

impl From<TypeMismatchError> for RibCompilationError {
    fn from(value: TypeMismatchError) -> Self {

        let expected =
            TypeName::try_from(value.expected_type).map(|x| format!("Expected {}", x)).ok();

        let actual =
            TypeName::try_from(value.actual_type).map(|x| format!("Found {}", x)).ok();

        let cause_suffix = match (expected, actual) {
            (Some(expected), Some(actual)) => format!("{}. {}", expected, actual),
            (Some(expected), None) => format!("{}. Found unknown type. Specify types and try again", expected),
            _ => "".to_string()
        };

        let cause = if !value.field_path.is_empty() {
            format!("type mismatch at path: `{}`. {}", value.field_path, cause_suffix)
        } else {
            format!("type mismatch. {}", cause_suffix)
        };

        RibCompilationError {
            cause,
            expr: value.expr_with_wrong_type,
            immediate_parent: value.parent_expr,
            additional_error_details:vec![],
            help_messages: vec![]
        }
    }
}

impl From<FunctionCallTypeError> for RibCompilationError {
    fn from(value: FunctionCallTypeError) -> Self {
        match value {
            FunctionCallTypeError::InvalidFunctionCall {
                function_call_name: function_name,
            } => {
                RibCompilationError {
                    cause: format!("Invalid function call: `{}`", function_name),
                    expr: Expr::identifier_global(function_name, None),
                    immediate_parent: None,
                    additional_error_details: vec![],
                    help_messages: vec![]
                }
            }
            FunctionCallTypeError::TypeMisMatch {
                function_call_name: call_type,
                error,
                ..
            } => {
                let mut original_compilation : RibCompilationError = error.into();

                let error_detail = format!(
                    "Invalid argument to the function `{}`", call_type
                );

                original_compilation.additional_error_details.push(error_detail);

                original_compilation
            }
            FunctionCallTypeError::MissingRecordFields {
                function_call_name: call_type,
                missing_fields,
                argument
            } => {
                let missing_fields = missing_fields
                    .iter()
                    .map(|path| path.to_string())
                    .collect::<Vec<String>>()
                    .join(", ");

                let rib_compilation_error = RibCompilationError {
                    cause: format!(
                        "Invalid argument to the function `{}`:  Missing field(s) in record `{}`",
                        call_type, missing_fields
                    ),
                    expr: argument,
                    immediate_parent: None,
                    additional_error_details: vec![],
                    help_messages: vec![]
                };

                rib_compilation_error
            }

            FunctionCallTypeError::UnResolvedTypes {
                function_call_name: call_type,
                unresolved_error,
                expected_type,
                ..
            } => {

                let expected = TypeName::try_from(expected_type.clone())
                    .map(|t| format!("Expected {}", t))
                    .unwrap_or_default();

                let mut rib_compilation_error =
                    RibCompilationError::from(unresolved_error);

                rib_compilation_error.additional_error_details.push(format!(
                    "Invalid argument to `{}`. {}",
                    call_type,
                    expected
                ));

                rib_compilation_error
            }
        }
    }
}

impl From<InvalidExpr> for RibCompilationError {
    fn from(value: InvalidExpr) -> Self {

       let cause = format!("cannot be a {}", value.expected_type);

        RibCompilationError {
            cause,
            expr: value.expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![]
        }
    }
}

impl From<InvalidProgramReturn> for RibCompilationError {
    fn from(value: InvalidProgramReturn) -> Self {
        RibCompilationError {
            cause: value.message,
            expr: value.return_expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![]
        }
    }
}

impl From<InvalidWorkerName> for RibCompilationError {
    fn from(value: InvalidWorkerName) -> Self {
        RibCompilationError {
            cause: value.message,
            expr: value.worker_name_expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![]
        }
    }
}


impl From<InvalidMathExprError> for RibCompilationError {
    fn from(value: InvalidMathExprError) -> Self {

        let expr = match value {
            InvalidMathExprError::Both { math_expr, ..}
            | InvalidMathExprError::Left { math_expr, .. }
            | InvalidMathExprError::Right { math_expr, .. } => {
                math_expr
            }
        };

        RibCompilationError {
            cause:  "invalid math expression".to_string(),
            expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![]
        }
    }
}

impl From<ExhaustivePatternMatchError> for RibCompilationError {
    fn from(value: ExhaustivePatternMatchError) -> Self {
        let expr = match &value {
            ExhaustivePatternMatchError::MissingConstructors { predicate, .. }
            | ExhaustivePatternMatchError::DeadCode { predicate, .. } => {
                predicate.clone()
            }
        };

        let cause = match value {
            ExhaustivePatternMatchError::MissingConstructors { missing_constructors, .. } => {
                format!("non-exhaustive pattern match. The following patterns are not covered: `{}`. To ensure a complete match, add these patterns or cover them with a wildcard (`_`) or an identifier.", missing_constructors.join(", "))
            }
            ExhaustivePatternMatchError::DeadCode { dead_pattern, cause, .. } => {
                format!("Error: Dead code detected. The pattern `{}` is unreachable due to the existence of the pattern `{}` prior to it", dead_pattern, cause)
            }
        };

        RibCompilationError {
            cause,
            expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![]
        }
    }
}