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

//! Canonical encoding for [`DurationValuePayload`] (signed total nanoseconds).
//!
//! - Text form: ISO 8601 duration (`P[nD]T[nH][nM][nS]`), with `-` prefix
//!   for negative values. Fractional seconds carry up to 9 digits;
//!   `from_text` rejects fractions longer than 9 digits and requires at
//!   least one digit before or after the decimal point. `PT` with no time
//!   units is rejected. Time units must appear in the strict order
//!   `H < M < S` and may not repeat.
//! - `from_text` also accepts the shorthand forms `<N>ns`, `<N>us`,
//!   `<N>ms`, `<N>s` (integer N, optional leading `-`). The shorthand set
//!   is intentionally limited to these four units.
//! - JSON form: a string in the ISO 8601 form on output; `from_json`
//!   accepts either the same string form or the object
//!   `{ "nanoseconds": N }`.

use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::DurationValuePayload;
use serde_json::Value;

const NS_PER_US: i128 = 1_000;
const NS_PER_MS: i128 = 1_000_000;
const NS_PER_S: i128 = 1_000_000_000;
const NS_PER_MIN: i128 = 60 * NS_PER_S;
const NS_PER_HOUR: i128 = 60 * NS_PER_MIN;
const NS_PER_DAY: i128 = 24 * NS_PER_HOUR;

pub fn to_text(payload: &DurationValuePayload) -> String {
    let total = payload.nanoseconds as i128;
    if total == 0 {
        return "PT0S".to_string();
    }
    let negative = total < 0;
    let mut abs = total.unsigned_abs() as i128;

    let days = abs / NS_PER_DAY;
    abs %= NS_PER_DAY;
    let hours = abs / NS_PER_HOUR;
    abs %= NS_PER_HOUR;
    let minutes = abs / NS_PER_MIN;
    abs %= NS_PER_MIN;
    let seconds = abs / NS_PER_S;
    let nanos = (abs % NS_PER_S) as u32;

    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push('P');
    if days != 0 {
        out.push_str(&format!("{days}D"));
    }
    let time_part_present = hours != 0 || minutes != 0 || seconds != 0 || nanos != 0;
    if time_part_present {
        out.push('T');
        if hours != 0 {
            out.push_str(&format!("{hours}H"));
        }
        if minutes != 0 {
            out.push_str(&format!("{minutes}M"));
        }
        if seconds != 0 || nanos != 0 {
            if nanos == 0 {
                out.push_str(&format!("{seconds}S"));
            } else {
                let mut frac = format!("{nanos:09}");
                while frac.ends_with('0') {
                    frac.pop();
                }
                out.push_str(&format!("{seconds}.{frac}S"));
            }
        }
    }
    out
}

pub fn from_text(s: &str) -> Result<DurationValuePayload, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    if let Some(p) = parse_shorthand(s)? {
        return Ok(p);
    }
    parse_iso8601(s)
}

pub fn to_json(payload: &DurationValuePayload) -> Value {
    Value::String(to_text(payload))
}

pub fn from_json(value: &Value) -> Result<DurationValuePayload, ParseError> {
    match value {
        Value::String(s) => from_text(s),
        Value::Object(obj) => {
            for key in obj.keys() {
                if key != "nanoseconds" {
                    return Err(ParseError::ExtraField(key.clone()));
                }
            }
            let ns = obj
                .get("nanoseconds")
                .ok_or(ParseError::MissingField("nanoseconds"))?;
            let n = ns.as_i64().ok_or(ParseError::TypeField {
                expected: "integer",
                field: Some("nanoseconds"),
            })?;
            Ok(DurationValuePayload { nanoseconds: n })
        }
        _ => Err(ParseError::TypeField {
            expected: "string or object",
            field: None,
        }),
    }
}

