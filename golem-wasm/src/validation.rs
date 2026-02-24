// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::analysis::AnalysedType;
use crate::Value;

/// Validates that a `Value` is structurally compatible with the given `AnalysedType`.
///
/// This performs a recursive structural check: it verifies that the `Value` variant
/// matches the expected `AnalysedType` variant and that composite types (records, tuples,
/// lists, etc.) have the correct arity and recursively valid children.
///
/// This is intended to be used as a pre-invocation check to reject obviously malformed
/// inputs before they reach WASM execution.
pub fn validate_value_matches_type(value: &Value, expected: &AnalysedType) -> Result<(), String> {
    match (value, expected) {
        // Primitives
        (Value::Bool(_), AnalysedType::Bool(_))
        | (Value::U8(_), AnalysedType::U8(_))
        | (Value::U16(_), AnalysedType::U16(_))
        | (Value::U32(_), AnalysedType::U32(_))
        | (Value::U64(_), AnalysedType::U64(_))
        | (Value::S8(_), AnalysedType::S8(_))
        | (Value::S16(_), AnalysedType::S16(_))
        | (Value::S32(_), AnalysedType::S32(_))
        | (Value::S64(_), AnalysedType::S64(_))
        | (Value::F32(_), AnalysedType::F32(_))
        | (Value::F64(_), AnalysedType::F64(_))
        | (Value::Char(_), AnalysedType::Chr(_))
        | (Value::String(_), AnalysedType::Str(_))
        | (Value::Handle { .. }, AnalysedType::Handle(_)) => Ok(()),

        // List: validate all items against the inner type
        (Value::List(items), AnalysedType::List(list_type)) => {
            for (i, item) in items.iter().enumerate() {
                validate_value_matches_type(item, &list_type.inner).map_err(|e| {
                    format!("list element {i}: {e}")
                })?;
            }
            Ok(())
        }

        // Tuple: arity check + recursive validation
        (Value::Tuple(values), AnalysedType::Tuple(tuple_type)) => {
            if values.len() != tuple_type.items.len() {
                return Err(format!(
                    "tuple has {} elements, expected {}",
                    values.len(),
                    tuple_type.items.len()
                ));
            }
            for (i, (val, typ)) in values.iter().zip(tuple_type.items.iter()).enumerate() {
                validate_value_matches_type(val, typ).map_err(|e| {
                    format!("tuple element {i}: {e}")
                })?;
            }
            Ok(())
        }

        // Record: arity check + recursive validation
        (Value::Record(fields), AnalysedType::Record(record_type)) => {
            if fields.len() != record_type.fields.len() {
                return Err(format!(
                    "record has {} fields, expected {}",
                    fields.len(),
                    record_type.fields.len()
                ));
            }
            for (val, field_def) in fields.iter().zip(record_type.fields.iter()) {
                validate_value_matches_type(val, &field_def.typ).map_err(|e| {
                    format!("record field '{}': {e}", field_def.name)
                })?;
            }
            Ok(())
        }

        // Variant: case index in range + payload validation
        (Value::Variant { case_idx, case_value }, AnalysedType::Variant(variant_type)) => {
            let idx = *case_idx as usize;
            if idx >= variant_type.cases.len() {
                return Err(format!(
                    "variant case index {} out of range (type has {} cases)",
                    case_idx,
                    variant_type.cases.len()
                ));
            }
            let case_def = &variant_type.cases[idx];
            match (&case_def.typ, case_value) {
                (Some(expected_typ), Some(val)) => {
                    validate_value_matches_type(val, expected_typ).map_err(|e| {
                        format!("variant case '{}': {e}", case_def.name)
                    })
                }
                (None, None) => Ok(()),
                (Some(_), None) => Err(format!(
                    "variant case '{}' expects a payload but none was provided",
                    case_def.name
                )),
                (None, Some(_)) => Err(format!(
                    "variant case '{}' expects no payload but one was provided",
                    case_def.name
                )),
            }
        }

        // Enum: case index in range
        (Value::Enum(case_idx), AnalysedType::Enum(enum_type)) => {
            if (*case_idx as usize) >= enum_type.cases.len() {
                Err(format!(
                    "enum case index {} out of range (type has {} cases)",
                    case_idx,
                    enum_type.cases.len()
                ))
            } else {
                Ok(())
            }
        }

        // Flags: length check
        (Value::Flags(flags), AnalysedType::Flags(flags_type)) => {
            if flags.len() != flags_type.names.len() {
                Err(format!(
                    "flags has {} bits, expected {}",
                    flags.len(),
                    flags_type.names.len()
                ))
            } else {
                Ok(())
            }
        }

        // Option: validate inner value if present
        (Value::Option(opt_val), AnalysedType::Option(opt_type)) => {
            if let Some(val) = opt_val {
                validate_value_matches_type(val, &opt_type.inner).map_err(|e| {
                    format!("option value: {e}")
                })
            } else {
                Ok(())
            }
        }

        // Result: validate ok/err payloads
        (Value::Result(result_val), AnalysedType::Result(result_type)) => match result_val {
            Ok(ok_val) => match (&result_type.ok, ok_val) {
                (Some(ok_type), Some(val)) => {
                    validate_value_matches_type(val, ok_type).map_err(|e| {
                        format!("result ok value: {e}")
                    })
                }
                (None, None) => Ok(()),
                (Some(_), None) => Err("result ok expects a value but none was provided".into()),
                (None, Some(_)) => {
                    Err("result ok expects no value but one was provided".into())
                }
            },
            Err(err_val) => match (&result_type.err, err_val) {
                (Some(err_type), Some(val)) => {
                    validate_value_matches_type(val, err_type).map_err(|e| {
                        format!("result err value: {e}")
                    })
                }
                (None, None) => Ok(()),
                (Some(_), None) => Err("result err expects a value but none was provided".into()),
                (None, Some(_)) => {
                    Err("result err expects no value but one was provided".into())
                }
            },
        },

        // Mismatched variants
        _ => Err(format!(
            "expected {}, got {}",
            type_name(expected),
            value_type_name(value)
        )),
    }
}

