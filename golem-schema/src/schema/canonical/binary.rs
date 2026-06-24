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

//! Canonical encoding for [`BinaryValuePayload`].
//!
//! - Text form: RFC 2397-style data URL `data:<mime>;base64,<base64>`. The
//!   base64 alphabet is **URL-safe-no-pad** (Golem-specific; this differs
//!   from the standard `data:` URL convention which uses standard base64
//!   with `+`, `/` and `=` padding). When the payload has no MIME type the
//!   form is `data:;base64,<base64>`. An explicit `Some("")` mime is
//!   rejected at encode time — we never emit a form we would not decode
//!   back to the same payload.
//! - JSON form: `{ "bytes": "<base64>", "mimeType": "...?" }`. The
//!   `mimeType` field is absent (not `null`) on output when `None`; the
//!   decoder rejects an explicit `mimeType: null` and an empty string.
//! - When the MIME type is `Some(s)`, `s` must match a minimal MIME regex
//!   (`type/subtype` with a small ASCII-only character set).

use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::BinaryValuePayload;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

const DATA_PREFIX: &str = "data:";
const BASE64_MARKER: &str = ";base64,";

fn mime_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[A-Za-z0-9!#$&^_.+\-]+/[A-Za-z0-9!#$&^_.+\-]+$").expect("mime regex compiles")
    })
}

fn validate_mime(mime: &str) -> Result<(), ParseError> {
    if mime.is_empty() {
        return Err(ParseError::BadFormat("empty mime_type".into()));
    }
    if !mime_regex().is_match(mime) {
        return Err(ParseError::BadFormat("invalid mime_type".into()));
    }
    Ok(())
}

pub fn to_text(payload: &BinaryValuePayload) -> Result<String, ParseError> {
    if let Some(mime) = payload.mime_type.as_deref() {
        validate_mime(mime)?;
    }
    let mime = payload.mime_type.as_deref().unwrap_or("");
    let encoded = URL_SAFE_NO_PAD.encode(&payload.bytes);
    Ok(format!("{DATA_PREFIX}{mime}{BASE64_MARKER}{encoded}"))
}

pub fn from_text(s: &str) -> Result<BinaryValuePayload, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let rest = s
        .strip_prefix(DATA_PREFIX)
        .ok_or_else(|| ParseError::BadFormat("missing 'data:' prefix".into()))?;
    let marker_at = rest
        .find(BASE64_MARKER)
        .ok_or_else(|| ParseError::BadFormat("missing ';base64,' marker".into()))?;
    let mime = &rest[..marker_at];
    let body = &rest[marker_at + BASE64_MARKER.len()..];
    let bytes = URL_SAFE_NO_PAD
        .decode(body.as_bytes())
        .map_err(|e| ParseError::InvalidBase64(e.to_string()))?;
    let mime_type = if mime.is_empty() {
        None
    } else {
        validate_mime(mime)?;
        Some(mime.to_string())
    };
    Ok(BinaryValuePayload { bytes, mime_type })
}

pub fn to_json(payload: &BinaryValuePayload) -> Result<Value, ParseError> {
    if let Some(mime) = payload.mime_type.as_deref() {
        validate_mime(mime)?;
    }
    let mut obj = Map::new();
    obj.insert(
        "bytes".to_string(),
        Value::String(URL_SAFE_NO_PAD.encode(&payload.bytes)),
    );
    if let Some(mime) = &payload.mime_type {
        obj.insert("mimeType".to_string(), Value::String(mime.clone()));
    }
    Ok(Value::Object(obj))
}

pub fn from_json(value: &Value) -> Result<BinaryValuePayload, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    let b64 = obj
        .get("bytes")
        .ok_or(ParseError::MissingField("bytes"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("bytes"),
        })?;
    let bytes = URL_SAFE_NO_PAD
        .decode(b64.as_bytes())
        .map_err(|e| ParseError::InvalidBase64(e.to_string()))?;
    let mime_type = match obj.get("mimeType") {
        Some(Value::String(s)) => {
            validate_mime(s)?;
            Some(s.clone())
        }
        None => None,
        Some(_) => {
            return Err(ParseError::TypeField {
                expected: "string",
                field: Some("mimeType"),
            });
        }
    };
    for key in obj.keys() {
        if key != "bytes" && key != "mimeType" {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    Ok(BinaryValuePayload { bytes, mime_type })
}

#[cfg(test)]
mod tests {
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
}
