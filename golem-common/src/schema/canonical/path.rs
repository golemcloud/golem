// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Canonical encoding for [`super::super::schema_value::SchemaValue::Path`]
//! payloads.
//!
//! The payload is the path string itself.
//!
//! - Text form: the path as-is (UTF-8), with no scheme prefix.
//! - JSON form: a JSON string with the same body.
//! - Empty paths are rejected on both encode and decode — we never emit a
//!   form we would not decode back.

use crate::schema::canonical::error::ParseError;
use serde_json::Value;

pub fn to_text(payload: &str) -> Result<String, ParseError> {
    if payload.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(payload.to_string())
}

pub fn from_text(s: &str) -> Result<String, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(s.to_string())
}

pub fn to_json(payload: &str) -> Result<Value, ParseError> {
    if payload.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(Value::String(payload.to_string()))
}

pub fn from_json(value: &Value) -> Result<String, ParseError> {
    match value {
        Value::String(s) if s.is_empty() => Err(ParseError::Empty),
        Value::String(s) => Ok(s.clone()),
        _ => Err(ParseError::TypeField {
            expected: "string",
            field: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn text_round_trip(p in "[^\x00]{1,32}") {
            let s = to_text(&p).expect("to_text");
            let back = from_text(&s).expect("from_text");
            prop_assert_eq!(back, p);
        }

        #[test]
        fn json_round_trip(p in "[^\x00]{1,32}") {
            let j = to_json(&p).expect("to_json");
            let back = from_json(&j).expect("from_json");
            prop_assert_eq!(back, p);
        }
    }

    #[test]
    fn empty_text_rejected() {
        assert_eq!(from_text(""), Err(ParseError::Empty));
    }

    #[test]
    fn empty_text_encode_rejected() {
        assert_eq!(to_text(""), Err(ParseError::Empty));
    }

    #[test]
    fn empty_json_encode_rejected() {
        assert_eq!(to_json(""), Err(ParseError::Empty));
    }

    #[test]
    fn json_wrong_type() {
        assert_eq!(
            from_json(&Value::Null),
            Err(ParseError::TypeField {
                expected: "string",
                field: None,
            })
        );
        assert_eq!(
            from_json(&serde_json::json!({})),
            Err(ParseError::TypeField {
                expected: "string",
                field: None,
            })
        );
    }

    #[test]
    fn json_empty_string_rejected() {
        assert_eq!(
            from_json(&Value::String(String::new())),
            Err(ParseError::Empty)
        );
    }
}
