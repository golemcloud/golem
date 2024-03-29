use crate::path::{KeyName, Path, PathComponent};
use golem_wasm_rpc::TypeAnnotatedValue;
use std::collections::HashMap;

pub trait Getter<T> {
    fn get(&self, key: &Path) -> Option<T>;
}

impl Getter<TypeAnnotatedValue> for TypeAnnotatedValue {
    fn get(&self, key: &Path) -> Option<TypeAnnotatedValue> {
        let size = key.0.len();
        fn go(
            type_annotated_value: &TypeAnnotatedValue,
            paths: Vec<PathComponent>,
            index: usize,
            size: usize,
        ) -> Option<TypeAnnotatedValue> {
            if index < size {
                match &paths[index] {
                    PathComponent::KeyName(key) => match type_annotated_value {
                        TypeAnnotatedValue::Record { value, .. } => {
                            let new_value = value.iter().find(|(k, _)| k == &key.0).map(|(_, v)| v);
                            match new_value {
                                Some(new_value) => go(new_value, paths, index + 1, size),
                                None => None,
                            }
                        }
                        _ => None,
                    },
                    PathComponent::Index(value_index) => match get_array(type_annotated_value) {
                        Some(type_values) => {
                            let new_value = type_values.get(value_index.0).map(|v| v);
                            match new_value {
                                Some(new_value) => go(new_value, paths, index + 1, size),
                                None => None,
                            }
                        }
                        None => None,
                    },
                }
            } else {
                Some(type_annotated_value.clone())
            }
        }

        go(self, key.0.clone(), 0, size)
    }
}

fn get_array<'a>(value: &TypeAnnotatedValue) -> Option<&'a Vec<TypeAnnotatedValue>> {
    match value {
        TypeAnnotatedValue::List { values, .. } => Some(values),
        TypeAnnotatedValue::Tuple { value, .. } => Some(value),

        _ => None,
    }
}
