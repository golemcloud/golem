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
//!     "environmentId": "<uuid>",
//!     "resourceName": "...",
//!     "expectedUse": "<u64-as-string>",
//!     "lastCredit": "<i64-as-string>",
//!     "lastCreditAt": "<rfc3339>"
//!   }
//!   ```
//!   `environmentId` is the canonical hyphenated UUID string.
//!   `expectedUse` and `lastCredit` are written as JSON **strings** on
//!   output (so JavaScript-side parsers can preserve the full u64 / i64
//!   range without precision loss). On input both a JSON number and a JSON
//!   string are accepted for those two fields.

use crate::model::EnvironmentId;
use crate::schema::canonical::datetime;
use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::QuotaTokenValuePayload;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value};

const TEXT_PREFIX: &str = "quota-token:";

const FIELD_ENV_ID: &str = "environmentId";
const FIELD_RESOURCE: &str = "resourceName";
const FIELD_EXPECTED_USE: &str = "expectedUse";
const FIELD_LAST_CREDIT: &str = "lastCredit";
const FIELD_LAST_CREDIT_AT: &str = "lastCreditAt";

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
        Value::String(payload.environment_id.uuid.hyphenated().to_string()),
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
        .ok_or(ParseError::MissingField("environmentId"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("environmentId"),
        })?;
    let environment_id = EnvironmentId::new(
        uuid::Uuid::parse_str(env_id_str)
            .map_err(|e| ParseError::BadFormat(format!("invalid UUID: {e}")))?,
    );
    let resource_name = obj
        .get(FIELD_RESOURCE)
        .ok_or(ParseError::MissingField("resourceName"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("resourceName"),
        })?
        .to_string();
    let expected_use = parse_u64_field(
        obj.get(FIELD_EXPECTED_USE)
            .ok_or(ParseError::MissingField("expectedUse"))?,
        "expectedUse",
    )?;
    let last_credit = parse_i64_field(
        obj.get(FIELD_LAST_CREDIT)
            .ok_or(ParseError::MissingField("lastCredit"))?,
        "lastCredit",
    )?;
    let last_credit_at = datetime::from_json(
        obj.get(FIELD_LAST_CREDIT_AT)
            .ok_or(ParseError::MissingField("lastCreditAt"))?,
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
mod tests;
