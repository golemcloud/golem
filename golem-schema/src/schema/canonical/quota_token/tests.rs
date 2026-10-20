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
                    environment_id: EnvironmentId::new(uuid::Uuid::from_u64_pair(hi, lo)),
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
        "environmentId": "00000000-0000-0000-0000-000000000000",
        "resourceName": "r",
        "expectedUse": "1",
        "lastCredit": "0"
    });
    assert_eq!(from_json(&v), Err(ParseError::MissingField("lastCreditAt")));
}

#[test]
fn json_extra_field_rejected() {
    let v = serde_json::json!({
        "environmentId": "00000000-0000-0000-0000-000000000000",
        "resourceName": "r",
        "expectedUse": "1",
        "lastCredit": "0",
        "lastCreditAt": "2025-01-01T00:00:00Z",
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
        "environmentId": "00000000-0000-0000-0000-000000000000",
        "resourceName": "r",
        "expectedUse": 42,
        "lastCredit": -1,
        "lastCreditAt": "2025-01-01T00:00:00Z"
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
        environment_id: EnvironmentId::new(uuid::Uuid::nil()),
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
        "environmentId": "not a uuid",
        "resourceName": "r",
        "expectedUse": "1",
        "lastCredit": "0",
        "lastCreditAt": "2025-01-01T00:00:00Z"
    });
    assert!(matches!(from_json(&v), Err(ParseError::BadFormat(_))));
}

#[test]
fn wrong_expected_use_type_rejected() {
    let v = serde_json::json!({
        "environmentId": "00000000-0000-0000-0000-000000000000",
        "resourceName": "r",
        "expectedUse": true,
        "lastCredit": "0",
        "lastCreditAt": "2025-01-01T00:00:00Z"
    });
    assert_eq!(
        from_json(&v),
        Err(ParseError::TypeField {
            expected: "number or string",
            field: Some("expectedUse"),
        })
    );
}

#[test]
fn wrong_last_credit_type_rejected() {
    let v = serde_json::json!({
        "environmentId": "00000000-0000-0000-0000-000000000000",
        "resourceName": "r",
        "expectedUse": "0",
        "lastCredit": true,
        "lastCreditAt": "2025-01-01T00:00:00Z"
    });
    assert_eq!(
        from_json(&v),
        Err(ParseError::TypeField {
            expected: "number or string",
            field: Some("lastCredit"),
        })
    );
}

#[test]
fn invalid_last_credit_at_rejected() {
    let v = serde_json::json!({
        "environmentId": "00000000-0000-0000-0000-000000000000",
        "resourceName": "r",
        "expectedUse": "0",
        "lastCredit": "0",
        "lastCreditAt": "not a date"
    });
    assert!(matches!(from_json(&v), Err(ParseError::Nested(_))));
}
