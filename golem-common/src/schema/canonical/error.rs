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

use std::fmt::{Display, Formatter};

/// Errors produced by the canonical text / JSON parsers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Input was empty where a non-empty value is required.
    Empty,
    /// Input had the wrong shape (free-form description).
    BadFormat(String),
    /// JSON object was missing a required field.
    MissingField(&'static str),
    /// JSON object carried an unexpected field name.
    ExtraField(String),
    /// JSON value (or field value) did not have the expected JSON type.
    ///
    /// Retained for sites that do not yet know which field they were reading;
    /// new code should prefer [`ParseError::TypeField`].
    Type(&'static str),
    /// JSON value did not have the expected JSON type, with the field name
    /// that produced the failure when applicable.
    TypeField {
        expected: &'static str,
        field: Option<&'static str>,
    },
    /// Bytes were not valid UTF-8 when text was required.
    InvalidUtf8,
    /// Base64 payload could not be decoded.
    InvalidBase64(String),
    /// Number was out of the supported range.
    OutOfRange(&'static str),
    /// Embedded sub-value failed to parse.
    Nested(Box<ParseError>),
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => f.write_str("input is empty"),
            ParseError::BadFormat(msg) => write!(f, "bad format: {msg}"),
            ParseError::MissingField(name) => write!(f, "missing field: {name}"),
            ParseError::ExtraField(name) => write!(f, "unexpected field: {name}"),
            ParseError::Type(expected) => write!(f, "expected {expected}"),
            ParseError::TypeField {
                expected,
                field: Some(name),
            } => write!(f, "expected {expected} for field {name}"),
            ParseError::TypeField {
                expected,
                field: None,
            } => write!(f, "expected {expected}"),
            ParseError::InvalidUtf8 => f.write_str("invalid UTF-8"),
            ParseError::InvalidBase64(msg) => write!(f, "invalid base64: {msg}"),
            ParseError::OutOfRange(what) => write!(f, "out of range: {what}"),
            ParseError::Nested(inner) => write!(f, "{inner}"),
        }
    }
}

impl std::error::Error for ParseError {}
