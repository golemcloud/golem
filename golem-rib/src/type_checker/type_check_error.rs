use crate::type_checker::{Path, PathElem, PathType};
use crate::{Expr, InferredType, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt;
use std::fmt::Display;

#[derive(Clone, Debug)]
pub struct UnResolvedTypesError {
    pub unresolved_expr: Expr,
    pub unresolved_path: Path,
    pub additional_messages: Vec<String>,
    pub parent_expr: Option<Expr>,
}

impl UnResolvedTypesError {
    pub fn new(expr: &Expr) -> Self {
        UnResolvedTypesError {
            unresolved_expr: expr.clone(),
            unresolved_path: Path::default(),
            additional_messages: Vec::new(),
            parent_expr: None,
        }
    }

    pub fn with_parent_expr(&self, expr: &Expr) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.parent_expr = Some(expr.clone());
        unresolved_error
    }

    pub fn with_additional_message(&self, message: impl AsRef<str>) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .additional_messages
            .push(message.as_ref().to_string());
        unresolved_error
    }

    pub fn at_field(&self, field_name: String) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .unresolved_path
            .push_front(PathElem::Field(field_name));
        unresolved_error
    }

    pub fn at_index(&self, index: usize) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .unresolved_path
            .push_front(PathElem::Index(index));
        unresolved_error
    }
}

impl Display for UnResolvedTypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path_type = PathType::from_path(&self.unresolved_path);
        let parent_expr_opt = self.parent_expr.clone();

        match path_type {
            Some(PathType::RecordPath(path)) => {
                write!(
                    f,
                    "Unable to determine the type of `{}` in the record at path `{}`",
                    self.unresolved_expr, path
                )
            }
            Some(PathType::IndexPath(path)) => {
                write!(
                    f,
                    "Unable to determine the type of `{}` at index `{}`",
                    self.unresolved_expr, path
                )
            }
            None => {
                write!(
                    f,
                    "Unable to determine the type of `{}`",
                    self.unresolved_expr
                )?;

                if let Some(parent) = parent_expr_opt {
                    write!(f, " in {}", parent)?;
                }

                Ok(())
            }
        }?;

        if !self.additional_messages.is_empty() {
            for message in &self.additional_messages {
                write!(f, ". {}", message)?;
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct TypeMismatchError {
    pub field_path: Path,
    pub expected_type: AnalysedType,
    pub actual_type: InferredType,
}

impl TypeMismatchError {
    pub fn updated_expected_type(&self, expected_type: &AnalysedType) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error.expected_type = expected_type.clone();
        mismatch_error
    }

    pub fn at_field(&self, field_name: String) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error
            .field_path
            .push_front(PathElem::Field(field_name));
        mismatch_error
    }

    pub fn at_index(&self, index: usize) -> TypeMismatchError {
        let mut new_messages: TypeMismatchError = self.clone();
        new_messages.field_path.push_front(PathElem::Index(index));
        new_messages
    }

    pub fn new(expected_type: AnalysedType, actual_type: InferredType) -> Self {
        TypeMismatchError {
            field_path: Path::default(),
            expected_type,
            actual_type,
        }
    }
}

impl Display for TypeMismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        let field_path = self.field_path.to_string();

        let expected_type = TypeName::try_from(self.expected_type.clone())
            .map(|x| x.to_string())
            .unwrap_or_default();

        let base_error = if field_path.is_empty() {
            format!("Type mismatch. Expected `{}`", &expected_type)
        } else {
            format!(
                "Type mismatch for `{}`. Expected `{}`",
                &field_path, &expected_type
            )
        };

        if self.actual_type.is_one_of() || self.actual_type.is_all_of() {
            write!(f, "{}", &base_error)
        } else {
            write!(f, "{}. Found `{:?}`", &base_error, self.actual_type)
        }
    }
}
