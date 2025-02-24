use crate::type_inference::kind::{GetTypeKind, TypeKind};
use crate::{Expr, InferredType, Path, PathElem};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt;
use std::fmt::Display;

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

#[derive(Clone, Debug)]
pub struct TypeMismatchError {
    pub expr_with_wrong_type: Expr,
    pub parent_expr: Option<Expr>,
    pub expected_type: ExpectedType,
    pub actual_type: ActualType,
    pub field_path: Path,
    pub additional_error_detail: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum ExpectedType {
    AnalysedType(AnalysedType),
    Kind(TypeKind),
}

// If the actual type is not fully known but only a hint through TypeKind
#[derive(Clone, Debug)]
pub enum ActualType {
    Inferred(InferredType),
    Kind(TypeKind),
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

    pub fn with_actual_inferred_type(
        expr: &Expr,
        parent_expr: Option<&Expr>,
        expected_type: AnalysedType,
        actual_type: InferredType,
    ) -> Self {
        TypeMismatchError {
            expr_with_wrong_type: expr.clone(),
            parent_expr: parent_expr.cloned(),
            expected_type: ExpectedType::AnalysedType(expected_type),
            actual_type: ActualType::Inferred(actual_type),
            field_path: Path::default(),
            additional_error_detail: Vec::new(),
        }
    }

    pub fn with_actual_type_kind(
        expr: &Expr,
        parent_expr: Option<&Expr>,
        expected_type: AnalysedType,
        actual_type: &TypeKind,
    ) -> Self {
        TypeMismatchError {
            expr_with_wrong_type: expr.clone(),
            parent_expr: parent_expr.cloned(),
            expected_type: ExpectedType::AnalysedType(expected_type),
            actual_type: ActualType::Kind(actual_type.clone()),
            field_path: Path::default(),
            additional_error_detail: Vec::new(),
        }
    }
}

pub struct MultipleUnResolvedTypesError(pub Vec<UnResolvedTypesError>);

#[derive(Clone)]
pub struct UnResolvedTypesError {
    pub unresolved_expr: Expr,
    pub parent_expr: Option<Expr>,
    pub additional_messages: Vec<String>,
    pub help_messages: Vec<String>,
    pub path: Path,
}

impl UnResolvedTypesError {
    pub fn from(expr: &Expr, parent_expr: Option<Expr>) -> Self {
        let unresolved_types = UnResolvedTypesError {
            unresolved_expr: expr.clone(),
            additional_messages: Vec::new(),
            parent_expr: parent_expr.clone(),
            help_messages: Vec::new(),
            path: Path::default(),
        };

        unresolved_types.with_default_help_messages()
    }

    pub fn with_default_help_messages(&self) -> Self {
        self.with_help_message(
            "consider specifying the type explicitly. Examples: `1: u64`, `person.age: u8`",
        )
        .with_help_message(
            "or specify the type in let binding. Example: let numbers: list<u8> = [1, 2, 3]",
        )
    }

    pub fn with_parent_expr(&self, expr: &Expr) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.parent_expr = Some(expr.clone());
        unresolved_error
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

impl Display for UnResolvedTypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.unresolved_expr.source_span();

        writeln!(
            f,
            "cannot determine the type of the following rib expression found at line {}, column {}",
            span.start_line(),
            span.start_column()
        )?;

        writeln!(f, "`{}`", self.unresolved_expr)?;

        if let Some(parent) = &self.parent_expr {
            writeln!(f, "found within:")?;
            writeln!(f, "`{}`", parent)?;
        }

        if !self.additional_messages.is_empty() {
            for message in &self.additional_messages {
                writeln!(f, "{}", message)?;
            }
        }

        Ok(())
    }
}

pub enum FunctionCallError {
    InvalidFunctionCall {
        function_name: String,
        expr: Expr,
        message: String,
    },
    TypeMisMatch {
        function_name: String,
        argument: Expr,
        error: TypeMismatchError,
    },
    MissingRecordFields {
        function_name: String,
        argument: Expr,
        missing_fields: Vec<Path>,
    },
    UnResolvedTypes {
        function_name: String,
        argument: Expr,
        unresolved_error: UnResolvedTypesError,
        expected_type: AnalysedType,
    },

    InvalidResourceMethodCall {
        resource_method_name: String,
        invalid_lhs: Expr,
    },

    InvalidGenericTypeParameter {
        generic_type_parameter: String,
        expr: Expr,
        message: String,
    },

    ArgumentSizeMisMatch {
        function_name: String,
        expr: Expr,
        expected: usize,
        provided: usize,
    },
}

pub struct InvalidWorkerName {
    pub worker_name_expr: Expr,
    pub message: String,
}

#[derive(Clone)]
pub struct CustomError {
    pub expr: Expr,
    pub message: String,
    pub help_message: Vec<String>,
}

impl CustomError {
    pub fn new(expr: &Expr, message: impl AsRef<str>) -> CustomError {
        CustomError {
            expr: expr.clone(),
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
