use crate::type_checker::ExhaustivePatternMatchError;
use crate::{
    ActualType, AmbiguousTypeError, CustomError, ExpectedType, Expr, FunctionCallError,
    InvalidPatternMatchError, InvalidWorkerName, TypeMismatchError, TypeName, TypeUnificationError,
    UnResolvedTypesError,
};
use std::fmt;
use std::fmt::{Debug, Display};

// RibTypeError is front end of all types of errors that can occur during type inference phase
// or type checker phase such as `UnresolvedTypesError`, `TypeMismatchError`, `AmbiguousTypeError` etc
#[derive(Clone, PartialEq)]
pub struct RibTypeError {
    pub cause: String,
    pub expr: Expr,
    pub immediate_parent: Option<Expr>,
    pub additional_error_details: Vec<String>,
    pub help_messages: Vec<String>,
}

impl Debug for RibTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self)
    }
}

impl RibTypeError {
    pub fn with_additional_error_detail(&self, detail: &str) -> RibTypeError {
        let mut error = self.clone();
        error.additional_error_details.push(detail.to_string());
        error
    }
}

impl Display for RibTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.expr.source_span();

        writeln!(
            f,
            "error in the following rib found at line {}, column {}",
            span.start_line(),
            span.start_column()
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

impl From<UnResolvedTypesError> for RibTypeError {
    fn from(value: UnResolvedTypesError) -> Self {
        let mut rib_compilation_error = RibTypeError {
            cause: "cannot determine the type".to_string(),
            expr: value.unresolved_expr,
            immediate_parent: value.parent_expr,
            additional_error_details: value.additional_messages,
            help_messages: value.help_messages,
        };

        if !value.path.is_empty() {
            rib_compilation_error
                .additional_error_details
                .push(format!("unresolved type at path: `{}`", value.path));
        }

        rib_compilation_error
    }
}

impl From<TypeUnificationError> for RibTypeError {
    fn from(value: TypeUnificationError) -> Self {
        match value {
            TypeUnificationError::TypeMismatchError { error } => error.into(),
            TypeUnificationError::UnresolvedTypesError { error } => error.into(),
        }
    }
}

impl From<TypeMismatchError> for RibTypeError {
    fn from(value: TypeMismatchError) -> Self {
        let expected = match value.expected_type {
            ExpectedType::AnalysedType(analysed_type) => TypeName::try_from(analysed_type)
                .map(|x| format!("expected {}", x))
                .ok(),
            ExpectedType::Hint(kind) => Some(format!("expected {}", kind)),
            ExpectedType::InferredType(type_name) => {
                Some(format!("expected {}", type_name.printable()))
            }
        };

        let actual = match value.actual_type {
            ActualType::Hint(type_kind) => Some(format!("found {}", type_kind)),
            ActualType::Inferred(inferred_type) => TypeName::try_from(inferred_type)
                .map(|x| format!("found {}", x))
                .ok(),
        };

        let cause_suffix = match (expected, actual) {
            (Some(expected), Some(actual)) => format!("{}, {}", expected, actual),
            (Some(expected), None) => expected.to_string(),
            _ => "".to_string(),
        };

        let cause = if !value.field_path.is_empty() {
            format!(
                "type mismatch at path: `{}`. {}",
                value.field_path, cause_suffix
            )
        } else {
            format!("type mismatch. {}", cause_suffix)
        };

        RibTypeError {
            cause,
            expr: value.expr_with_wrong_type,
            immediate_parent: value.parent_expr,
            additional_error_details: value.additional_error_detail,
            help_messages: vec![],
        }
    }
}

impl From<FunctionCallError> for RibTypeError {
    fn from(value: FunctionCallError) -> Self {
        match value {
            FunctionCallError::InvalidFunctionCall {
                function_name,
                expr,
                message,
            } => RibTypeError {
                cause: format!("invalid function call `{}`", function_name),
                expr,
                immediate_parent: None,
                additional_error_details: vec![message],
                help_messages: vec![],
            },
            FunctionCallError::TypeMisMatch {
                function_name,
                error,
                ..
            } => {
                let mut original_compilation: RibTypeError = error.into();

                let error_detail = format!("invalid argument to the function `{}`", function_name);

                original_compilation
                    .additional_error_details
                    .push(error_detail);

                original_compilation
            }
            FunctionCallError::MissingRecordFields {
                function_name: call_type,
                missing_fields,
                argument,
            } => {
                let missing_fields = missing_fields
                    .iter()
                    .map(|path| path.to_string())
                    .collect::<Vec<String>>()
                    .join(", ");

                RibTypeError {
                    cause: format!(
                        "invalid argument to the function `{}`.  missing field(s) in record: `{}`",
                        call_type, missing_fields
                    ),
                    expr: argument,
                    immediate_parent: None,
                    additional_error_details: vec![],
                    help_messages: vec![],
                }
            }

            FunctionCallError::UnResolvedTypes {
                function_name: call_type,
                unresolved_error,
                expected_type,
                ..
            } => {
                let expected = TypeName::try_from(expected_type.clone())
                    .map(|t| format!("expected {}", t))
                    .unwrap_or_default();

                let mut rib_compilation_error = RibTypeError::from(unresolved_error);

                rib_compilation_error
                    .additional_error_details
                    .push(format!("invalid argument to `{}`. {}", call_type, expected));

                rib_compilation_error
            }
            FunctionCallError::InvalidResourceMethodCall {
                invalid_lhs,
                resource_method_name: function_call_name,
            } => RibTypeError {
                cause: format!("invalid resource method call: `{}`", function_call_name),
                expr: invalid_lhs,
                immediate_parent: None,
                additional_error_details: vec![],
                help_messages: vec![],
            },
            FunctionCallError::InvalidGenericTypeParameter {
                generic_type_parameter,
                message,
            } => RibTypeError {
                cause: "invalid type parameter".to_string(),
                expr: Expr::literal(generic_type_parameter),
                immediate_parent: None,
                additional_error_details: vec![message],
                help_messages: vec![],
            },

            FunctionCallError::ArgumentSizeMisMatch {
                function_name,
                expr,
                expected,
                provided,
            } => RibTypeError {
                cause: format!(
                    "invalid argument size for function `{}`. expected {} arguments, found {}",
                    function_name, expected, provided
                ),
                expr,
                immediate_parent: None,
                additional_error_details: vec![],
                help_messages: vec![],
            },
        }
    }
}

impl From<InvalidWorkerName> for RibTypeError {
    fn from(value: InvalidWorkerName) -> Self {
        RibTypeError {
            cause: value.message,
            expr: value.worker_name_expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![],
        }
    }
}

impl From<ExhaustivePatternMatchError> for RibTypeError {
    fn from(value: ExhaustivePatternMatchError) -> Self {
        let expr = match &value {
            ExhaustivePatternMatchError::MissingConstructors { predicate, .. }
            | ExhaustivePatternMatchError::DeadCode { predicate, .. } => predicate.clone(),
        };

        let cause = match value {
            ExhaustivePatternMatchError::MissingConstructors {
                missing_constructors,
                ..
            } => {
                format!(
                    "non-exhaustive pattern match: the following patterns are not covered: `{}`",
                    missing_constructors.join(", ")
                )
            }
            ExhaustivePatternMatchError::DeadCode {
                dead_pattern,
                cause,
                ..
            } => {
                format!("dead code detected, pattern `{}` is unreachable due to the existence of the pattern `{}` prior to it", dead_pattern, cause)
            }
        };

        RibTypeError {
            cause,
            expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: vec![
                "to ensure a complete match, add missing patterns or use wildcard (`_`)"
                    .to_string(),
            ],
        }
    }
}

impl From<AmbiguousTypeError> for RibTypeError {
    fn from(value: AmbiguousTypeError) -> Self {
        let cause = format!(
            "ambiguous types: {}",
            value
                .ambiguous_types
                .iter()
                .map(|t| format!("`{}`", t))
                .collect::<Vec<String>>()
                .join(", ")
        );

        RibTypeError {
            cause,
            expr: value.expr,
            immediate_parent: None,
            additional_error_details: value.additional_error_details,
            help_messages: vec![],
        }
    }
}

impl From<InvalidPatternMatchError> for RibTypeError {
    fn from(value: InvalidPatternMatchError) -> Self {
        let (cause, expr) = match &value {
            InvalidPatternMatchError::ConstructorMismatch {
                predicate_expr,
                constructor_name,
                ..
            } => {
                (
                    format!(
                        "invalid pattern match: cannot match to constructor `{}`",
                        constructor_name
                    ),
                    predicate_expr,
                )
            }
            InvalidPatternMatchError::ArgSizeMismatch {
                predicate_expr,
                expected_arg_size,
                actual_arg_size,
                constructor_name,
                ..
            } => {
                (
                    format!(
                        "invalid pattern match: missing arguments in constructor `{}`. expected {} arguments, found {}",
                        constructor_name, expected_arg_size, actual_arg_size
                    ),
                    predicate_expr,
                )
            }
        };

        let immediate_parent = match &value {
            InvalidPatternMatchError::ConstructorMismatch { match_expr, .. } => Some(match_expr),
            InvalidPatternMatchError::ArgSizeMismatch { match_expr, .. } => Some(match_expr),
        };

        RibTypeError {
            cause,
            expr: expr.clone(),
            immediate_parent: immediate_parent.cloned(),
            additional_error_details: vec![],
            help_messages: vec![],
        }
    }
}

impl From<CustomError> for RibTypeError {
    fn from(value: CustomError) -> Self {
        RibTypeError {
            cause: value.message,
            expr: value.expr,
            immediate_parent: None,
            additional_error_details: vec![],
            help_messages: value.help_message,
        }
    }
}
