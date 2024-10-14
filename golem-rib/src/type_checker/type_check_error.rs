use std::fmt;
use std::fmt::Display;
use golem_wasm_ast::analysis::AnalysedType;
use crate::{InferredType, TypeName};


#[derive(Clone, Debug)]
pub enum TypeCheckError {
    UnResolvedTypesError(UnResolvedTypesError),
    TypeMismatchError(TypeMismatchError),
}

impl TypeCheckError {
    pub fn unresolved_types_error(msg: String) -> Self {
        TypeCheckError::UnResolvedTypesError(UnResolvedTypesError(msg))
    }

    pub fn type_mismatch_error(
       type_mismatch_error: TypeMismatchError
    ) -> Self {
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
pub struct UnResolvedTypesError(pub String);

impl Display for UnResolvedTypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unresolved type: {}", self.0)
    }
}

#[derive(Clone, Debug)]
pub struct TypeMismatchError {
    pub field_path: Path,
    pub expected_type: AnalysedType,
    pub actual_type: InferredType,
}

#[derive(Clone, Debug)]
pub struct Path(Vec<PathElem>);

impl Path {
    fn push_front(&mut self, elem: PathElem) {
        self.0.insert(0, elem);
    }
}

impl Default for Path {
    fn default() -> Self {
        Path(Vec::new())
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
                    write!(f, "[{}]", index)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum PathElem {
    Field(String),
    Index(usize),
}

impl TypeMismatchError {
    pub fn at_field(&self, field_name: String) -> TypeMismatchError {
        let mut mismatch_error: TypeMismatchError = self.clone();
        mismatch_error.field_path.push_front(PathElem::Field(field_name));
        mismatch_error
    }

    pub fn at_index(&self, index: usize) -> TypeMismatchError {
        let mut new_messages: TypeMismatchError = self.clone();
        new_messages.field_path.push_front(PathElem::Index(index));
        new_messages
    }

    pub fn new(
        expected_type: AnalysedType,
        actual_type: InferredType,
    ) -> Self {
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
            format!("Type mismatch for `{}`. Expected `{}`", &field_path, &expected_type)
        };

        if self.actual_type.is_one_of() || self.actual_type.is_all_of() {
            write!(f, "{}", &base_error)
        } else {
            write!(
                f,
                "{}. Found `{:?}`",
                 &base_error, self.actual_type
            )
        }
    }
}