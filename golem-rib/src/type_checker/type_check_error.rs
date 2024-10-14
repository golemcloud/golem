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
        expected_type: AnalysedType,
        actual_type: InferredType,
    ) -> Self {
        TypeCheckError::TypeMismatchError(TypeMismatchError::new(
            expected_type,
            actual_type,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct UnResolvedTypesError(pub String);

#[derive(Clone, Debug)]
pub struct TypeMismatchError {
    pub field_path: Vec<PathElem>,
    pub expected_type: AnalysedType,
    pub actual_type: InferredType,
}

#[derive(Clone, Debug)]
enum PathElem {
    Field(String),
    Index(usize),
}

impl TypeMismatchError {
    pub fn at_field(&self, field_name: String) -> TypeMismatchError {
        let mut new_messages: TypeMismatchError = self.clone();
        new_messages.field_path.insert(0, PathElem::Field(field_name));
        new_messages
    }

    pub fn at_index(&self, index: usize) -> TypeMismatchError {
        let mut new_messages: TypeMismatchError = self.clone();
        new_messages.field_path.insert(0, PathElem::Index(index));
        new_messages
    }

    pub fn new(
        expected_type: AnalysedType,
        actual_type: InferredType,
    ) -> Self {
        TypeMismatchError {
            field_path: vec![],
            expected_type,
            actual_type,
        }
    }
}

impl Display for TypeMismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for detail in self.details.iter() {
            write!(f, "{}\n", detail)?;
        }

        let expected_type = TypeName::try_from(self.expected_type.clone())
            .map(|x| x.to_string())
            .unwrap_or_default();

        if self.actual_type.is_one_of() || self.actual_type.is_all_of() {
            write!(f, "Expected type `{}` ", &expected_type)
        } else {
            write!(
                f,
                "Expected type `{}`, got `{:?}`",
                &expected_type, self.actual_type
            )
        }
    }
}