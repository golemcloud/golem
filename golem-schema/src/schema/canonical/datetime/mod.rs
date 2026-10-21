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

//! Canonical encoding for [`super::super::schema_value::SchemaValue::Datetime`]
//! payloads (`chrono::DateTime<Utc>`).
//!
//! - Text form: RFC 3339 in UTC, with nanosecond precision and a trailing
//!   `Z`, e.g. `2025-04-12T13:14:15.123456789Z`. The canonical form
//!   **preserves nanoseconds**, but restricts the **year domain to
//!   `0000..=9999`** (the RFC 3339 four-digit, non-negative year field).
//!   Out-of-range datetimes are rejected at encode time as
//!   [`ParseError::OutOfRange`].
//! - JSON form: a JSON string with the same RFC 3339 UTC form.

use crate::schema::canonical::error::ParseError;
use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde_json::Value;

fn check_year_range(payload: &DateTime<Utc>) -> Result<(), ParseError> {
    let year = payload.year();
    if !(0..=9999).contains(&year) {
        return Err(ParseError::OutOfRange("datetime year"));
    }
    Ok(())
}

pub fn to_text(payload: &DateTime<Utc>) -> Result<String, ParseError> {
    check_year_range(payload)?;
    Ok(payload.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

pub fn from_text(s: &str) -> Result<DateTime<Utc>, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let parsed = DateTime::parse_from_rfc3339(s)
        .map_err(|e| ParseError::BadFormat(format!("RFC 3339 expected: {e}")))?;
    Ok(parsed.with_timezone(&Utc))
}

pub fn to_json(payload: &DateTime<Utc>) -> Result<Value, ParseError> {
    Ok(Value::String(to_text(payload)?))
}

pub fn from_json(value: &Value) -> Result<DateTime<Utc>, ParseError> {
    match value {
        Value::String(s) => from_text(s),
        _ => Err(ParseError::TypeField {
            expected: "string",
            field: None,
        }),
    }
}

#[cfg(test)]
mod tests;
