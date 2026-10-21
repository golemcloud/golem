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
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use test_r::test;

fn payload_strategy() -> impl Strategy<Value = BinaryValuePayload> {
    (vec(any::<u8>(), 0..64), option::of("[a-z]+/[a-z0-9.+-]+"))
        .prop_map(|(bytes, mime_type)| BinaryValuePayload { bytes, mime_type })
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
fn missing_data_prefix() {
    assert!(matches!(from_text("foo"), Err(ParseError::BadFormat(_))));
}

#[test]
fn missing_base64_marker() {
    assert!(matches!(
        from_text("data:text/plain,hello"),
        Err(ParseError::BadFormat(_))
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
fn json_missing_bytes() {
    assert_eq!(
        from_json(&serde_json::json!({})),
        Err(ParseError::MissingField("bytes"))
    );
}

#[test]
fn data_url_no_mime_form() {
    let p = BinaryValuePayload {
        bytes: vec![1, 2, 3],
        mime_type: None,
    };
    assert!(to_text(&p).expect("to_text").starts_with("data:;base64,"));
}

#[test]
fn empty_bytes_data_url_decoded() {
    let v = from_text("data:;base64,").expect("decode empty bytes");
    assert_eq!(
        v,
        BinaryValuePayload {
            bytes: vec![],
            mime_type: None,
        }
    );
}

#[test]
fn invalid_base64_body_rejected() {
    assert!(matches!(
        from_text("data:;base64,!!!"),
        Err(ParseError::InvalidBase64(_))
    ));
}

#[test]
fn empty_mime_type_text_encode_rejected() {
    let p = BinaryValuePayload {
        bytes: vec![1, 2, 3],
        mime_type: Some(String::new()),
    };
    assert!(matches!(to_text(&p), Err(ParseError::BadFormat(_))));
}

#[test]
fn empty_mime_type_json_encode_rejected() {
    let p = BinaryValuePayload {
        bytes: vec![1, 2, 3],
        mime_type: Some(String::new()),
    };
    assert!(matches!(to_json(&p), Err(ParseError::BadFormat(_))));
}

#[test]
fn empty_mime_type_json_decode_rejected() {
    let v = serde_json::json!({ "bytes": "AAEC", "mimeType": "" });
    assert!(matches!(from_json(&v), Err(ParseError::BadFormat(_))));
}

#[test]
fn standard_base64_text_rejected() {
    // standard base64 with `+/` characters is rejected by the URL-safe
    // decoder.
    assert!(matches!(
        from_text("data:;base64,+/=="),
        Err(ParseError::InvalidBase64(_))
    ));
}

#[test]
fn standard_base64_json_rejected() {
    let v = serde_json::json!({ "bytes": "+/==" });
    assert!(matches!(from_json(&v), Err(ParseError::InvalidBase64(_))));
}

#[test]
fn json_mime_wrong_type_rejected() {
    let v = serde_json::json!({ "bytes": "AA", "mimeType": 42 });
    assert_eq!(
        from_json(&v),
        Err(ParseError::TypeField {
            expected: "string",
            field: Some("mimeType"),
        })
    );
}

#[test]
fn json_mime_null_rejected() {
    let v = serde_json::json!({ "bytes": "AA", "mimeType": null });
    assert_eq!(
        from_json(&v),
        Err(ParseError::TypeField {
            expected: "string",
            field: Some("mimeType"),
        })
    );
}

#[test]
fn json_extra_field_rejected() {
    let v = serde_json::json!({ "bytes": "AA", "extra": true });
    assert_eq!(from_json(&v), Err(ParseError::ExtraField("extra".into())));
}

#[test]
fn json_invalid_mime_rejected() {
    let v = serde_json::json!({ "bytes": "AA", "mimeType": "no slash" });
    assert!(matches!(from_json(&v), Err(ParseError::BadFormat(_))));
}
