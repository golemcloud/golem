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

mod r#impl;

use crate::ValueAndType;
use golem_wasm_ast::analysis::AnalysedType;
use serde::ser::Error;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value as JsonValue;

pub trait ValueAndTypeJsonExtensions: Sized {
    /// Parses a JSON value representation (with no type information) into a typed value based
    /// on the given type information.
    fn parse_with_type(json_val: &JsonValue, typ: &AnalysedType) -> Result<Self, Vec<String>>;

    /// Converts a type annotated value to a JSON value representation with no type information.
    ///
    /// Use `ValueAndType`'s `Serialize` instance with `serde_json` to get a self-describing
    /// representation that contains both the type information and the value.
    fn to_json_value(&self) -> Result<JsonValue, String>;
}

/// An internal representation of a ValueAndType that can be serialized to JSON.
#[derive(Serialize, Deserialize)]
struct ValueAndTypeJson {
    typ: AnalysedType,
    value: serde_json::Value,
}

/// A representation that optionally pairs type definition with a JSON represented value.
///
/// It can only be converted to any of the typed value representations if the type information
/// is present (or provided externally).
///
/// The JSON format is backward compatible with `ValueAndTypeJson`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionallyValueAndTypeJson {
    pub typ: Option<AnalysedType>,
    pub value: serde_json::Value,
}

impl OptionallyValueAndTypeJson {
    pub fn has_type(&self) -> bool {
        self.typ.is_some()
    }

    pub fn into_json_value(self) -> serde_json::Value {
        self.value
    }

    pub fn into_value_and_type(self, typ: AnalysedType) -> Result<ValueAndType, Vec<String>> {
        ValueAndType::parse_with_type(&self.value, &typ)
    }

    pub fn try_into_value_and_type(self) -> Result<Option<ValueAndType>, Vec<String>> {
        match self.typ {
            Some(typ) => ValueAndType::parse_with_type(&self.value, &typ).map(Some),
            None => Ok(None),
        }
    }
}

impl TryFrom<ValueAndType> for OptionallyValueAndTypeJson {
    type Error = String;

    fn try_from(vnt: ValueAndType) -> Result<Self, Self::Error> {
        let value = vnt.to_json_value()?;
        Ok(OptionallyValueAndTypeJson {
            typ: Some(vnt.typ),
            value,
        })
    }
}

impl Serialize for ValueAndType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let typ = self.typ.clone();
        let value = self.to_json_value().map_err(S::Error::custom)?;
        let json = ValueAndTypeJson { typ, value };
        json.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ValueAndType {
    fn deserialize<D>(deserializer: D) -> Result<ValueAndType, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let json = ValueAndTypeJson::deserialize(deserializer)?;
        let value = ValueAndType::parse_with_type(&json.value, &json.typ).map_err(|err| {
            serde::de::Error::custom(format!(
                "Invalid type-annotated JSON value: {}",
                err.join(", ")
            ))
        })?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::{IntoValueAndType, Value, ValueAndType};
    use golem_wasm_ast::analysis::analysed_type::{result_err, result_ok, str, tuple};

    use serde_json::json;

    #[test]
    fn example1() {
        let vnt = (10u32, "hello".to_string()).into_value_and_type();
        let json = serde_json::to_value(&vnt).unwrap();
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

        let tav2: ValueAndType = serde_json::from_value(json).unwrap();
        assert_eq!(vnt, tav2);
    }

    #[test]
    fn example2() {
        let vnt = ValueAndType {
            typ: tuple(vec![result_err(str())]),
            value: Value::Tuple(vec![Value::Result(Ok(None))]),
        };
        let json = serde_json::to_value(&vnt).unwrap();
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

        let tav2: ValueAndType = serde_json::from_value(json).unwrap();
        assert_eq!(vnt, tav2);
    }

    #[test]
    fn example3() {
        let vnt = ValueAndType {
            typ: tuple(vec![result_ok(str())]),
            value: Value::Tuple(vec![Value::Result(Err(None))]),
        };
        let json = serde_json::to_value(&vnt).unwrap();
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

        let tav2: ValueAndType = serde_json::from_value(json).unwrap();
        assert_eq!(vnt, tav2);
    }
}
