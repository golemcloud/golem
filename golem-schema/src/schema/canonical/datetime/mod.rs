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
//! payloads (`jiff::Timestamp`).
//!
//! - Text form: RFC 3339 in UTC, with nanosecond precision and a trailing
//!   `Z`, e.g. `2025-04-12T13:14:15.123456789Z`. The canonical form
//!   **preserves nanoseconds**, but restricts the **year domain to
//!   `0000..=9999`** (the RFC 3339 four-digit, non-negative year field).
//!   Instants must also fit in [`jiff::Timestamp`]'s representable range.
//!   Out-of-range datetimes are rejected at encode time as
//!   [`ParseError::OutOfRange`].
//! - JSON form: a JSON string with the same RFC 3339 UTC form.

use crate::schema::canonical::error::ParseError;
use jiff::Timestamp;
use serde_json::Value;

fn check_year_range(payload: &Timestamp) -> Result<(), ParseError> {
    let year = payload.to_zoned_datetime(jiff::tz::TimeZone::UTC).year();
    if !(0..=9999).contains(&year) {
        return Err(ParseError::OutOfRange("datetime year"));
    }
    Ok(())
}

pub fn to_text(payload: &Timestamp) -> Result<String, ParseError> {
    check_year_range(payload)?;
    Ok(format!("{payload:.9}"))
}

pub fn from_text(s: &str) -> Result<Timestamp, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    // Jiff also accepts ISO 8601 forms outside RFC 3339, including omitted
    // seconds, extended years, and time zone annotations.
    let bytes = s.as_bytes();
    let valid_prefix = bytes.len() >= 20
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b't')
        && bytes[13] == b':'
        && bytes[16] == b':';
    let mut offset_start = 19;
    if valid_prefix && bytes[offset_start] == b'.' {
        offset_start += 1;
        let fraction_start = offset_start;
        while bytes.get(offset_start).is_some_and(u8::is_ascii_digit) {
            offset_start += 1;
        }
        if offset_start == fraction_start {
            return Err(ParseError::BadFormat("RFC 3339 expected".into()));
        }
    }
    let valid_offset = matches!(bytes.get(offset_start..), Some([b'Z' | b'z']))
        || matches!(bytes.get(offset_start..), Some([b'+' | b'-', h1, h2, b':', m1, m2])
            if [h1, h2, m1, m2].iter().all(|digit| digit.is_ascii_digit())
                && (*h1 < b'2' || (*h1 == b'2' && *h2 <= b'3'))
                && *m1 <= b'5');
    if !valid_prefix || !valid_offset {
        return Err(ParseError::BadFormat("RFC 3339 expected".into()));
    }
    // Jiff clamps leap seconds to 59; reject them instead of changing the instant.
    if &bytes[17..19] == b"60" {
        return Err(ParseError::BadFormat(
            "leap seconds are not supported".into(),
        ));
    }
    let parsed = s
        .parse::<Timestamp>()
        .map_err(|e| ParseError::BadFormat(format!("RFC 3339 expected: {e}")))?;
    Ok(parsed)
}

pub fn to_json(payload: &Timestamp) -> Result<Value, ParseError> {
    Ok(Value::String(to_text(payload)?))
}

pub fn from_json(value: &Value) -> Result<Timestamp, ParseError> {
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
