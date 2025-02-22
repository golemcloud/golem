use std::fmt;
use std::fmt::Display;
use crate::{Expr, TypeName};
use crate::type_checker::{TypeMismatchError, UnResolvedTypesError};

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

