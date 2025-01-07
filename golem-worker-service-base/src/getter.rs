// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::headers::ResolvedResponseHeaders;
use crate::path::{Path, PathComponent};
use golem_wasm_ast::analysis::{AnalysedType, TypeRecord};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::{TypedList, TypedRecord, TypedTuple};
use golem_wasm_rpc::{Value, ValueAndType};
use http::StatusCode;
use rib::GetLiteralValue;
use rib::LiteralValue;

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
    #[error("Internal error: {0}")]
    Internal(String),
}

// To deal with fields in a TypeAnnotatedValue (that's returned from golem-rib)
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
                            let new_value = value
                                .iter()
                                .find(|name_value| name_value.name == key.0)
                                .and_then(|v| v.value.clone().map(|vv| vv.type_annotated_value))
                                .flatten();

                            match new_value {
                                Some(new_value) => go(&new_value, paths, index + 1, size),
                                _ => Err(GetError::KeyNotFound(key.0.clone())),
                            }
                        }
                        _ => Err(GetError::NotRecord {
                            key_name: key.0.clone(),
                            found: type_annotated_value.to_json_value().to_string(),
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
                            found: type_annotated_value.to_json_value().to_string(),
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

impl Getter<ValueAndType> for ValueAndType {
    fn get(&self, key: &Path) -> Result<ValueAndType, GetError> {
        let tav: TypeAnnotatedValue = self
            .clone()
            .try_into()
            .map_err(|errs: Vec<String>| GetError::Internal(errs.join(", ")))?;
        let result = tav.get(key)?;
        result.try_into().map_err(GetError::Internal)
    }
}

fn get_array(value: &TypeAnnotatedValue) -> Option<Vec<TypeAnnotatedValue>> {
    match value {
        TypeAnnotatedValue::List(TypedList { values, .. }) => {
            let vec = values
                .iter()
                .filter_map(|v| v.clone().type_annotated_value)
                .collect::<Vec<_>>();
            Some(vec)
        }
        TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) => {
            let vec = value
                .iter()
                .filter_map(|v| v.clone().type_annotated_value)
                .collect::<Vec<_>>();
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

pub fn get_response_headers(
    field_values: &[Value],
    record: &TypeRecord,
) -> Result<Option<ResolvedResponseHeaders>, String> {
    match record
        .fields
        .iter()
        .position(|pair| &pair.name == "headers")
    {
        None => Ok(None),
        Some(field_position) => Ok(Some(ResolvedResponseHeaders::from_typed_value(
            ValueAndType::new(
                field_values[field_position].clone(),
                record.fields[field_position].typ.clone(),
            ),
        )?)),
    }
}

pub fn get_response_headers_or_default(
    value: &ValueAndType,
) -> Result<ResolvedResponseHeaders, String> {
    match value {
        ValueAndType {
            value: Value::Record(field_values),
            typ: AnalysedType::Record(record),
        } => get_response_headers(field_values, record).map(|headers| headers.unwrap_or_default()),
        _ => Ok(ResolvedResponseHeaders::default()),
    }
}

pub fn get_status_code(
    field_values: &[Value],
    record: &TypeRecord,
) -> Result<Option<StatusCode>, String> {
    match record
        .fields
        .iter()
        .position(|field| &field.name == "status")
    {
        None => Ok(None),
        Some(field_position) => Ok(Some(get_status_code_inner(ValueAndType::new(
            field_values[field_position].clone(),
            record.fields[field_position].typ.clone(),
        ))?)),
    }
}

pub fn get_status_code_or_ok(value: &ValueAndType) -> Result<StatusCode, String> {
    match value {
        ValueAndType {
            value: Value::Record(field_values),
            typ: AnalysedType::Record(record),
        } => get_status_code(field_values, record).map(|status| status.unwrap_or(StatusCode::OK)),
        _ => Ok(StatusCode::OK),
    }
}

fn get_status_code_inner(status_code: ValueAndType) -> Result<StatusCode, String> {
    let status_res: Result<u16, String> =
        match status_code.get_literal() {
            Some(LiteralValue::String(status_str)) => status_str.parse().map_err(|e| {
                format!(
                    "Invalid Status Code Expression. It is resolved to a string but not a number {}. Error: {}",
                    status_str, e
                )
            }),
            Some(LiteralValue::Num(number)) => number.to_string().parse().map_err(|e| {
                format!(
                    "Invalid Status Code Expression. It is resolved to a number but not a u16 {}. Error: {}",
                    number, e
                )
            }),
            _ => Err(format!(
                "Status Code Expression is evaluated to a complex value. It is resolved to {:?}",
                status_code.value
            ))
        };

    let status_u16 = status_res?;

    StatusCode::from_u16(status_u16).map_err(|e|
        format!(
            "Invalid Status Code. A valid status code cannot be formed from the evaluated status code expression {}. Error: {}",
            status_u16, e
        ))
}
