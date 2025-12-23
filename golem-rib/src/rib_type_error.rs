// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::rib_source_span::SourceSpan;
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
#[derive(Debug, Clone, PartialEq)]
pub struct RibTypeError {
    pub cause: String,
    pub expr: Option<Expr>,
    pub additional_error_details: Vec<String>,
    pub help_messages: Vec<String>,
    pub source_span: SourceSpan,
}

impl Display for RibTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.source_span.clone();

        writeln!(
            f,
            "error in the following rib found at line {}, column {}",
            span.start_line(),
            span.start_column()
        )?;

        if let Some(expr) = &self.expr {
            writeln!(f, "`{expr}`")?;
        }

        writeln!(f, "cause: {}", self.cause)?;

        if !self.additional_error_details.is_empty() {
            for message in &self.additional_error_details {
                writeln!(f, "{message}")?;
            }
        }

        if !self.help_messages.is_empty() {
            for message in &self.help_messages {
                writeln!(f, "help: {message}")?;
            }
        }

        Ok(())
    }
}

impl RibTypeError {
    pub fn from_rib_type_error_internal(
        rib_type_error: RibTypeErrorInternal,
        rib_program: Expr,
    ) -> RibTypeError {
        let wrong_expr = rib_program.lookup(&rib_type_error.source_span);

        RibTypeError {
            cause: rib_type_error.cause,
            expr: wrong_expr,
            additional_error_details: rib_type_error.additional_error_details,
            help_messages: rib_type_error.help_messages,
            source_span: rib_type_error.source_span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RibTypeErrorInternal {
    pub cause: String,
    pub source_span: SourceSpan,
    pub additional_error_details: Vec<String>,
    pub help_messages: Vec<String>,
}

impl RibTypeErrorInternal {
    pub fn with_additional_error_detail(&self, detail: &str) -> RibTypeErrorInternal {
        let mut error = self.clone();
        error.additional_error_details.push(detail.to_string());
        error
    }

    // expr is the original full program, to be used to look up
    // the source span
    pub fn printable(&self, expr: &Expr) -> String {
        let source_span = &self.source_span;

        let wrong_expr = expr.lookup(source_span);

        let mut error_message = format!(
            "error in the following rib found at line {}, column {}:\n",
            source_span.start_line(),
            source_span.start_column()
        );

        if let Some(wrong_expr) = wrong_expr {
            error_message.push_str(&format!("`{wrong_expr}`\n"));
        }

        error_message.push_str(&format!("cause: {}\n", self.cause));

        if !self.additional_error_details.is_empty() {
            for message in &self.additional_error_details {
                error_message.push_str(&format!("{message}\n"));
            }
        }

        if !self.help_messages.is_empty() {
            for message in &self.help_messages {
                error_message.push_str(&format!("help: {message}\n"));
            }
        }

        error_message
    }
}

impl From<UnResolvedTypesError> for RibTypeErrorInternal {
    fn from(value: UnResolvedTypesError) -> Self {
        let mut rib_compilation_error = RibTypeErrorInternal {
            cause: "cannot determine the type".to_string(),
            additional_error_details: value.additional_messages,
            help_messages: value.help_messages,
            source_span: value.source_span,
        };

        if !value.path.is_empty() {
            rib_compilation_error
                .additional_error_details
                .push(format!("unresolved type at path: `{}`", value.path));
        }

        rib_compilation_error
    }
}

impl From<TypeUnificationError> for RibTypeErrorInternal {
    fn from(value: TypeUnificationError) -> Self {
        match value {
            TypeUnificationError::TypeMismatchError { error } => error.into(),
            TypeUnificationError::UnresolvedTypesError { error } => error.into(),
        }
    }
}

impl From<TypeMismatchError> for RibTypeErrorInternal {
    fn from(value: TypeMismatchError) -> Self {
        let expected = match value.expected_type {
            ExpectedType::AnalysedType(analysed_type) => TypeName::try_from(analysed_type)
                .map(|x| format!("expected {x}"))
                .ok(),
            ExpectedType::Hint(kind) => Some(format!("expected {kind}")),
            ExpectedType::InferredType(type_name) => {
                Some(format!("expected {}", type_name.printable()))
            }
        };

        let actual = match value.actual_type {
            ActualType::Hint(type_kind) => Some(format!("found {type_kind}")),
            ActualType::Inferred(inferred_type) => TypeName::try_from(inferred_type)
                .map(|x| format!("found {x}"))
                .ok(),
        };

        let cause_suffix = match (expected, actual) {
            (Some(expected), Some(actual)) => format!("{expected}, {actual}"),
            (Some(expected), None) => expected.to_string(),
            _ => "".to_string(),
        };

        let cause = if !value.field_path.is_empty() {
            format!(
                "type mismatch at path: `{}`. {}",
                value.field_path, cause_suffix
            )
        } else {
            format!("type mismatch. {cause_suffix}")
        };

        RibTypeErrorInternal {
            cause,
            source_span: value.source_span,
            additional_error_details: value.additional_error_detail,
            help_messages: vec![],
        }
    }
}

impl From<FunctionCallError> for RibTypeErrorInternal {
    fn from(value: FunctionCallError) -> Self {
        match value {
            FunctionCallError::InvalidFunctionCall {
                function_name,
                source_span,
                message,
            } => RibTypeErrorInternal {
                cause: format!("invalid function call `{function_name}`"),
                source_span,
                additional_error_details: vec![message],
                help_messages: vec![],
            },
            FunctionCallError::TypeMisMatch {
                function_name,
                error,
                ..
            } => {
                let mut original_compilation: RibTypeErrorInternal = error.into();

                let error_detail = format!("invalid argument to the function `{function_name}`");

                original_compilation
                    .additional_error_details
                    .push(error_detail);

                original_compilation
            }
            FunctionCallError::MissingRecordFields {
                function_name: call_type,
                missing_fields,
                argument_source_span,
            } => {
                let missing_fields = missing_fields
                    .iter()
                    .map(|path| path.to_string())
                    .collect::<Vec<String>>()
                    .join(", ");

                RibTypeErrorInternal {
                    cause: format!(
                        "invalid argument to the function `{call_type}`.  missing field(s) in record: `{missing_fields}`"
                    ),
                    source_span: argument_source_span,
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
                    .map(|t| format!("expected {t}"))
                    .unwrap_or_default();

                let mut rib_compilation_error = RibTypeErrorInternal::from(unresolved_error);

                rib_compilation_error
                    .additional_error_details
                    .push(format!("invalid argument to `{call_type}`. {expected}"));

                rib_compilation_error
            }
            FunctionCallError::InvalidResourceMethodCall {
                invalid_lhs_source_span,
                resource_method_name: function_call_name,
            } => RibTypeErrorInternal {
                cause: format!("invalid resource method call: `{function_call_name}`"),
                source_span: invalid_lhs_source_span,
                additional_error_details: vec![],
                help_messages: vec![],
            },
            FunctionCallError::InvalidGenericTypeParameter {
                generic_type_parameter,
                message,
                source_span,
            } => RibTypeErrorInternal {
                cause: format!(
                    "invalid generic type parameter: `{generic_type_parameter}`"
                ),
                source_span,
                additional_error_details: vec![message],
                help_messages: vec![],
            },

            FunctionCallError::ArgumentSizeMisMatch {
                function_name,
                source_span: argument_source_span,
                expected,
                provided,
            } => RibTypeErrorInternal {
                cause: format!(
                    "invalid argument size for function `{function_name}`. expected {expected} arguments, found {provided}"
                ),
                source_span: argument_source_span,
                additional_error_details: vec![],
                help_messages: vec![],
            },
        }
    }
}

impl From<InvalidWorkerName> for RibTypeErrorInternal {
    fn from(value: InvalidWorkerName) -> Self {
        RibTypeErrorInternal {
            cause: value.message,
            source_span: value.worker_name_source_span,
            additional_error_details: vec![],
            help_messages: vec![],
        }
    }
}

impl From<ExhaustivePatternMatchError> for RibTypeErrorInternal {
    fn from(value: ExhaustivePatternMatchError) -> Self {
        let source_span = match &value {
            ExhaustivePatternMatchError::MissingConstructors {
                predicate_source_span,
                ..
            }
            | ExhaustivePatternMatchError::DeadCode {
                predicate_source_span,
                ..
            } => predicate_source_span.clone(),
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
                format!("dead code detected, pattern `{dead_pattern}` is unreachable due to the existence of the pattern `{cause}` prior to it")
            }
        };

        RibTypeErrorInternal {
            cause,
            source_span,
            additional_error_details: vec![],
            help_messages: vec![
                "to ensure a complete match, add missing patterns or use wildcard (`_`)"
                    .to_string(),
            ],
        }
    }
}

impl From<AmbiguousTypeError> for RibTypeErrorInternal {
    fn from(value: AmbiguousTypeError) -> Self {
        let cause = format!(
            "ambiguous types: {}",
            value
                .ambiguous_types
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<String>>()
                .join(", ")
        );

        RibTypeErrorInternal {
            cause,
            source_span: value.ambiguous_expr_source_span,
            additional_error_details: value.additional_error_details,
            help_messages: vec![],
        }
    }
}

impl From<InvalidPatternMatchError> for RibTypeErrorInternal {
    fn from(value: InvalidPatternMatchError) -> Self {
        let (cause, source_span) = match &value {
            InvalidPatternMatchError::ConstructorMismatch {
                match_expr_source_span,
                constructor_name,
                ..
            } => {
                (
                    format!(
                        "invalid pattern match: cannot match to constructor `{constructor_name}`"
                    ),
                    match_expr_source_span,
                )
            }
            InvalidPatternMatchError::ArgSizeMismatch {
                match_expr_source_span,
                expected_arg_size,
                actual_arg_size,
                constructor_name,
                ..
            } => {
                (
                    format!(
                        "invalid pattern match: missing arguments in constructor `{constructor_name}`. expected {expected_arg_size} arguments, found {actual_arg_size}"
                    ),
                    match_expr_source_span,
                )
            }
        };

        RibTypeErrorInternal {
            cause,
            source_span: source_span.clone(),
            additional_error_details: vec![],
            help_messages: vec![],
        }
    }
}

impl From<CustomError> for RibTypeErrorInternal {
    fn from(value: CustomError) -> Self {
        RibTypeErrorInternal {
            cause: value.message,
            source_span: value.source_span,
            additional_error_details: vec![],
            help_messages: value.help_message,
        }
    }
}
