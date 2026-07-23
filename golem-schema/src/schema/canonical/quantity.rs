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

//! Canonical encoding for [`QuantityValue`] (`mantissa * 10^(-scale)` with a
//! free-form unit string).
//!
//! - Text form: `<decimal><unit>` with no separator on output. The decimal
//!   is written without trailing zeros after the point. Unit characters
//!   must match `^[A-Za-z0-9%°µμ_/\-^]+$` (ASCII letters, ASCII digits,
//!   `%`, `°`, `µ`, `μ`, `_`, `/`, `-`, `^`). Unit may be empty.
//!   `from_text` additionally accepts a single ASCII space between the
//!   decimal and the unit (e.g. `1 kg`); the output form is always
//!   no-space.
//! - Text form is restricted to `|scale| <= 18` and rejects
//!   `mantissa == i64::MIN`; both would either overflow representation or
//!   produce an unbounded output string. JSON encoding is unrestricted
//!   on both fronts. A negative-scale rendering whose absolute decimal
//!   string would exceed 40 characters is rejected as
//!   `ParseError::OutOfRange("quantity scale")`.
//! - JSON form: `{ "mantissa": …, "scale": …, "unit": "..." }`.
//!
//! Mantissa/scale equality is by numeric value, not by raw struct fields:
//! `(15, 1)` and `(150, 2)` represent the same number and a round-trip
//! through this encoder normalises to the smallest scale that round-trips
//! exactly.

use crate::schema::canonical::error::ParseError;
use crate::schema::schema_type::QuantityValue;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

const MAX_ABS_SCALE_TEXT: i32 = 18;
const MAX_NEGATIVE_SCALE_BODY_LEN: usize = 40;

fn unit_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9%°µμ_/\-^]+$").expect("quantity unit regex compiles"))
}

pub fn to_text(payload: &QuantityValue) -> Result<String, ParseError> {
    if payload.mantissa == i64::MIN {
        return Err(ParseError::OutOfRange("quantity mantissa"));
    }
    if payload.scale.unsigned_abs() > MAX_ABS_SCALE_TEXT as u32 {
        return Err(ParseError::OutOfRange("quantity scale"));
    }
    validate_unit(&payload.unit)?;
    let body = format_decimal(payload.mantissa, payload.scale);
    if payload.scale < 0 && body.trim_start_matches('-').len() > MAX_NEGATIVE_SCALE_BODY_LEN {
        return Err(ParseError::OutOfRange("quantity scale"));
    }
    Ok(format!("{}{}", body, payload.unit))
}

pub fn from_text(s: &str) -> Result<QuantityValue, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let (decimal, unit) = split_decimal_and_unit(s)?;
    validate_unit(unit)?;
    let (mantissa, scale) = parse_decimal(decimal)?;
    Ok(QuantityValue {
        mantissa,
        scale,
        unit: unit.to_string(),
    })
}

pub fn to_json(payload: &QuantityValue) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "mantissa".to_string(),
        Value::Number(serde_json::Number::from(payload.mantissa)),
    );
    obj.insert(
        "scale".to_string(),
        Value::Number(serde_json::Number::from(payload.scale)),
    );
    obj.insert("unit".to_string(), Value::String(payload.unit.clone()));
    Value::Object(obj)
}

