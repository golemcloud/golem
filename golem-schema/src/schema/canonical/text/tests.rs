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
use proptest::option;
use proptest::prelude::*;
use test_r::test;

fn payload_strategy() -> impl Strategy<Value = TextValuePayload> {
    (".*", option::of("[a-z]{2}(-[A-Z]{2})?"))
        .prop_map(|(text, language)| TextValuePayload { text, language })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn text_round_trip(p in payload_strategy()) {
        let s = to_text(&p);
        let back = from_text(&s).expect("from_text");
        prop_assert_eq!(back.text, p.text);
        prop_assert!(back.language.is_none());
    }

    #[test]
    fn json_round_trip(p in payload_strategy()) {
        let j = to_json(&p);
        let back = from_json(&j).expect("from_json");
        prop_assert_eq!(back, p);
    }
}

#[test]
fn json_rejects_non_object() {
    assert_eq!(
        from_json(&Value::Null),
        Err(ParseError::TypeField {
            expected: "object",
            field: None,
        })
    );
    assert_eq!(
        from_json(&Value::String("hi".into())),
        Err(ParseError::TypeField {
            expected: "object",
            field: None,
        })
    );
}

#[test]
fn json_requires_text() {
    assert_eq!(
        from_json(&serde_json::json!({})),
        Err(ParseError::MissingField("text"))
    );
}

#[test]
fn json_rejects_extra_field() {
    let v = serde_json::json!({ "text": "x", "what": "no" });
    assert_eq!(from_json(&v), Err(ParseError::ExtraField("what".into())));
}

#[test]
fn json_language_omitted_when_none() {
    let p = TextValuePayload {
        text: "x".into(),
        language: None,
    };
    let j = to_json(&p);
    assert_eq!(j, serde_json::json!({ "text": "x" }));
}

#[test]
fn json_language_wrong_type_rejected() {
    let v = serde_json::json!({ "text": "x", "language": 42 });
    assert_eq!(
        from_json(&v),
        Err(ParseError::TypeField {
            expected: "string",
            field: Some("language"),
        })
    );
}

#[test]
fn json_language_null_rejected() {
    let v = serde_json::json!({ "text": "x", "language": null });
    assert_eq!(
        from_json(&v),
        Err(ParseError::TypeField {
            expected: "string",
            field: Some("language"),
        })
    );
}