fn parse_shorthand(s: &str) -> Result<Option<DurationValuePayload>, ParseError> {
    // The shorthand forms are <int>ns / <int>us / <int>ms / <int>s with an
    // optional leading sign and at least one digit. The presence of the ISO
    // 8601 leader letter `P` (with or without a `-` sign) rules out
    // shorthand.
    let trimmed = s.trim();
    if trimmed.starts_with('P') || trimmed.starts_with("-P") || trimmed.starts_with("+P") {
        return Ok(None);
    }
    let (unit_len, factor) = if let Some(rest) = trimmed.strip_suffix("ns") {
        if rest.is_empty() {
            return Ok(None);
        }
        (2usize, 1i128)
    } else if let Some(rest) = trimmed.strip_suffix("us") {
        if rest.is_empty() {
            return Ok(None);
        }
        (2usize, NS_PER_US)
    } else if let Some(rest) = trimmed.strip_suffix("ms") {
        if rest.is_empty() {
            return Ok(None);
        }
        (2usize, NS_PER_MS)
    } else if let Some(rest) = trimmed.strip_suffix('s') {
        if rest.is_empty() {
            return Ok(None);
        }
        (1usize, NS_PER_S)
    } else {
        return Ok(None);
    };
    let num_str = &trimmed[..trimmed.len() - unit_len];
    let n: i128 = num_str
        .parse()
        .map_err(|_| ParseError::BadFormat(format!("invalid shorthand number: {num_str}")))?;
    let total = n
        .checked_mul(factor)
        .ok_or(ParseError::OutOfRange("duration shorthand"))?;
    let ns: i64 = total
        .try_into()
        .map_err(|_| ParseError::OutOfRange("duration nanoseconds"))?;
    Ok(Some(DurationValuePayload { nanoseconds: ns }))
}

fn parse_iso8601(s: &str) -> Result<DurationValuePayload, ParseError> {
    let (negative, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r)
    } else {
        (false, s)
    };
    let body = rest
        .strip_prefix('P')
        .ok_or_else(|| ParseError::BadFormat("ISO 8601 duration must start with 'P'".into()))?;

    // Split on optional 'T' designator.
    let (date_part, time_part) = match body.find('T') {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };

    let mut total: i128 = 0;
    let mut saw_any = false;

    // Date part supports D only at this layer (Y/M ambiguous; not supported).
    if !date_part.is_empty() {
        let (n, unit, leftover) = take_int_then_unit(date_part)?;
        if !leftover.is_empty() {
            return Err(ParseError::BadFormat(format!(
                "unexpected trailing input in date section: {leftover}"
            )));
        }
        if unit != 'D' {
            return Err(ParseError::BadFormat(format!(
                "unsupported date unit: {unit}"
            )));
        }
        total = total
            .checked_add(
                n.checked_mul(NS_PER_DAY)
                    .ok_or(ParseError::OutOfRange("days"))?,
            )
            .ok_or(ParseError::OutOfRange("duration"))?;
        saw_any = true;
    }

    if let Some(mut tp) = time_part {
        if tp.is_empty() {
            return Err(ParseError::BadFormat("empty time section after 'T'".into()));
        }
        let mut last_order: Option<u8> = None;
        let mut saw_time_unit = false;
        while !tp.is_empty() {
            // Allow a fractional component on the seconds field only.
            let (digits_end, has_dot, trailing_dot) = scan_decimal(tp);
            if digits_end == 0 {
                return Err(ParseError::BadFormat(format!(
                    "expected digits in time section: {tp}"
                )));
            }
            if trailing_dot {
                return Err(ParseError::BadFormat(
                    "decimal point requires digits on both sides".into(),
                ));
            }
            let unit_byte = tp.as_bytes().get(digits_end).copied().ok_or_else(|| {
                ParseError::BadFormat("missing unit after digits in time section".into())
            })?;
            let unit = unit_byte as char;
            let number_str = &tp[..digits_end];
            let order = match unit {
                'H' => 1,
                'M' => 2,
                'S' => 3,
                _ => {
                    return Err(ParseError::BadFormat(format!(
                        "unsupported time unit: {unit}"
                    )));
                }
            };
            if let Some(prev) = last_order
                && prev >= order
            {
                return Err(ParseError::BadFormat(
                    "time units out of order or duplicated".into(),
                ));
            }
            last_order = Some(order);
            if has_dot && unit != 'S' {
                return Err(ParseError::BadFormat(
                    "fractional value only allowed on seconds".into(),
                ));
            }
            let contribution = match unit {
                'H' => {
                    let n: i128 = number_str.parse().map_err(|_| {
                        ParseError::BadFormat(format!("invalid hours: {number_str}"))
                    })?;
                    n.checked_mul(NS_PER_HOUR)
                        .ok_or(ParseError::OutOfRange("hours"))?
                }
                'M' => {
                    let n: i128 = number_str.parse().map_err(|_| {
                        ParseError::BadFormat(format!("invalid minutes: {number_str}"))
                    })?;
                    n.checked_mul(NS_PER_MIN)
                        .ok_or(ParseError::OutOfRange("minutes"))?
                }
                'S' => parse_seconds_with_fraction(number_str)?,
                _ => unreachable!(),
            };
            total = total
                .checked_add(contribution)
                .ok_or(ParseError::OutOfRange("duration"))?;
            saw_any = true;
            saw_time_unit = true;
            tp = &tp[digits_end + 1..];
        }
        if !saw_time_unit {
            return Err(ParseError::BadFormat(
                "time section requires at least one H/M/S unit".into(),
            ));
        }
    }

    if !saw_any {
        return Err(ParseError::BadFormat("empty duration".into()));
    }

    if negative {
        total = -total;
    }

    let ns: i64 = total
        .try_into()
        .map_err(|_| ParseError::OutOfRange("duration nanoseconds"))?;
    Ok(DurationValuePayload { nanoseconds: ns })
}

