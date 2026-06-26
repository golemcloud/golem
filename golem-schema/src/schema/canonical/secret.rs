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

//! Canonical encoding for [`SecretValuePayload`] (opaque snapshot only).
//!
//! - Text form: `secret:<json-snapshot>`. The `secret:` prefix is required on
//!   both encode and decode.
//! - JSON form: `{ "secretId": "...", "version": n, "resolvedAt": "..." }`
//!   with optional `configKey` and `category`.

use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::SecretValuePayload;
use serde_json::{Map, Value};

const TEXT_PREFIX: &str = "secret:";

pub fn to_text(payload: &SecretValuePayload) -> Result<String, ParseError> {
    let json = to_json(payload)?;
    Ok(format!("{TEXT_PREFIX}{json}"))
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
    let value = serde_json::from_str(rest).map_err(|e| ParseError::BadFormat(e.to_string()))?;
    from_json(&value)
}

pub fn to_json(payload: &SecretValuePayload) -> Result<Value, ParseError> {
    let mut obj = Map::new();
    obj.insert(
        "secretId".to_string(),
        Value::String(payload.secret_id.to_string()),
    );
    if let Some(config_key) = &payload.config_key {
        obj.insert(
            "configKey".to_string(),
            Value::Array(config_key.iter().cloned().map(Value::String).collect()),
        );
    }
    obj.insert("version".to_string(), Value::Number(payload.version.into()));
    obj.insert(
        "resolvedAt".to_string(),
        Value::String(payload.resolved_at.to_rfc3339()),
    );
    if let Some(category) = &payload.category {
        obj.insert("category".to_string(), Value::String(category.clone()));
    }
    Ok(Value::Object(obj))
}

pub fn from_json(value: &Value) -> Result<SecretValuePayload, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    let secret_id = obj
        .get("secretId")
        .ok_or(ParseError::MissingField("secretId"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("secretId"),
        })?
        .parse()
        .map_err(|e: uuid::Error| ParseError::BadFormat(e.to_string()))?;
    let config_key = match obj.get("configKey") {
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or(ParseError::TypeField {
                        expected: "string",
                        field: Some("configKey"),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => {
            return Err(ParseError::TypeField {
                expected: "array",
                field: Some("configKey"),
            });
        }
        None => None,
    };
    let version = obj
        .get("version")
        .ok_or(ParseError::MissingField("version"))?
        .as_u64()
        .ok_or(ParseError::TypeField {
            expected: "u64",
            field: Some("version"),
        })?;
    let resolved_at = obj
        .get("resolvedAt")
        .ok_or(ParseError::MissingField("resolvedAt"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("resolvedAt"),
        })?
        .parse::<chrono::DateTime<chrono::Utc>>()
        .map_err(|e| ParseError::BadFormat(e.to_string()))?;
    let category = match obj.get("category") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(ParseError::TypeField {
                expected: "string",
                field: Some("category"),
            });
        }
        None => None,
    };
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "secretId" | "configKey" | "version" | "resolvedAt" | "category"
        ) {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    Ok(SecretValuePayload {
        secret_id,
        config_key,
        version,
        resolved_at,
        category,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    fn payload_strategy() -> impl Strategy<Value = SecretValuePayload> {
        (
            any::<u128>(),
            prop::option::of(prop::collection::vec("[A-Za-z0-9_-]{1,12}", 0..3)),
            any::<u64>(),
            prop::option::of("[A-Za-z0-9_-]{1,12}"),
        )
            .prop_map(|(id, config_key, version, category)| SecretValuePayload {
                secret_id: uuid::Uuid::from_u128(id),
                config_key,
                version,
                resolved_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                category,
            })
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
            Err(ParseError::MissingField("secretId"))
        );
    }

    #[test]
    fn json_extra_field_rejected() {
        let v = serde_json::json!({
            "secretId": uuid::Uuid::nil().to_string(),
            "version": 0,
            "resolvedAt": "1970-01-01T00:00:00Z",
            "extra": true
        });
        assert_eq!(from_json(&v), Err(ParseError::ExtraField("extra".into())));
    }

    #[test]
    fn json_wrong_secret_id_type_rejected() {
        let v = serde_json::json!({ "secretId": 42 });
        assert_eq!(
            from_json(&v),
            Err(ParseError::TypeField {
                expected: "string",
                field: Some("secretId"),
            })
        );
    }
}