fn type_name(typ: &AnalysedType) -> &'static str {
    match typ {
        AnalysedType::Bool(_) => "bool",
        AnalysedType::U8(_) => "u8",
        AnalysedType::U16(_) => "u16",
        AnalysedType::U32(_) => "u32",
        AnalysedType::U64(_) => "u64",
        AnalysedType::S8(_) => "s8",
        AnalysedType::S16(_) => "s16",
        AnalysedType::S32(_) => "s32",
        AnalysedType::S64(_) => "s64",
        AnalysedType::F32(_) => "f32",
        AnalysedType::F64(_) => "f64",
        AnalysedType::Chr(_) => "char",
        AnalysedType::Str(_) => "string",
        AnalysedType::List(_) => "list",
        AnalysedType::Tuple(_) => "tuple",
        AnalysedType::Record(_) => "record",
        AnalysedType::Variant(_) => "variant",
        AnalysedType::Enum(_) => "enum",
        AnalysedType::Flags(_) => "flags",
        AnalysedType::Option(_) => "option",
        AnalysedType::Result(_) => "result",
        AnalysedType::Handle(_) => "handle",
    }
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "bool",
        Value::U8(_) => "u8",
        Value::U16(_) => "u16",
        Value::U32(_) => "u32",
        Value::U64(_) => "u64",
        Value::S8(_) => "s8",
        Value::S16(_) => "s16",
        Value::S32(_) => "s32",
        Value::S64(_) => "s64",
        Value::F32(_) => "f32",
        Value::F64(_) => "f64",
        Value::Char(_) => "char",
        Value::String(_) => "string",
        Value::List(_) => "list",
        Value::Tuple(_) => "tuple",
        Value::Record(_) => "record",
        Value::Variant { .. } => "variant",
        Value::Enum(_) => "enum",
        Value::Flags(_) => "flags",
        Value::Option(_) => "option",
        Value::Result(_) => "result",
        Value::Handle { .. } => "handle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analysed_type::*;
    use test_r::test;

    #[test]
    fn primitives_match() {
        assert!(validate_value_matches_type(&Value::Bool(true), &bool()).is_ok());
        assert!(validate_value_matches_type(&Value::U8(1), &u8()).is_ok());
        assert!(validate_value_matches_type(&Value::U16(1), &u16()).is_ok());
        assert!(validate_value_matches_type(&Value::U32(1), &u32()).is_ok());
        assert!(validate_value_matches_type(&Value::U64(1), &u64()).is_ok());
        assert!(validate_value_matches_type(&Value::S8(1), &s8()).is_ok());
        assert!(validate_value_matches_type(&Value::S16(1), &s16()).is_ok());
        assert!(validate_value_matches_type(&Value::S32(1), &s32()).is_ok());
        assert!(validate_value_matches_type(&Value::S64(1), &s64()).is_ok());
        assert!(validate_value_matches_type(&Value::F32(1.0), &f32()).is_ok());
        assert!(validate_value_matches_type(&Value::F64(1.0), &f64()).is_ok());
        assert!(validate_value_matches_type(&Value::Char('a'), &chr()).is_ok());
        assert!(validate_value_matches_type(&Value::String("hi".into()), &str()).is_ok());
    }

    #[test]
    fn primitive_mismatch() {
        let err = validate_value_matches_type(&Value::String("hi".into()), &u64()).unwrap_err();
        assert_eq!(err, "expected u64, got string");
    }

    #[test]
    fn tuple_valid() {
        let val = Value::Tuple(vec![Value::U32(1), Value::String("x".into())]);
        let typ = tuple(vec![u32(), str()]);
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn tuple_wrong_arity() {
        let val = Value::Tuple(vec![Value::U32(1)]);
        let typ = tuple(vec![u32(), str()]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("tuple has 1 elements, expected 2"));
    }

    #[test]
    fn tuple_wrong_element_type() {
        let val = Value::Tuple(vec![Value::String("x".into()), Value::String("y".into())]);
        let typ = tuple(vec![u32(), str()]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("tuple element 0"));
        assert!(err.contains("expected u32, got string"));
    }

    #[test]
    fn record_valid() {
        let val = Value::Record(vec![Value::U64(42), Value::String("name".into())]);
        let typ = record(vec![field("id", u64()), field("name", str())]);
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn record_wrong_field_type() {
        let val = Value::Record(vec![Value::String("not-a-number".into())]);
        let typ = record(vec![field("id", u64())]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("record field 'id'"));
    }

    #[test]
    fn list_valid() {
        let val = Value::List(vec![Value::U32(1), Value::U32(2)]);
        let typ = list(u32());
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn list_wrong_element() {
        let val = Value::List(vec![Value::U32(1), Value::String("x".into())]);
        let typ = list(u32());
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("list element 1"));
    }

    #[test]
    fn option_none_valid() {
        let val = Value::Option(None);
        let typ = option(u32());
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn option_some_valid() {
        let val = Value::Option(Some(Box::new(Value::U32(1))));
        let typ = option(u32());
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn option_some_wrong_type() {
        let val = Value::Option(Some(Box::new(Value::String("x".into()))));
        let typ = option(u32());
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("option value"));
    }

    #[test]
    fn result_ok_valid() {
        let val = Value::Result(Ok(Some(Box::new(Value::U32(1)))));
        let typ = result(u32(), str());
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn result_err_valid() {
        let val = Value::Result(Err(Some(Box::new(Value::String("err".into())))));
        let typ = result(u32(), str());
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn result_ok_wrong_type() {
        let val = Value::Result(Ok(Some(Box::new(Value::String("x".into())))));
        let typ = result(u32(), str());
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("result ok value"));
    }

    #[test]
    fn enum_valid() {
        let val = Value::Enum(0);
        let typ = r#enum(&["a", "b"]);
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn enum_out_of_range() {
        let val = Value::Enum(5);
        let typ = r#enum(&["a", "b"]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("out of range"));
    }

    #[test]
    fn flags_valid() {
        let val = Value::Flags(vec![true, false, true]);
        let typ = flags(&["a", "b", "c"]);
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn flags_wrong_length() {
        let val = Value::Flags(vec![true]);
        let typ = flags(&["a", "b", "c"]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("flags has 1 bits, expected 3"));
    }

    #[test]
    fn variant_valid_with_payload() {
        let val = Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::U32(1))),
        };
        let typ = variant(vec![case("some", u32()), unit_case("none")]);
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn variant_valid_unit() {
        let val = Value::Variant {
            case_idx: 1,
            case_value: None,
        };
        let typ = variant(vec![case("some", u32()), unit_case("none")]);
        assert!(validate_value_matches_type(&val, &typ).is_ok());
    }

    #[test]
    fn variant_out_of_range() {
        let val = Value::Variant {
            case_idx: 5,
            case_value: None,
        };
        let typ = variant(vec![unit_case("a")]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("out of range"));
    }

    #[test]
    fn variant_unexpected_payload() {
        let val = Value::Variant {
            case_idx: 1,
            case_value: Some(Box::new(Value::U32(1))),
        };
        let typ = variant(vec![case("some", u32()), unit_case("none")]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("expects no payload"));
    }

    #[test]
    fn nested_deep_mismatch() {
        let val = Value::Record(vec![
            Value::String("name".into()),
            Value::List(vec![
                Value::Tuple(vec![Value::U32(1), Value::U32(2)]),
                Value::Tuple(vec![Value::U32(3), Value::String("wrong".into())]),
            ]),
        ]);
        let inner_tuple = tuple(vec![u32(), u32()]);
        let typ = record(vec![field("name", str()), field("items", list(inner_tuple))]);
        let err = validate_value_matches_type(&val, &typ).unwrap_err();
        assert!(err.contains("record field 'items'"));
        assert!(err.contains("list element 1"));
        assert!(err.contains("tuple element 1"));
        assert!(err.contains("expected u32, got string"));
    }
}
