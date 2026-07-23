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
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};
    use proptest::prelude::*;
    use test_r::test;

    fn datetime_strategy() -> impl Strategy<Value = DateTime<Utc>> {
        // RFC 3339 only encodes years 0000-9999, with a four-digit, non-negative
        // year field, so cap the strategy at year 9999.
        (0i64..253_402_214_400i64, 0u32..1_000_000_000).prop_map(|(s, n)| {
            Utc.timestamp_opt(s, n)
                .single()
                .expect("strategy bounds keep timestamps valid")
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn text_round_trip(p in datetime_strategy()) {
            let s = to_text(&p).expect("to_text");
            let back = from_text(&s).expect("from_text");
            prop_assert_eq!(back, p);
        }

        #[test]
        fn json_round_trip(p in datetime_strategy()) {
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
    fn garbage_text_rejected() {
        assert!(matches!(
            from_text("not a date"),
            Err(ParseError::BadFormat(_))
        ));
    }

    #[test]
    fn json_wrong_type() {
        assert_eq!(
            from_json(&Value::Number(serde_json::Number::from(1u64))),
            Err(ParseError::TypeField {
                expected: "string",
                field: None,
            })
        );
    }

    #[test]
    fn accepts_non_utc_offset_and_converts() {
        let v = from_text("2025-04-12T13:14:15+02:00").expect("parse");
        assert_eq!(v.timezone(), Utc);
    }

    fn dt_at_year(year: i32) -> DateTime<Utc> {
        let date = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date");
        date.and_hms_opt(0, 0, 0).expect("valid time").and_utc()
    }

    #[test]
    fn year_zero_accepted() {
        let dt = dt_at_year(0);
        assert!(to_text(&dt).is_ok());
        assert!(to_json(&dt).is_ok());
    }

    #[test]
    fn year_9999_accepted() {
        let dt = dt_at_year(9999);
        assert!(to_text(&dt).is_ok());
        assert!(to_json(&dt).is_ok());
    }

    #[test]
    fn negative_year_rejected() {
        let dt = dt_at_year(-1);
        assert_eq!(to_text(&dt), Err(ParseError::OutOfRange("datetime year")));
        assert_eq!(to_json(&dt), Err(ParseError::OutOfRange("datetime year")));
    }

    #[test]
    fn year_above_max_rejected() {
        let dt = dt_at_year(10_000);
        assert_eq!(to_text(&dt), Err(ParseError::OutOfRange("datetime year")));
        assert_eq!(to_json(&dt), Err(ParseError::OutOfRange("datetime year")));
    }
}
