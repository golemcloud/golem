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

//! Canonical encoding for [`QuotaTokenValuePayload`].
//!
//! - Text form: `quota-token:<base64-of-json>`, where `<base64-of-json>` is
//!   the URL-safe-no-pad base64 encoding of the canonical JSON form's UTF-8
//!   bytes. (URL-safe-no-pad is a Golem-specific choice; it differs from
//!   the standard base64 alphabet.)
//! - JSON form:
//!   ```json
//!   {
//!     "environment_id": "<uuid>",
//!     "resource_name": "...",
//!     "expected_use": "<u64-as-string>",
//!     "last_credit": "<i64-as-string>",
//!     "last_credit_at": "<rfc3339>"
//!   }
//!   ```
//!   `environment_id` is the canonical hyphenated UUID string.
//!   `expected_use` and `last_credit` are written as JSON **strings** on
//!   output (so JavaScript-side parsers can preserve the full u64 / i64
//!   range without precision loss). On input both a JSON number and a JSON
//!   string are accepted for those two fields.

use crate::schema::canonical::datetime;
use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::QuotaTokenValuePayload;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value};
use uuid::Uuid;

const TEXT_PREFIX: &str = "quota-token:";

const FIELD_ENV_ID: &str = "environment_id";
const FIELD_RESOURCE: &str = "resource_name";
const FIELD_EXPECTED_USE: &str = "expected_use";
const FIELD_LAST_CREDIT: &str = "last_credit";
const FIELD_LAST_CREDIT_AT: &str = "last_credit_at";

pub fn to_text(payload: &QuotaTokenValuePayload) -> Result<String, ParseError> {
    let json = to_json(payload)?;
    let body = serde_json::to_vec(&json).expect("serialise QuotaToken JSON");
    Ok(format!("{TEXT_PREFIX}{}", URL_SAFE_NO_PAD.encode(body)))
}

pub fn from_text(s: &str) -> Result<QuotaTokenValuePayload, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let rest = s
        .strip_prefix(TEXT_PREFIX)
        .ok_or_else(|| ParseError::BadFormat("missing 'quota-token:' prefix".into()))?;
    if rest.is_empty() {
        return Err(ParseError::Empty);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(rest.as_bytes())
        .map_err(|e| ParseError::InvalidBase64(e.to_string()))?;
    let json: Value = serde_json::from_slice(&bytes)
        .map_err(|e| ParseError::BadFormat(format!("invalid JSON payload: {e}")))?;
    from_json(&json)
}

pub fn to_json(payload: &QuotaTokenValuePayload) -> Result<Value, ParseError> {
    let mut obj = Map::new();
    obj.insert(
        FIELD_ENV_ID.to_string(),
        Value::String(payload.environment_id.hyphenated().to_string()),
    );
    obj.insert(
        FIELD_RESOURCE.to_string(),
        Value::String(payload.resource_name.clone()),
    );
    obj.insert(
        FIELD_EXPECTED_USE.to_string(),
        Value::String(payload.expected_use.to_string()),
    );
    obj.insert(
        FIELD_LAST_CREDIT.to_string(),
        Value::String(payload.last_credit.to_string()),
    );
    obj.insert(
        FIELD_LAST_CREDIT_AT.to_string(),
        datetime::to_json(&payload.last_credit_at).map_err(|e| ParseError::Nested(Box::new(e)))?,
    );
    Ok(Value::Object(obj))
}

fn parse_u64_field(value: &Value, field: &'static str) -> Result<u64, ParseError> {
    match value {
        Value::Number(n) => n.as_u64().ok_or(ParseError::TypeField {
            expected: "non-negative integer",
            field: Some(field),
        }),
        Value::String(s) => s.parse::<u64>().map_err(|_| ParseError::TypeField {
            expected: "non-negative integer string",
            field: Some(field),
        }),
        _ => Err(ParseError::TypeField {
            expected: "number or string",
            field: Some(field),
        }),
    }
}