fn take_int_then_unit(s: &str) -> Result<(i128, char, &str), ParseError> {
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            end = i + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return Err(ParseError::BadFormat(format!("expected digits in: {s}")));
    }
    let n: i128 = s[..end]
        .parse()
        .map_err(|_| ParseError::BadFormat(format!("invalid integer: {}", &s[..end])))?;
    let unit_ch = s[end..]
        .chars()
        .next()
        .ok_or_else(|| ParseError::BadFormat("missing unit after digits".into()))?;
    Ok((n, unit_ch, &s[end + unit_ch.len_utf8()..]))
}

/// Scans the leading decimal number in `s` and returns
/// `(end_index, has_dot, trailing_dot)` where `end_index` is the byte
/// offset of the first non-numeric character (the unit letter, normally),
/// and `trailing_dot` indicates that the scan stopped immediately after a
/// `.` with no fractional digits (e.g. `1.`).
fn scan_decimal(s: &str) -> (usize, bool, bool) {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut has_dot = false;
    let mut dot_position: Option<usize> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() {
            i += 1;
        } else if b == b'.' && !has_dot {
            has_dot = true;
            dot_position = Some(i);
            i += 1;
        } else {
            break;
        }
    }
    let trailing_dot = match dot_position {
        Some(pos) => pos + 1 == i,
        None => false,
    };
    (i, has_dot, trailing_dot)
}

