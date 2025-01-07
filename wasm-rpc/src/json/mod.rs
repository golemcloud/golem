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

mod r#impl;

use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::ValueAndType;
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value as JsonValue;

pub trait TypeAnnotatedValueJsonExtensions: Sized {
    /// Parses a JSON value representation (with no type information) into a typed value based
    /// on the given type information.
    fn parse_with_type(json_val: &JsonValue, typ: &AnalysedType) -> Result<Self, Vec<String>>;

    /// Converts a `TypeAnnotatedValue` to a JSON value representation with no type information.
    ///
    /// Use `TypeAnnotatedValue`'s `Serialize` instance with `serde_json` to get a self-describing
    /// representation that contains both the type information and the value.
    fn to_json_value(&self) -> JsonValue;
}

/// An internal representation of a TypeAnnotatedValue that can be serialized to JSON.
#[derive(Serialize, Deserialize)]
struct TypeAnnotatedValueJson {
    typ: AnalysedType,
    value: serde_json::Value,
}

impl Serialize for TypeAnnotatedValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let typ: AnalysedType = self.try_into().map_err(serde::ser::Error::custom)?;
        let value = self.to_json_value();
        let json = TypeAnnotatedValueJson { typ, value };
        json.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TypeAnnotatedValue {
    fn deserialize<D>(deserializer: D) -> Result<TypeAnnotatedValue, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let json = TypeAnnotatedValueJson::deserialize(deserializer)?;
        let value = TypeAnnotatedValue::parse_with_type(&json.value, &json.typ).map_err(|err| {
            serde::de::Error::custom(format!(
                "Invalid type-annotated JSON value: {}",
                err.join(", ")
            ))
        })?;
        Ok(value)
    }
}

impl Serialize for ValueAndType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let tav: TypeAnnotatedValue = self.try_into().map_err(|err: Vec<String>| {
            serde::ser::Error::custom(format!(
                "Invalid type-annotated JSON value: {}",
                err.join(", ")
            ))
        })?;
        tav.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ValueAndType {
    fn deserialize<D>(deserializer: D) -> Result<ValueAndType, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let tav = TypeAnnotatedValue::deserialize(deserializer)?;
        tav.try_into().map_err(|err| {
            serde::de::Error::custom(format!("Invalid type-annotated JSON value: {err}",))
        })
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
    use crate::{TypeAnnotatedValueConstructors, Value};
    use golem_wasm_ast::analysis::analysed_type::{result_err, result_ok, str, tuple, u32};

    use serde_json::json;

    #[test]
    fn example1() {
        let tav = TypeAnnotatedValue::create(
            &Value::Tuple(vec![Value::U32(10), Value::String("hello".to_string())]),
            &tuple(vec![u32(), str()]),
        )
        .unwrap();
        let json = serde_json::to_value(&tav).unwrap();
        assert_eq!(
            json,
            json!({
                "typ": {
                    "type": "Tuple",
                    "items": [
                            { "type": "U32" },
                            { "type": "Str" }
                    ]
                },
                "value": [10, "hello"]
            })
        );

        let tav2: TypeAnnotatedValue = serde_json::from_value(json).unwrap();
        assert_eq!(tav, tav2);
    }

    #[test]
    fn example2() {
        let tav = TypeAnnotatedValue::create(
            &Value::Tuple(vec![Value::Result(Ok(None))]),
            &tuple(vec![result_err(str())]),
        )
        .unwrap();
        let json = serde_json::to_value(&tav).unwrap();
        assert_eq!(
            json,
            json!({
                "typ": {
                    "type": "Tuple",
                    "items": [
                        {
                            "type": "Result",
                            "err": {
                                "type": "Str"
                            },
                            "ok": null
                        },
                    ]
                },
                "value": [{ "ok": null }]
            })
        );

        let tav2: TypeAnnotatedValue = serde_json::from_value(json).unwrap();
        assert_eq!(tav, tav2);
    }

    #[test]
    fn example3() {
        let tav = TypeAnnotatedValue::create(
            &Value::Tuple(vec![Value::Result(Err(None))]),
            &tuple(vec![result_ok(str())]),
        )
        .unwrap();
        let json = serde_json::to_value(&tav).unwrap();
        assert_eq!(
            json,
            json!({
                "typ": {
                    "type": "Tuple",
                    "items": [
                        {
                            "type": "Result",
                            "ok": {
                                "type": "Str"
                            },
                            "err": null
                        },
                    ]
                },
                "value": [{ "err": null }]
            })
        );

        let tav2: TypeAnnotatedValue = serde_json::from_value(json).unwrap();
        assert_eq!(tav, tav2);
    }
}
