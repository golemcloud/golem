use futures_util::stream::iter;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::{TypedList, TypedRecord, TypedTuple};

use crate::evaluator::path::{Path, PathComponent};

pub trait Getter<T> {
    fn get(&self, key: &Path) -> Result<T, GetError>;
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum GetError {
    #[error("Key not found: {0}")]
    KeyNotFound(String),
    #[error("Index not found: {0}")]
    IndexNotFound(usize),
    #[error("Not a record: key_name: {key_name}, original_value: {found}")]
    NotRecord { key_name: String, found: String },
    #[error("Not an array: index: {index}, original_value: {found}")]
    NotArray { index: usize, found: String },
}

impl Getter<TypeAnnotatedValue> for TypeAnnotatedValue {
    fn get(&self, key: &Path) -> Result<TypeAnnotatedValue, GetError> {
        let size = key.0.len();
        fn go(
            type_annotated_value: &TypeAnnotatedValue,
            paths: Vec<PathComponent>,
            index: usize,
            size: usize,
        ) -> Result<TypeAnnotatedValue, GetError> {
            if index < size {
                match &paths[index] {
                    PathComponent::KeyName(key) => match type_annotated_value {
                        TypeAnnotatedValue::Record(TypedRecord { value, .. }) => {
                            let new_value =
                                value.into_iter().find(|name_value| name_value.name == key.0)
                                    .map(|v| v.value.clone().map(|vv| vv.type_annotated_value)).flatten().flatten();

                            match new_value {
                                Some(new_value) => go(&new_value, paths, index + 1, size),
                                _ => Err(GetError::KeyNotFound(key.0.clone())),
                            }
                        }
                        _ => Err(GetError::NotRecord {
                            key_name: key.0.clone(),
                            found: get_json_from_typed_value(&type_annotated_value.clone())
                                .to_string(),
                        }),
                    },
                    PathComponent::Index(value_index) => match get_array(type_annotated_value) {
                        Some(type_values) => {
                            let new_value = type_values.get(value_index.0);
                            match new_value {
                                Some(new_value) => go(new_value, paths, index + 1, size),
                                None => Err(GetError::IndexNotFound(value_index.0)),
                            }
                        }
                        None => Err(GetError::NotArray {
                            index: value_index.0,
                            found: get_json_from_typed_value(type_annotated_value).to_string(),
                        }),
                    },
                }
            } else {
                Ok(type_annotated_value.clone())
            }
        }

        go(self, key.0.clone(), 0, size)
    }
}

fn get_array(value: &TypeAnnotatedValue) -> Option<Vec<TypeAnnotatedValue>> {
    match value {
        TypeAnnotatedValue::List (TypedList { values, .. }) => {
            let vec = values.into_iter().filter_map(|v| v.type_annotated_value).collect::<Vec<_>>();
            Some(vec)
        },
        TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) =>{
            let vec = value.into_iter().filter_map(|v| v.type_annotated_value).collect::<Vec<_>>();
            Some(vec)
        }
        _ => None,
    }
}

pub trait GetterExt<T> {
    fn get_optional(&self, key: &Path) -> Option<T>;
}

impl<T: Getter<T>> GetterExt<T> for T {
    fn get_optional(&self, key: &Path) -> Option<T> {
        match self.get(key) {
            Ok(value) => Some(value),
            Err(_) => None,
        }
    }
}
