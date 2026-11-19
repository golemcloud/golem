// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::{PathSegmentType, QueryOrHeaderType};
use golem_common::schema::SchemaValue;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum HttpParameterError {
    #[error("Failed parsing value; Provided: {value}; Expected type: {expected}")]
    ValueParsingFailed {
        value: String,
        expected: &'static str,
    },
    #[error("Expected {expected} values to be provided, but found none")]
    MissingValue { expected: &'static str },
    #[error("Expected {expected} values to be provided, but found too many")]
    TooManyValues { expected: &'static str },
    #[error("Invalid input parameter '{name}': {error}")]
    InvalidParameter { name: String, error: String },
}

pub const SCALAR_MAP_ERROR: &str = "maps require name[key] entries, not a scalar value";

/// Recognize one decoded deepObject entry, without admitting nested brackets.
pub fn map_entry_key<'a>(name: &str, wire_name: &'a str) -> Option<&'a str> {
    let key = wire_name
        .strip_prefix(name)?
        .strip_prefix('[')?
        .strip_suffix(']')?;
    (!key.is_empty() && !key.contains(['[', ']'])).then_some(key)
}

/// Parse query values separately from headers so collection absence and map
/// namespaces follow the URL binding contract without changing header parsing.
pub fn parse_query_parameter(
    name: &str,
    query: &HashMap<String, Vec<String>>,
    ty: &QueryOrHeaderType,
) -> Result<SchemaValue, HttpParameterError> {
    let values = query.get(name).map(Vec::as_slice).unwrap_or_default();
    match ty {
        QueryOrHeaderType::Option { inner, .. } => {
            let present = if inner.is_map() {
                query.contains_key(name)
                    || query.keys().any(|key| map_entry_key(name, key).is_some())
            } else {
                !values.is_empty()
            };
            Ok(SchemaValue::Option {
                inner: if present {
                    Some(Box::new(parse_query_parameter(name, query, inner)?))
                } else {
                    None
                },
            })
        }
        QueryOrHeaderType::Map { key, value } => {
            if query.contains_key(name) {
                return Err(HttpParameterError::InvalidParameter {
                    name: name.into(),
                    error: SCALAR_MAP_ERROR.into(),
                });
            }
            let mut entries: Vec<(SchemaValue, SchemaValue)> = Vec::new();
            let mut wire_entries: Vec<_> = query
                .iter()
                .filter_map(|(wire_name, values)| {
                    map_entry_key(name, wire_name).map(|key| (key, values))
                })
                .collect();
            // Invocation retry matching compares serialized map entries in order.
            wire_entries.sort_unstable_by_key(|(key, _)| *key);
            for (raw_key, values) in wire_entries {
                if values.len() != 1 {
                    return Err(HttpParameterError::TooManyValues {
                        expected: "one value per map key",
                    });
                }
                let parsed_key = parse_path_segment_value(raw_key.into(), key)?;
                if entries
                    .iter()
                    .any(|(k, _)| *k == parsed_key || k.bitwise_eq(&parsed_key))
                {
                    return Err(HttpParameterError::InvalidParameter {
                        name: name.into(),
                        error: "duplicate parsed map key".into(),
                    });
                }
                entries.push((
                    parsed_key,
                    parse_path_segment_value(values[0].clone(), value)?,
                ));
            }
            if entries.is_empty() {
                return Err(HttpParameterError::MissingValue {
                    expected: "map entries",
                });
            }
            Ok(SchemaValue::Map { entries })
        }
        QueryOrHeaderType::List { .. } if values.is_empty() => {
            Err(HttpParameterError::MissingValue {
                expected: "list values",
            })
        }
        _ => parse_query_or_header_value(values, ty),
    }
}

/// Parse a single HTTP path segment (or query/header scalar) into a
/// schema-native value according to its declared scalar type.
pub fn parse_path_segment_value(
    value: String,
    r#type: &PathSegmentType,
) -> Result<SchemaValue, HttpParameterError> {
    match r#type {
        PathSegmentType::Str => Ok(SchemaValue::String(value)),
        PathSegmentType::Chr => {
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(SchemaValue::Char(c)),
                _ => Err(HttpParameterError::ValueParsingFailed {
                    value,
                    expected: "char",
                }),
            }
        }
        PathSegmentType::F64 => value.parse::<f64>().map(SchemaValue::F64).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "f64",
            }
        }),
        PathSegmentType::F32 => value.parse::<f32>().map(SchemaValue::F32).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "f32",
            }
        }),
        PathSegmentType::U64 => value.parse::<u64>().map(SchemaValue::U64).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "u64",
            }
        }),
        PathSegmentType::S64 => value.parse::<i64>().map(SchemaValue::S64).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "i64",
            }
        }),
        PathSegmentType::U32 => value.parse::<u32>().map(SchemaValue::U32).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "u32",
            }
        }),
        PathSegmentType::S32 => value.parse::<i32>().map(SchemaValue::S32).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "i32",
            }
        }),
        PathSegmentType::U16 => value.parse::<u16>().map(SchemaValue::U16).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "u16",
            }
        }),
        PathSegmentType::S16 => value.parse::<i16>().map(SchemaValue::S16).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "i16",
            }
        }),
        PathSegmentType::U8 => value.parse::<u8>().map(SchemaValue::U8).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "u8",
            }
        }),
        PathSegmentType::S8 => value.parse::<i8>().map(SchemaValue::S8).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "i8",
            }
        }),
        PathSegmentType::Bool => value.parse::<bool>().map(SchemaValue::Bool).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "bool",
            }
        }),
        PathSegmentType::Uuid => value.parse().map(SchemaValue::Uuid).map_err(|_| {
            HttpParameterError::ValueParsingFailed {
                value,
                expected: "uuid",
            }
        }),
        PathSegmentType::Enum(inner) => {
            let case_index = inner.iter().position(|c| *c == value).ok_or_else(|| {
                HttpParameterError::ValueParsingFailed {
                    value,
                    expected: "enum variant",
                }
            })?;
            Ok(SchemaValue::Enum {
                case: case_index
                    .try_into()
                    .expect("could not convert usize to u32"),
            })
        }
    }
}

/// Parse the HTTP values bound to a query parameter or header into a
/// schema-native value according to its declared type.
pub fn parse_query_or_header_value(
    values: &[String],
    r#type: &QueryOrHeaderType,
) -> Result<SchemaValue, HttpParameterError> {
    match r#type {
        QueryOrHeaderType::Primitive(inner) => {
            if values.len() > 1 {
                return Err(HttpParameterError::TooManyValues {
                    expected: "single value",
                });
            }
            let value = values.first().ok_or(HttpParameterError::MissingValue {
                expected: "single value",
            })?;
            parse_path_segment_value(value.clone(), inner)
        }
        QueryOrHeaderType::Option { inner, .. } => {
            if values.is_empty() {
                Ok(SchemaValue::Option { inner: None })
            } else {
                Ok(SchemaValue::Option {
                    inner: Some(Box::new(parse_query_or_header_value(values, inner)?)),
                })
            }
        }
        QueryOrHeaderType::List { inner, .. } => {
            let mut parsed_values = Vec::with_capacity(values.len());
            for value in values {
                parsed_values.push(parse_path_segment_value(value.clone(), inner)?);
            }
            Ok(SchemaValue::List {
                elements: parsed_values,
            })
        }
        QueryOrHeaderType::Map { .. } => Err(HttpParameterError::MissingValue {
            expected: "deepObject query entries",
        }),
    }
}
