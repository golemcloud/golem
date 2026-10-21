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

//! Canonical encoding for [`TextValuePayload`].
//!
//! - Text form: just the body string. The text form is **intentionally
//!   lossy** for the `language` field: the optional language tag is **not**
//!   carried in the text encoding; `from_text` always yields
//!   `language: None`.
//! - JSON form: `{ "text": "...", "language": "en"? }`. `language` is
//!   absent (not `null`) on output when `None`; the decoder rejects an
//!   explicit `language: null`.

use crate::schema::canonical::error::ParseError;
use crate::schema::schema_value::TextValuePayload;
use serde_json::{Map, Value};

pub fn to_text(payload: &TextValuePayload) -> String {
    payload.text.clone()
}

pub fn from_text(s: &str) -> Result<TextValuePayload, ParseError> {
    Ok(TextValuePayload {
        text: s.to_string(),
        language: None,
    })
}

pub fn to_json(payload: &TextValuePayload) -> Value {
    let mut obj = Map::new();
    obj.insert("text".to_string(), Value::String(payload.text.clone()));
    if let Some(lang) = &payload.language {
        obj.insert("language".to_string(), Value::String(lang.clone()));
    }
    Value::Object(obj)
}

pub fn from_json(value: &Value) -> Result<TextValuePayload, ParseError> {
    let obj = value.as_object().ok_or(ParseError::TypeField {
        expected: "object",
        field: None,
    })?;
    let text = obj
        .get("text")
        .ok_or(ParseError::MissingField("text"))?
        .as_str()
        .ok_or(ParseError::TypeField {
            expected: "string",
            field: Some("text"),
        })?
        .to_string();
    let language = match obj.get("language") {
        Some(Value::String(s)) => Some(s.clone()),
        None => None,
        Some(_) => {
            return Err(ParseError::TypeField {
                expected: "string",
                field: Some("language"),
            });
        }
    };
    for key in obj.keys() {
        if key != "text" && key != "language" {
            return Err(ParseError::ExtraField(key.clone()));
        }
    }
    Ok(TextValuePayload { text, language })
}

#[cfg(test)]
mod tests;
