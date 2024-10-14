use crate::{Expr, InferredType, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt;
use std::fmt::Display;

#[derive(Clone, Debug)]
pub enum TypeCheckError {
    UnResolvedTypesError(UnResolvedTypesError),
    TypeMismatchError(TypeMismatchError),
}

impl TypeCheckError {
    pub fn unresolved_types_error(unresolved_type_error: UnResolvedTypesError) -> Self {
        TypeCheckError::UnResolvedTypesError(unresolved_type_error)
    }

    pub fn type_mismatch_error(type_mismatch_error: TypeMismatchError) -> Self {
        TypeCheckError::TypeMismatchError(type_mismatch_error)
    }
}

impl Display for TypeCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeCheckError::UnResolvedTypesError(e) => write!(f, "{}", e),
            TypeCheckError::TypeMismatchError(e) => write!(f, "{}", e),
        }
    }
}

#[derive(Clone, Debug)]
pub struct UnResolvedTypesError {
    pub expr: Expr,
    pub unresolved_path: Path,
    pub additional_messages: Vec<String>,
}

impl UnResolvedTypesError {
    pub fn new(expr: &Expr) -> Self {
        UnResolvedTypesError {
            expr: expr.clone(),
            unresolved_path: Path::default(),
            additional_messages: Vec::new(),
        }
    }

    pub fn add_message(&self, message: &str) -> Self {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .additional_messages
            .push(message.to_string());
        unresolved_error
    }

    pub fn at_field(&self, field_name: String) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .unresolved_path
            .push_back(PathElem::Field(field_name));
        unresolved_error
    }

    pub fn at_index(&self, index: usize) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error
            .unresolved_path
            .push_back(PathElem::Index(index));
        unresolved_error
    }
}

impl Display for UnResolvedTypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        let field_path = self.unresolved_path.to_string();
        if field_path.is_empty() {
            write!(f, "Cannot infer the type of: `{}`", self.expr)?;
        } else {
            write!(
                f,
                "Cannot infer the type of `{}` in `{}`",
                self.expr, field_path
            )?;
        }

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

#[derive(Clone, Debug, Default)]
pub struct Path(Vec<PathElem>);

impl Path {
    pub fn from_elem(elem: PathElem) -> Self {
        Path(vec![elem])
    }

    pub fn push_front(&mut self, elem: PathElem) {
        self.0.insert(0, elem);
    }

    pub fn push_back(&mut self, elem: PathElem) {
        self.0.push(elem);
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut is_first = true;
        for elem in &self.0 {
            match elem {
                PathElem::Field(name) => {
                    if is_first {
                        write!(f, "{}", name)?;
                        is_first = false;
                    } else {
                        write!(f, ".{}", name)?;
                    }
                }
                PathElem::Index(index) => {
                    if is_first {
                        write!(f, "index: {}", index)?;
                        is_first = false;
                    } else {
                        write!(f, "[{}]", index)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum PathElem {
    Field(String),
    Index(usize),
}