fn parse_i64_field(value: &Value, field: &'static str) -> Result<i64, ParseError> {
    match value {
        Value::Number(n) => n.as_i64().ok_or(ParseError::TypeField {
            expected: "integer",
            field: Some(field),
        }),
        Value::String(s) => s.parse::<i64>().map_err(|_| ParseError::TypeField {
            expected: "integer string",
            field: Some(field),
        }),
        _ => Err(ParseError::TypeField {
            expected: "number or string",
            field: Some(field),
        }),
    }
}

pub fn from_json(value: &Value) -> Result<QuotaTokenValuePayload, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    for key in obj.keys() {
        if ![
            FIELD_ENV_ID,
            FIELD_RESOURCE,
            FIELD_EXPECTED_USE,
            FIELD_LAST_CREDIT,
            FIELD_LAST_CREDIT_AT,
        ]
        .contains(&key.as_str())
        {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    let env_id_str = obj
        .get(FIELD_ENV_ID)
        .ok_or(ParseError::MissingField("environment_id"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("environment_id"),
        })?;
    let environment_id = Uuid::parse_str(env_id_str)
        .map_err(|e| ParseError::BadFormat(format!("invalid UUID: {e}")))?;
    let resource_name = obj
        .get(FIELD_RESOURCE)
        .ok_or(ParseError::MissingField("resource_name"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("resource_name"),
        })?
        .to_string();
    let expected_use = parse_u64_field(
        obj.get(FIELD_EXPECTED_USE)
            .ok_or(ParseError::MissingField("expected_use"))?,
        "expected_use",
    )?;
    let last_credit = parse_i64_field(
        obj.get(FIELD_LAST_CREDIT)
            .ok_or(ParseError::MissingField("last_credit"))?,
        "last_credit",
    )?;
    let last_credit_at = datetime::from_json(
        obj.get(FIELD_LAST_CREDIT_AT)
            .ok_or(ParseError::MissingField("last_credit_at"))?,
    )
    .map_err(|e| ParseError::Nested(Box::new(e)))?;
    Ok(QuotaTokenValuePayload {
        environment_id,
        resource_name,
        expected_use,
        last_credit,
        last_credit_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use proptest::prelude::*;
    use test_r::test;

    fn datetime_strategy() -> impl Strategy<Value = DateTime<Utc>> {
        // RFC 3339 only encodes years 0000-9999.
        (0i64..253_402_214_400i64, 0u32..1_000_000_000).prop_map(|(s, n)| {
            Utc.timestamp_opt(s, n)
                .single()
                .expect("strategy bounds keep timestamps valid")
        })
    }

    fn payload_strategy() -> impl Strategy<Value = QuotaTokenValuePayload> {
        (
            any::<u64>(),
            any::<u64>(),
            "[a-z][a-z0-9_-]{0,15}",
            any::<u64>(),
            any::<i64>(),
            datetime_strategy(),
        )
            .prop_map(
                |(hi, lo, resource_name, expected_use, last_credit, last_credit_at)| {
                    QuotaTokenValuePayload {
                        environment_id: Uuid::from_u64_pair(hi, lo),
                        resource_name,
                        expected_use,
                        last_credit,
                        last_credit_at,
                    }
                },
            )
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
        assert!(matches!(
            from_text("garbage"),
            Err(ParseError::BadFormat(_))
        ));
    }

    #[test]
    fn prefix_only_rejected() {
        assert_eq!(from_text("quota-token:"), Err(ParseError::Empty));
    }

    #[test]
    fn bad_base64_rejected() {
        assert!(matches!(
            from_text("quota-token:!!!"),
            Err(ParseError::InvalidBase64(_))
        ));
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
        let v = serde_json::json!({
            "environment_id": "00000000-0000-0000-0000-000000000000",
            "resource_name": "r",
            "expected_use": "1",
            "last_credit": "0"
        });
        assert_eq!(
            from_json(&v),
            Err(ParseError::MissingField("last_credit_at"))
        );
    }

    #[test]
    fn json_extra_field_rejected() {
        let v = serde_json::json!({
            "environment_id": "00000000-0000-0000-0000-000000000000",
            "resource_name": "r",
            "expected_use": "1",
            "last_credit": "0",
            "last_credit_at": "2025-01-01T00:00:00Z",
            "stowaway": true
        });
        assert_eq!(
            from_json(&v),
            Err(ParseError::ExtraField("stowaway".into()))
        );
    }

    #[test]
    fn expected_use_accepts_number_or_string() {
        let base = serde_json::json!({
            "environment_id": "00000000-0000-0000-0000-000000000000",
            "resource_name": "r",
            "expected_use": 42,
            "last_credit": -1,
            "last_credit_at": "2025-01-01T00:00:00Z"
        });
        let v_num = from_json(&base).expect("number form");
        assert_eq!(v_num.expected_use, 42);
        assert_eq!(v_num.last_credit, -1);

        let mut s = base.as_object().expect("base obj").clone();
        s.insert(
            FIELD_EXPECTED_USE.into(),
            Value::String("18446744073709551615".into()), // u64::MAX
        );
        s.insert(
            FIELD_LAST_CREDIT.into(),
            Value::String("-9223372036854775808".into()), // i64::MIN
        );
        let v_str = from_json(&Value::Object(s)).expect("string form");
        assert_eq!(v_str.expected_use, u64::MAX);
        assert_eq!(v_str.last_credit, i64::MIN);
    }

    #[test]
    fn output_uses_string_for_numeric_fields() {
        let p = QuotaTokenValuePayload {
            environment_id: Uuid::nil(),
            resource_name: "r".into(),
            expected_use: u64::MAX,
            last_credit: i64::MIN,
            last_credit_at: chrono::Utc.timestamp_opt(0, 0).single().unwrap(),
        };
        let v = to_json(&p).expect("to_json");
        let obj = v.as_object().expect("object");
        assert_eq!(
            obj.get(FIELD_EXPECTED_USE),
            Some(&Value::String(u64::MAX.to_string()))
        );
        assert_eq!(
            obj.get(FIELD_LAST_CREDIT),
            Some(&Value::String(i64::MIN.to_string()))
        );
    }

    #[test]
    fn invalid_uuid_rejected() {
        let v = serde_json::json!({
            "environment_id": "not a uuid",
            "resource_name": "r",
            "expected_use": "1",
            "last_credit": "0",
            "last_credit_at": "2025-01-01T00:00:00Z"
        });
        assert!(matches!(from_json(&v), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn wrong_expected_use_type_rejected() {
        let v = serde_json::json!({
            "environment_id": "00000000-0000-0000-0000-000000000000",
            "resource_name": "r",
            "expected_use": true,
            "last_credit": "0",
            "last_credit_at": "2025-01-01T00:00:00Z"
        });
        assert_eq!(
            from_json(&v),
            Err(ParseError::TypeField {
                expected: "number or string",
                field: Some("expected_use"),
            })
        );
    }

    #[test]
    fn wrong_last_credit_type_rejected() {
        let v = serde_json::json!({
            "environment_id": "00000000-0000-0000-0000-000000000000",
            "resource_name": "r",
            "expected_use": "0",
            "last_credit": true,
            "last_credit_at": "2025-01-01T00:00:00Z"
        });
        assert_eq!(
            from_json(&v),
            Err(ParseError::TypeField {
                expected: "number or string",
                field: Some("last_credit"),
            })
        );
    }

    #[test]
    fn invalid_last_credit_at_rejected() {
        let v = serde_json::json!({
            "environment_id": "00000000-0000-0000-0000-000000000000",
            "resource_name": "r",
            "expected_use": "0",
            "last_credit": "0",
            "last_credit_at": "not a date"
        });
        assert!(matches!(from_json(&v), Err(ParseError::Nested(_))));
    }
}