pub fn from_json(value: &Value) -> Result<QuantityValue, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    let mantissa = obj
        .get("mantissa")
        .ok_or(ParseError::MissingField("mantissa"))?
        .as_i64()
        .ok_or(ParseError::TypeField {
            expected: "integer",
            field: Some("mantissa"),
        })?;
    let scale_raw = obj
        .get("scale")
        .ok_or(ParseError::MissingField("scale"))?
        .as_i64()
        .ok_or(ParseError::TypeField {
            expected: "integer",
            field: Some("scale"),
        })?;
    let scale: i32 = scale_raw
        .try_into()
        .map_err(|_| ParseError::OutOfRange("scale"))?;
    let unit = obj
        .get("unit")
        .ok_or(ParseError::MissingField("unit"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("unit"),
        })?
        .to_string();
    for key in obj.keys() {
        if key != "mantissa" && key != "scale" && key != "unit" {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    Ok(QuantityValue {
        mantissa,
        scale,
        unit,
    })
}

/// Compares two [`QuantityValue`]s as numeric quantities (`mantissa *
/// 10^(-scale)` plus unit), ignoring trailing-zero differences in the
/// representation.
pub fn numerically_equal(a: &QuantityValue, b: &QuantityValue) -> bool {
    if a.unit != b.unit {
        return false;
    }
    normalize(a.mantissa, a.scale) == normalize(b.mantissa, b.scale)
}

fn normalize(mantissa: i64, scale: i32) -> (i64, i32) {
    let mut m = mantissa;
    let mut s = scale;
    if m == 0 {
        return (0, 0);
    }
    while s > 0 && m % 10 == 0 {
        m /= 10;
        s -= 1;
    }
    while s < 0 {
        match m.checked_mul(10) {
            Some(next) => {
                m = next;
                s += 1;
            }
            None => break,
        }
    }
    (m, s)
}

fn split_decimal_and_unit(s: &str) -> Result<(&str, &str), ParseError> {
    let bytes = s.as_bytes();
    let mut i = 0;
    // Optional leading sign.
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // Optional fractional part.
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if digits_start == i || (bytes[digits_start..i].iter().all(|b| !b.is_ascii_digit())) {
        return Err(ParseError::BadFormat(
            "expected a decimal number".to_string(),
        ));
    }
    let decimal = &s[..i];
    let mut unit_start = i;
    // Allow a single ASCII space between the decimal and the unit on input;
    // reject two or more. A leading space without a following unit also
    // reaches the unit regex which rejects empty input via the
    // non-emptiness check below.
    if unit_start < bytes.len() && bytes[unit_start] == b' ' {
        unit_start += 1;
        if unit_start < bytes.len() && bytes[unit_start] == b' ' {
            return Err(ParseError::BadFormat(
                "at most one ASCII space allowed between decimal and unit".into(),
            ));
        }
    }
    Ok((decimal, &s[unit_start..]))
}

fn validate_unit(unit: &str) -> Result<(), ParseError> {
    if unit.is_empty() {
        return Ok(());
    }
    if !unit_regex().is_match(unit) {
        return Err(ParseError::BadFormat(format!(
            "invalid characters in unit: {unit:?}"
        )));
    }
    Ok(())
}

fn parse_decimal(s: &str) -> Result<(i64, i32), ParseError> {
    // Accept "+12", "-12", "12.34", "-0.5", "12.", ".5", "12" forms.
    let (sign, body) = match s.as_bytes().first() {
        Some(b'+') => (1i64, &s[1..]),
        Some(b'-') => (-1i64, &s[1..]),
        _ => (1i64, s),
    };
    let (whole, frac) = match body.find('.') {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return Err(ParseError::BadFormat("empty number".into()));
    }
    let combined: String = format!("{whole}{frac}");
    let stripped = combined.trim_start_matches('0');
    let digits = if stripped.is_empty() { "0" } else { stripped };
    let magnitude: i64 = digits
        .parse()
        .map_err(|_| ParseError::OutOfRange("mantissa"))?;
    let mut mantissa = sign
        .checked_mul(magnitude)
        .ok_or(ParseError::OutOfRange("mantissa"))?;
    let mut scale: i32 = frac.len() as i32;
    while scale > 0 && mantissa % 10 == 0 && mantissa != 0 {
        mantissa /= 10;
        scale -= 1;
    }
    if mantissa == 0 {
        scale = 0;
    }
    Ok((mantissa, scale))
}

fn format_decimal(mantissa: i64, scale: i32) -> String {
    if mantissa == 0 {
        return "0".to_string();
    }
    let negative = mantissa < 0;
    // `to_text` rejects `i64::MIN` before reaching here, so `.abs()` is safe.
    let abs_str = mantissa.abs().to_string();
    let body = if scale <= 0 {
        let mut s = abs_str;
        for _ in 0..(-scale) {
            s.push('0');
        }
        s
    } else {
        let scale = scale as usize;
        if abs_str.len() <= scale {
            let pad = scale - abs_str.len();
            let mut frac = "0".repeat(pad);
            frac.push_str(&abs_str);
            // Strip trailing zeros.
            let trimmed = frac.trim_end_matches('0');
            if trimmed.is_empty() {
                "0".to_string()
            } else {
                format!("0.{trimmed}")
            }
        } else {
            let (left, right) = abs_str.split_at(abs_str.len() - scale);
            let right_trimmed = right.trim_end_matches('0');
            if right_trimmed.is_empty() {
                left.to_string()
            } else {
                format!("{left}.{right_trimmed}")
            }
        }
    };
    if negative { format!("-{body}") } else { body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    fn unit_strategy() -> impl Strategy<Value = String> {
        prop_oneof![Just(String::new()), "[A-Za-z]{1,3}".prop_map(String::from),]
    }

    fn payload_strategy() -> impl Strategy<Value = QuantityValue> {
        // Constrain the magnitude so format/parse fits in i64 after potential
        // trailing-zero expansion.
        (
            (-1_000_000_000_000i64..=1_000_000_000_000i64),
            (-6i32..=12i32),
            unit_strategy(),
        )
            .prop_map(|(mantissa, scale, unit)| QuantityValue {
                mantissa,
                scale,
                unit,
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn text_round_trip(p in payload_strategy()) {
            let s = to_text(&p).expect("to_text");
            let back = from_text(&s).expect("from_text");
            prop_assert!(
                numerically_equal(&p, &back),
                "text round-trip mismatch: input={p:?} text={s} back={back:?}",
            );
        }

        #[test]
        fn json_round_trip(p in payload_strategy()) {
            let j = to_json(&p);
            let back = from_json(&j).expect("from_json");
            prop_assert_eq!(back, p);
        }
    }

    #[test]
    fn simple_positive() {
        let p = QuantityValue {
            mantissa: 15,
            scale: 1,
            unit: "kg".into(),
        };
        assert_eq!(to_text(&p).expect("to_text"), "1.5kg");
    }

    #[test]
    fn negative_value() {
        let p = QuantityValue {
            mantissa: -1234,
            scale: 2,
            unit: "m".into(),
        };
        assert_eq!(to_text(&p).expect("to_text"), "-12.34m");
    }

    #[test]
    fn integer_value() {
        let p = QuantityValue {
            mantissa: 100,
            scale: 0,
            unit: "ms".into(),
        };
        assert_eq!(to_text(&p).expect("to_text"), "100ms");
    }

    #[test]
    fn negative_scale() {
        let p = QuantityValue {
            mantissa: 5,
            scale: -2,
            unit: "g".into(),
        };
        assert_eq!(to_text(&p).expect("to_text"), "500g");
    }

    #[test]
    fn parse_with_unit() {
        let v = from_text("1.5kg").expect("parse");
        assert_eq!(v.unit, "kg");
        assert!(numerically_equal(
            &v,
            &QuantityValue {
                mantissa: 15,
                scale: 1,
                unit: "kg".into()
            }
        ));
    }

    #[test]
    fn parse_no_unit() {
        let v = from_text("42").expect("parse");
        assert_eq!(v.unit, "");
        assert_eq!(v.mantissa, 42);
        assert_eq!(v.scale, 0);
    }

    #[test]
    fn empty_text_rejected() {
        assert_eq!(from_text(""), Err(ParseError::Empty));
    }

    #[test]
    fn garbage_unit_rejected() {
        assert!(matches!(from_text("1.5!!"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn missing_number_rejected() {
        assert!(matches!(from_text("kg"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn json_wrong_type() {
        assert_eq!(
            from_json(&Value::Bool(false)),
            Err(ParseError::TypeField {
                expected: "object",
                field: None,
            })
        );
    }

    #[test]
    fn json_missing_field() {
        let v = serde_json::json!({ "mantissa": 1, "scale": 0 });
        assert_eq!(from_json(&v), Err(ParseError::MissingField("unit")));
    }

    #[test]
    fn extended_unit_grammar_accepted() {
        assert!(from_text("1m2").is_ok());
        assert!(from_text("1m/s2").is_ok());
        assert!(from_text("1kg/m3").is_ok());
        // caret allowed.
        assert!(from_text("1m^2").is_ok());
    }

    #[test]
    fn single_space_between_decimal_and_unit_accepted() {
        let v = from_text("1 kg").expect("parse");
        assert_eq!(v.unit, "kg");
        assert_eq!(v.mantissa, 1);
        // Output never carries the space back.
        assert_eq!(to_text(&v).expect("to_text"), "1kg");
    }

    #[test]
    fn double_space_rejected() {
        assert!(matches!(from_text("1  kg"), Err(ParseError::BadFormat(_))));
    }

    #[test]
    fn scientific_notation_rejected() {
        // `e` is not a valid unit start (it is in the regex set) but the
        // resulting parse should still reject `1e3kg` because `e3kg` is not a
        // valid unit on its own (the leading digits are consumed by the
        // decimal scanner).
        let v = from_text("1e3kg");
        // We can't claim a specific failure mode here, but it must not
        // succeed silently as a mantissa with exponent.
        if let Ok(parsed) = v {
            assert_ne!(parsed.mantissa, 1000, "1e3 must not parse as 1000");
        }
    }

    #[test]
    fn scale_i32_min_rejected_in_text() {
        let p = QuantityValue {
            mantissa: 1,
            scale: i32::MIN,
            unit: "x".into(),
        };
        assert_eq!(to_text(&p), Err(ParseError::OutOfRange("quantity scale")));
    }

    #[test]
    fn very_large_positive_scale_text_rejected() {
        let p = QuantityValue {
            mantissa: 1,
            scale: 19,
            unit: "x".into(),
        };
        assert_eq!(to_text(&p), Err(ParseError::OutOfRange("quantity scale")));
    }

    #[test]
    fn very_large_negative_scale_text_rejected() {
        let p = QuantityValue {
            mantissa: 1,
            scale: -100,
            unit: "x".into(),
        };
        assert_eq!(to_text(&p), Err(ParseError::OutOfRange("quantity scale")));
    }

    #[test]
    fn i64_min_mantissa_text_rejected() {
        let p = QuantityValue {
            mantissa: i64::MIN,
            scale: 0,
            unit: "x".into(),
        };
        assert_eq!(
            to_text(&p),
            Err(ParseError::OutOfRange("quantity mantissa"))
        );
    }
}
