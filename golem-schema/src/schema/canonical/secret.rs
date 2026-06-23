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

//! Canonical encoding for [`SecretValuePayload`] (opaque reference only).
//!
//! - Text form: `secret:<opaque-ref>`. The `secret:` prefix is required on
//!   both encode and decode. An empty `secret_ref` is rejected on both
//!   sides — we never emit a form we would not decode back.
//! - JSON form: `{ "secret_ref": "..." }`.

use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::SecretValuePayload;
use serde_json::{Map, Value};

const TEXT_PREFIX: &str = "secret:";

pub fn to_text(payload: &SecretValuePayload) -> Result<String, ParseError> {
    if payload.secret_ref.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(format!("{TEXT_PREFIX}{}", payload.secret_ref))
}

pub fn from_text(s: &str) -> Result<SecretValuePayload, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let rest = s
        .strip_prefix(TEXT_PREFIX)
        .ok_or_else(|| ParseError::BadFormat("missing 'secret:' prefix".into()))?;
    if rest.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(SecretValuePayload {
        secret_ref: rest.to_string(),
    })
}

pub fn to_json(payload: &SecretValuePayload) -> Result<Value, ParseError> {
    if payload.secret_ref.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut obj = Map::new();
    obj.insert(
        "secret_ref".to_string(),
        Value::String(payload.secret_ref.clone()),
    );
    Ok(Value::Object(obj))
}

pub fn from_json(value: &Value) -> Result<SecretValuePayload, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    let secret_ref = obj
        .get("secret_ref")
        .ok_or(ParseError::MissingField("secret_ref"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("secret_ref"),
        })?
        .to_string();
    if secret_ref.is_empty() {
        return Err(ParseError::Empty);
    }
    for key in obj.keys() {
        if key != "secret_ref" {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    Ok(SecretValuePayload { secret_ref })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    fn payload_strategy() -> impl Strategy<Value = SecretValuePayload> {
        "[A-Za-z0-9_./-]{1,32}".prop_map(|secret_ref| SecretValuePayload { secret_ref })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn text_round_trip(p in payload_strategy()) {
            let s = to_text(&p).expect("to_text");
            let back = from_text(&s).expect("from_text");
            prop_assert_eq!(back, p);
        }

        #[test]
        fn json_round_trip(p in payload_strategy()) {
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
    fn missing_prefix_rejected() {
        assert!(matches!(from_text("plain"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn prefix_only_rejected() {
        assert_eq!(from_text("secret:"), Err(ParseError::Empty));
    }

    #[test]
    fn json_wrong_type() {
        assert_eq!(
            from_json(&Value::String("x".into())),
            Err(ParseError::TypeField {
                expected: "object",
                field: None,
            })
        );
    }

    #[test]
    fn json_missing_field() {
        assert_eq!(
            from_json(&serde_json::json!({})),
            Err(ParseError::MissingField("secret_ref"))
        );
    }

    #[test]
    fn empty_secret_ref_text_encode_rejected() {
        let p = SecretValuePayload {
            secret_ref: String::new(),
        };
        assert_eq!(to_text(&p), Err(ParseError::Empty));
    }

    #[test]
    fn empty_secret_ref_json_encode_rejected() {
        let p = SecretValuePayload {
            secret_ref: String::new(),
        };
        assert_eq!(to_json(&p), Err(ParseError::Empty));
    }

    #[test]
    fn json_extra_field_rejected() {
        let v = serde_json::json!({ "secret_ref": "x", "extra": true });
        assert_eq!(from_json(&v), Err(ParseError::ExtraField("extra".into())));
    }

    #[test]
    fn json_wrong_secret_ref_type_rejected() {
        let v = serde_json::json!({ "secret_ref": 42 });
        assert_eq!(
            from_json(&v),
            Err(ParseError::TypeField {
                expected: "string",
                field: Some("secret_ref"),
            })
        );
    }
}