fn parse_seconds_with_fraction(s: &str) -> Result<i128, ParseError> {
    let (whole, frac) = match s.find('.') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return Err(ParseError::BadFormat("empty seconds value".into()));
    }
    let whole_n: i128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| ParseError::BadFormat(format!("invalid seconds: {whole}")))?
    };
    let mut nanos: i128 = 0;
    if !frac.is_empty() {
        if frac.len() > 9 {
            return Err(ParseError::BadFormat(
                "fractional seconds limited to 9 digits".into(),
            ));
        }
        let mut padded = String::from(frac);
        while padded.len() < 9 {
            padded.push('0');
        }
        nanos = padded
            .parse()
            .map_err(|_| ParseError::BadFormat(format!("invalid fractional seconds: {frac}")))?;
    }
    let whole_ns = whole_n
        .checked_mul(NS_PER_S)
        .ok_or(ParseError::OutOfRange("seconds"))?;
    whole_ns
        .checked_add(nanos)
        .ok_or(ParseError::OutOfRange("seconds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    fn payload_strategy() -> impl Strategy<Value = DurationValuePayload> {
        any::<i64>().prop_map(|nanoseconds| DurationValuePayload { nanoseconds })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn text_round_trip(p in payload_strategy()) {
            let s = to_text(&p);
            let back = from_text(&s).expect("from_text");
            prop_assert_eq!(back, p);
        }

        #[test]
        fn json_round_trip(p in payload_strategy()) {
            let j = to_json(&p);
            let back = from_json(&j).expect("from_json");
            prop_assert_eq!(back, p);
        }
    }

    #[test]
    fn zero_is_pt0s() {
        assert_eq!(to_text(&DurationValuePayload { nanoseconds: 0 }), "PT0S");
    }

    #[test]
    fn small_positive() {
        let p = DurationValuePayload {
            nanoseconds: 3_004_005_000, // 3.004005 s
        };
        assert_eq!(to_text(&p), "PT3.004005S");
    }

    #[test]
    fn negative_minus_prefix() {
        let p = DurationValuePayload {
            nanoseconds: -(60 * NS_PER_S as i64),
        };
        assert_eq!(to_text(&p), "-PT1M");
    }

    #[test]
    fn days_and_time() {
        let p = DurationValuePayload {
            nanoseconds: (NS_PER_DAY + NS_PER_HOUR + NS_PER_MIN + NS_PER_S) as i64,
        };
        assert_eq!(to_text(&p), "P1DT1H1M1S");
    }

    #[test]
    fn shorthand_ns() {
        assert_eq!(
            from_text("12345ns"),
            Ok(DurationValuePayload { nanoseconds: 12345 })
        );
    }

    #[test]
    fn shorthand_us() {
        assert_eq!(
            from_text("2us"),
            Ok(DurationValuePayload { nanoseconds: 2_000 })
        );
    }

    #[test]
    fn shorthand_ms() {
        assert_eq!(
            from_text("5ms"),
            Ok(DurationValuePayload {
                nanoseconds: 5_000_000
            })
        );
    }

    #[test]
    fn shorthand_s() {
        assert_eq!(
            from_text("3s"),
            Ok(DurationValuePayload {
                nanoseconds: 3 * NS_PER_S as i64
            })
        );
    }

    #[test]
    fn shorthand_negative() {
        assert_eq!(
            from_text("-5ms"),
            Ok(DurationValuePayload {
                nanoseconds: -5_000_000
            })
        );
    }

    #[test]
    fn json_object_form() {
        let v = serde_json::json!({ "nanoseconds": 1234 });
        assert_eq!(
            from_json(&v),
            Ok(DurationValuePayload { nanoseconds: 1234 })
        );
    }

    #[test]
    fn empty_text_rejected() {
        assert_eq!(from_text(""), Err(ParseError::Empty));
    }

    #[test]
    fn garbage_text_rejected() {
        assert!(matches!(from_text("nope"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn json_wrong_type() {
        assert_eq!(
            from_json(&Value::Bool(true)),
            Err(ParseError::TypeField {
                expected: "string or object",
                field: None,
            })
        );
    }

    #[test]
    fn huge_seconds_overflow_rejected() {
        // i64::MAX seconds * 1_000_000_000 ns/s overflows our i128 accumulator
        // path via parse_seconds_with_fraction.
        let s = format!("PT{}S", i64::MAX);
        assert!(matches!(
            from_text(&s),
            Err(ParseError::OutOfRange("seconds") | ParseError::OutOfRange("duration nanoseconds"))
        ));
    }

    #[test]
    fn pt_dot_no_digits_rejected() {
        assert!(matches!(from_text("PT1."), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn empty_time_section_rejected() {
        assert!(matches!(from_text("PT"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn duplicate_or_out_of_order_units_rejected() {
        // S before M: out-of-order.
        assert!(matches!(from_text("PT1S1M"), Err(ParseError::BadFormat(_))));
        // Duplicate M.
        assert!(matches!(from_text("PT1M1M"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn fractional_seconds_too_long_rejected() {
        // 10 fractional digits.
        assert!(matches!(
            from_text("PT0.1234567890S"),
            Err(ParseError::BadFormat(_))
        ));
    }

    #[test]
    fn shorthand_overflow_rejected() {
        assert!(matches!(
            from_text("9999999999999999999999ns"),
            Err(ParseError::BadFormat(_)) | Err(ParseError::OutOfRange(_))
        ));
    }
}
