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

use super::*;
use jiff::civil::date;
use proptest::prelude::*;
use test_r::test;

fn datetime_strategy() -> impl Strategy<Value = Timestamp> {
    // Keep generated instants within both RFC 3339 and Jiff's ranges.
    (0i64..=Timestamp::MAX.as_second(), 0u32..1_000_000_000).prop_map(|(s, n)| {
        Timestamp::new(s, n as i32).expect("strategy bounds keep timestamps valid")
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
fn leap_seconds_are_rejected_without_changing_the_instant() {
    for input in [
        "2016-12-31T23:59:60Z",
        "2016-12-31T23:59:60.123456789Z",
        "2017-01-01T00:59:60+01:00",
    ] {
        assert!(matches!(from_text(input), Err(ParseError::BadFormat(_))));
        assert!(from_json(&Value::String(input.into())).is_err());
    }
    let previous = "2016-12-31T23:59:59.123456789Z";
    assert_eq!(to_text(&from_text(previous).unwrap()).unwrap(), previous);
    assert_eq!(
        from_text(&to_text(&Timestamp::MAX).unwrap()).unwrap(),
        Timestamp::MAX
    );
}

#[test]
fn non_rfc3339_iso8601_forms_rejected() {
    for input in [
        "2025-04-12T13:14Z",
        "-000001-04-12T13:14:15Z",
        "2025-04-12T13:14:15Z[UTC]",
        "2025-04-12T13:14:15+0200",
        "2025-04-12T13:14:15.Z",
        "2025-04-12T13:14:15+24:00",
        "2025-04-12T13:14:15-24:00",
        "2025-04-12T13:14:15+23:60",
    ] {
        assert!(from_text(input).is_err(), "accepted {input}");
    }
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
    assert_eq!(to_text(&v).unwrap(), "2025-04-12T11:14:15.000000000Z");
}

fn dt_at_year(year: i16) -> Timestamp {
    date(year, 1, 1)
        .at(0, 0, 0, 0)
        .to_zoned_datetime(jiff::tz::TimeZone::UTC)
        .unwrap()
        .timestamp()
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
    assert!("+010000-01-01T00:00:00Z".parse::<Timestamp>().is_err());
}
