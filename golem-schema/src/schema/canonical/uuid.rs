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

//! Canonical encoding for [`super::super::schema_value::SchemaValue::Uuid`].

use crate::schema::canonical::error::ParseError;
use serde_json::Value;

pub fn to_text(payload: &uuid::Uuid) -> String {
    payload.hyphenated().to_string()
}

pub fn from_text(s: &str) -> Result<uuid::Uuid, ParseError> {
    uuid::Uuid::parse_str(s).map_err(|error| ParseError::BadFormat(error.to_string()))
}

pub fn to_json(payload: &uuid::Uuid) -> Value {
    Value::String(to_text(payload))
}

pub fn from_json(value: &Value) -> Result<uuid::Uuid, ParseError> {
    match value {
        Value::String(value) => from_text(value),
        _ => Err(ParseError::TypeField {
            expected: "string",
            field: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn canonical_round_trip() {
        let uuid = uuid::Uuid::parse_str("dd00721b-3329-4621-a01d-c71f02cd78c6").unwrap();
        assert_eq!(from_text(&to_text(&uuid)), Ok(uuid));
        assert_eq!(from_json(&to_json(&uuid)), Ok(uuid));
    }

    #[test]
    fn invalid_uuid_is_rejected() {
        assert!(matches!(
            from_text("not-a-uuid"),
            Err(ParseError::BadFormat(_))
        ));
    }
}
