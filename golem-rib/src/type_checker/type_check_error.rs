use std::fmt::Display;
use golem_wasm_ast::analysis::AnalysedType;
use crate::{InferredType, TypeName};

#[derive(Clone, Debug)]
pub struct TypeCheckError {
    pub details: Vec<String>,
    pub field_path: Vec<PathElem>,
    pub expected_type: AnalysedType,
    pub actual_type: InferredType,
}

#[derive(Clone, Debug)]
enum PathElem {
    Field(String),
    Index(usize),
}

impl TypeCheckError {
    pub fn with_message(&self, message: String) -> TypeCheckError {
        let mut new_messages: TypeCheckError = self.clone();
        new_messages.details.push(message);
        new_messages
    }

    pub fn with_field_name(&self, field_name: String) -> TypeCheckError {
        let mut new_messages: TypeCheckError = self.clone();
        new_messages.field_path.push(PathElem::Field(field_name));
        new_messages
    }

    pub fn with_index(&self, index: usize) -> TypeCheckError {
        let mut new_messages: TypeCheckError = self.clone();
        new_messages.field_path.push(PathElem::Index(index));
        new_messages
    }

    pub fn new(
        expected_type: AnalysedType,
        actual_type: InferredType,
        message: Option<String>,
    ) -> Self {
        TypeCheckError {
            details: message.map(|x| vec![x]).unwrap_or_default(),
            field_path: vec![],
            expected_type,
            actual_type,
        }
    }
}

impl Display for TypeCheckError {
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