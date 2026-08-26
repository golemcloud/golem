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

//! Canonical encoding for [`PermissionCardValuePayload`].
//!
//! - Text form: `permission-card:<base64-of-json>`, where `<base64-of-json>`
//!   is the URL-safe-no-pad base64 encoding of the canonical JSON form's UTF-8
//!   bytes.
//! - JSON form:
//!   ```json
//!   {
//!     "cardId": "<uuid>",
//!     "parentIds": ["<uuid>", ...],
//!     "expiresAt": "<rfc3339>",
//!     "polymorphic": true
//!   }
//!   ```
//!   `cardId` is the canonical hyphenated UUID string and is the only
//!   authoritative identity field. `parentIds` defaults to an empty array when
//!   absent. `expiresAt` is optional (absent means no expiry). `polymorphic`
//!   defaults to `false` when absent.

use crate::schema::canonical::datetime;
use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::PermissionCardValuePayload;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value};

const TEXT_PREFIX: &str = "permission-card:";

const FIELD_CARD_ID: &str = "cardId";
const FIELD_PARENT_IDS: &str = "parentIds";
const FIELD_EXPIRES_AT: &str = "expiresAt";
const FIELD_POLYMORPHIC: &str = "polymorphic";

pub fn to_text(payload: &PermissionCardValuePayload) -> Result<String, ParseError> {
    let json = to_json(payload)?;
    let body = serde_json::to_vec(&json).expect("serialise PermissionCard JSON");
    Ok(format!("{TEXT_PREFIX}{}", URL_SAFE_NO_PAD.encode(body)))
}

pub fn from_text(s: &str) -> Result<PermissionCardValuePayload, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let rest = s
        .strip_prefix(TEXT_PREFIX)
        .ok_or_else(|| ParseError::BadFormat("missing 'permission-card:' prefix".into()))?;
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

pub fn to_json(payload: &PermissionCardValuePayload) -> Result<Value, ParseError> {
    let mut obj = Map::new();
    obj.insert(
        FIELD_CARD_ID.to_string(),
        Value::String(payload.card_id.hyphenated().to_string()),
    );
    obj.insert(
        FIELD_PARENT_IDS.to_string(),
        Value::Array(
            payload
                .parent_ids
                .iter()
                .map(|id| Value::String(id.hyphenated().to_string()))
                .collect(),
        ),
    );
    if let Some(expires_at) = &payload.expires_at {
        obj.insert(
            FIELD_EXPIRES_AT.to_string(),
            datetime::to_json(expires_at).map_err(|e| ParseError::Nested(Box::new(e)))?,
        );
    }
    obj.insert(
        FIELD_POLYMORPHIC.to_string(),
        Value::Bool(payload.polymorphic),
    );
    Ok(Value::Object(obj))
}

pub fn from_json(value: &Value) -> Result<PermissionCardValuePayload, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    for key in obj.keys() {
        if ![
            FIELD_CARD_ID,
            FIELD_PARENT_IDS,
            FIELD_EXPIRES_AT,
            FIELD_POLYMORPHIC,
        ]
        .contains(&key.as_str())
        {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    let card_id = uuid::Uuid::parse_str(
        obj.get(FIELD_CARD_ID)
            .ok_or(ParseError::MissingField("cardId"))?
            .as_str()
            .ok_or(ParseError::TypeField {
                expected: "string",
                field: Some("cardId"),
            })?,
    )
    .map_err(|e| ParseError::BadFormat(format!("invalid UUID: {e}")))?;
    let parent_ids = obj
        .get(FIELD_PARENT_IDS)
        .map(|v| {
            v.as_array()
                .ok_or(ParseError::TypeField {
                    expected: "array",
                    field: Some("parentIds"),
                })?
                .iter()
                .map(|id| {
                    uuid::Uuid::parse_str(id.as_str().ok_or(ParseError::TypeField {
                        expected: "string",
                        field: Some("parentIds"),
                    })?)
                    .map_err(|e| ParseError::BadFormat(format!("invalid UUID: {e}")))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let expires_at = match obj.get(FIELD_EXPIRES_AT) {
        Some(v) => Some(datetime::from_json(v).map_err(|e| ParseError::Nested(Box::new(e)))?),
        None => None,
    };
    let polymorphic = obj
        .get(FIELD_POLYMORPHIC)
        .map(|v| {
            v.as_bool().ok_or(ParseError::TypeField {
                expected: "boolean",
                field: Some("polymorphic"),
            })
        })
        .transpose()?
        .unwrap_or(false);
    Ok(PermissionCardValuePayload {
        card_id,
        parent_ids,
        expires_at,
        polymorphic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use proptest::prelude::*;
    use test_r::test;

    fn datetime_strategy() -> impl Strategy<Value = DateTime<Utc>> {
        (0i64..253_402_214_400i64, 0u32..1_000_000_000).prop_map(|(s, n)| {
            Utc.timestamp_opt(s, n)
                .single()
                .expect("strategy bounds keep timestamps valid")
        })
    }

    fn payload_strategy() -> impl Strategy<Value = PermissionCardValuePayload> {
        (
            any::<u64>(),
            any::<u64>(),
            proptest::collection::vec((any::<u64>(), any::<u64>()), 0..4),
            proptest::option::of(datetime_strategy()),
            any::<bool>(),
        )
            .prop_map(|(hi, lo, parents, expires_at, polymorphic)| {
                PermissionCardValuePayload {
                    card_id: uuid::Uuid::from_u64_pair(hi, lo),
                    parent_ids: parents
                        .into_iter()
                        .map(|(h, l)| uuid::Uuid::from_u64_pair(h, l))
                        .collect(),
                    expires_at,
                    polymorphic,
                }
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
        assert!(matches!(
            from_text("garbage"),
            Err(ParseError::BadFormat(_))
        ));
    }

    #[test]
    fn prefix_only_rejected() {
        assert_eq!(from_text("permission-card:"), Err(ParseError::Empty));
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
    fn json_missing_card_id() {
        let v = serde_json::json!({
            "parentIds": [],
            "polymorphic": false
        });
        assert_eq!(from_json(&v), Err(ParseError::MissingField("cardId")));
    }

    #[test]
    fn json_extra_field_rejected() {
        let v = serde_json::json!({
            "cardId": "00000000-0000-0000-0000-000000000000",
            "polymorphic": true,
            "stowaway": 1
        });
        assert_eq!(
            from_json(&v),
            Err(ParseError::ExtraField("stowaway".into()))
        );
    }

    #[test]
    fn json_defaults_parent_ids_and_polymorphic() {
        let v = serde_json::json!({
            "cardId": "00000000-0000-0000-0000-000000000000"
        });
        let p = from_json(&v).expect("defaults");
        assert!(p.parent_ids.is_empty());
        assert!(!p.polymorphic);
        assert!(p.expires_at.is_none());
    }

    #[test]
    fn invalid_card_id_rejected() {
        let v = serde_json::json!({
            "cardId": "not a uuid",
            "polymorphic": false
        });
        assert!(matches!(from_json(&v), Err(ParseError::BadFormat(_))));
    }
}
