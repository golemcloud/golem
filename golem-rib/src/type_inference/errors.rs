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
use crate::type_inference::type_hint::{GetTypeHint, TypeHint};
use crate::{Expr, InferredType, Path, PathElem};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt::Display;

#[derive(Debug, Clone)]
pub struct AmbiguousTypeError {
    pub ambiguous_expr_source_span: SourceSpan,
    pub ambiguous_types: Vec<String>,
    pub additional_error_details: Vec<String>,
}

impl AmbiguousTypeError {
    pub fn new(
        inferred_expr: &InferredType,
        source_span: &SourceSpan,
        expected: &TypeHint,
    ) -> AmbiguousTypeError {
        let actual_kind = inferred_expr.get_type_hint();
        match actual_kind {
            TypeHint::Ambiguous { possibilities } => {
                let possibilities = possibilities
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>();

                AmbiguousTypeError {
                    ambiguous_expr_source_span: source_span.clone(),
                    ambiguous_types: possibilities,
                    additional_error_details: vec![],
                }
            }
            _ => AmbiguousTypeError {
                ambiguous_expr_source_span: source_span.clone(),
                ambiguous_types: vec![expected.to_string(), inferred_expr.printable()],
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
        match_expr_source_span: SourceSpan,
        constructor_name: String,
    },
    ArgSizeMismatch {
        match_expr_source_span: SourceSpan,
        constructor_name: String,
        expected_arg_size: usize,
        actual_arg_size: usize,
    },
}

impl InvalidPatternMatchError {
    pub fn constructor_type_mismatch(
        match_expr_source_span: SourceSpan,
        constructor_name: &str,
    ) -> InvalidPatternMatchError {
        InvalidPatternMatchError::ConstructorMismatch {
            match_expr_source_span,
            constructor_name: constructor_name.to_string(),
        }
    }

    pub fn arg_size_mismatch(
        match_expr_source_span: SourceSpan,
        constructor_name: &str,
        expected_arg_size: usize,
        actual_arg_size: usize,
    ) -> InvalidPatternMatchError {
        InvalidPatternMatchError::ArgSizeMismatch {
            match_expr_source_span,
            expected_arg_size,
            actual_arg_size,
            constructor_name: constructor_name.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnificationError {}

#[derive(Clone, Debug)]
pub struct TypeMismatchError {
    pub source_span: SourceSpan,
    pub expected_type: ExpectedType,
    pub actual_type: ActualType,
    pub field_path: Path,
    pub additional_error_detail: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum ExpectedType {
    AnalysedType(AnalysedType),
    // If the expected type is not fully known yet but only a hint is available.
    // Example: when compiler cannot proceed unless it is a `record`, or `list` etc
    Hint(TypeHint),
    InferredType(InferredType),
}

#[derive(Clone, Debug)]
pub enum ActualType {
    Inferred(InferredType),
    // If the actual type is not fully known yet but only a hint is available
    Hint(TypeHint),
}

impl TypeMismatchError {
    pub fn updated_expected_type(&self, expected_type: &AnalysedType) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error.expected_type = ExpectedType::AnalysedType(expected_type.clone());
        mismatch_error
    }

    pub fn at_field(&self, field_name: String) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error
            .field_path
            .push_front(PathElem::Field(field_name));
        mismatch_error
    }
}

// A type unification can fail either due to a type mismatch or due to unresolved types
pub enum TypeUnificationError {
    TypeMismatchError { error: TypeMismatchError },
    UnresolvedTypesError { error: UnResolvedTypesError },
}

impl TypeUnificationError {
    pub fn unresolved_types_error(
        source_span: SourceSpan,
        additional_messages: Vec<String>,
    ) -> TypeUnificationError {
        TypeUnificationError::UnresolvedTypesError {
            error: UnResolvedTypesError {
                source_span,
                additional_messages,
                help_messages: vec![],
                path: Path::default(),
            },
        }
    }
    pub fn type_mismatch_error(
        source_span: SourceSpan,
        expected_type: InferredType,
        actual_type: InferredType,
        additional_error_detail: Vec<String>,
    ) -> TypeUnificationError {
        TypeUnificationError::TypeMismatchError {
            error: TypeMismatchError {
                source_span,
                expected_type: ExpectedType::InferredType(expected_type),
                actual_type: ActualType::Inferred(actual_type),
                field_path: Path::default(),
                additional_error_detail,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnResolvedTypesError {
    pub source_span: SourceSpan,
    pub help_messages: Vec<String>,
    pub path: Path,
    pub additional_messages: Vec<String>,
}

impl UnResolvedTypesError {
    pub fn from(source_span: SourceSpan) -> Self {
        let unresolved_types = UnResolvedTypesError {
            source_span,
            help_messages: Vec::new(),
            path: Path::default(),
            additional_messages: Vec::new(),
        };

        unresolved_types.with_default_help_messages()
    }

    pub fn with_default_help_messages(&self) -> Self {
        self.with_help_message(
            "try specifying the expected type explicitly",
        ).with_help_message(
            "if the issue persists, please review the script for potential type inconsistencies"
        )
    }

    pub fn with_additional_error_detail(&self, message: impl AsRef<str>) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .additional_messages
            .push(message.as_ref().to_string());
        unresolved_error
    }

    pub fn with_help_message(&self, message: impl AsRef<str>) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .help_messages
            .push(message.as_ref().to_string());

        unresolved_error
    }

    pub fn at_field(&self, field_name: String) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .path
            .push_front(PathElem::Field(field_name));
        unresolved_error
    }

    pub fn at_index(&self, index: usize) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.path.push_front(PathElem::Index(index));
        unresolved_error
    }
}

#[derive(Debug, Clone)]
pub enum FunctionCallError {
    InvalidFunctionCall {
        function_name: String,
        source_span: SourceSpan,
        message: String,
    },
    TypeMisMatch {
        function_name: String,
        argument_source_span: SourceSpan,
        error: TypeMismatchError,
    },
    MissingRecordFields {
        function_name: String,
        argument_source_span: SourceSpan,
        missing_fields: Vec<Path>,
    },
    UnResolvedTypes {
        function_name: String,
        source_span: SourceSpan,
        unresolved_error: UnResolvedTypesError,
        expected_type: AnalysedType,
    },

    InvalidResourceMethodCall {
        resource_method_name: String,
        invalid_lhs_source_span: SourceSpan,
    },

    InvalidGenericTypeParameter {
        generic_type_parameter: String,
        source_span: SourceSpan,
        message: String,
    },

    ArgumentSizeMisMatch {
        function_name: String,
        source_span: SourceSpan,
        expected: usize,
        provided: usize,
    },
}

impl FunctionCallError {
    pub fn invalid_function_call(
        function_name: &str,
        function_source_span: SourceSpan,
        message: impl AsRef<str>,
    ) -> FunctionCallError {
        FunctionCallError::InvalidFunctionCall {
            function_name: function_name.to_string(),
            source_span: function_source_span,
            message: message.as_ref().to_string(),
        }
    }
    pub fn invalid_generic_type_parameter(
        generic_type_parameter: &str,
        message: impl AsRef<str>,
        source_span: SourceSpan,
    ) -> FunctionCallError {
        FunctionCallError::InvalidGenericTypeParameter {
            generic_type_parameter: generic_type_parameter.to_string(),
            message: message.as_ref().to_string(),
            source_span,
        }
    }
}

pub struct InvalidWorkerName {
    pub worker_name_source_span: SourceSpan,
    pub message: String,
}

#[derive(Clone)]
pub struct CustomError {
    pub source_span: SourceSpan,
    pub message: String,
    pub help_message: Vec<String>,
}

impl CustomError {
    pub fn new(source_span: SourceSpan, message: impl AsRef<str>) -> CustomError {
        CustomError {
            source_span,
            message: message.as_ref().to_string(),
            help_message: Vec::new(),
        }
    }

    pub fn with_help_message(&self, message: impl AsRef<str>) -> CustomError {
        let mut custom_error: CustomError = self.clone();
        custom_error.help_message.push(message.as_ref().to_string());
        custom_error
    }
}
