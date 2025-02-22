use std::fmt;
use std::fmt::Display;
use crate::{Expr, TypeName};
use crate::type_checker::{FunctionCallTypeError, TypeMismatchError, UnResolvedTypesError};

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
            "Error at the following rib-expression found at line {}, column {}",
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
        RibCompilationError {
            cause: "cannot determine the type".to_string(),
            expr: value.unresolved_expr,
            immediate_parent: value.parent_expr,
            additional_error_details: value.additional_messages,
            help_messages: value.help_messages
        }
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

        let cause = if value.field_path.is_field_name() {
            format!("type mismatch at field: `{}`. {}", value.field_path, cause_suffix)
        } else if value.field_path.is_index() {
            format!("type mismatch at index: `{}`. {}", value.field_path, cause_suffix)
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

        let mut error_details = vec![];

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

