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

//! Shared error type for the renderer family.

use crate::schema::canonical::ParseError;
use std::fmt::{Display, Formatter};

/// Errors produced by the schema/value renderers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// A [`crate::schema::SchemaValue`] did not match the shape required by
    /// the accompanying [`crate::schema::SchemaType`].
    ValueMismatch { path: String, reason: String },
    /// A construct is recognised but not supported by the renderer (e.g.,
    /// `Future` / `Stream` payloads).
    Unsupported(&'static str),
    /// A canonical scalar encoder or decoder rejected the input.
    Canonical(ParseError),
    /// A JSON conversion failed (e.g., a number out of range for the
    /// target integer width).
    Json(String),
    /// A record decoder saw a JSON object with a field that is not declared
    /// by the schema.
    UnexpectedField { record: String, field: String },
    /// A flags decoder saw the same flag name listed twice.
    DuplicateFlag { flag: String },
    /// A union encoder picked a branch by tag but the branch body did not
    /// satisfy the branch's discriminator rule.
    UnionTagMismatch { tag: String, reason: String },
    /// A union decoder found multiple branches whose discriminator rules
    /// match the same incoming value (a structural ambiguity that should
    /// have been caught at validation; runtime is a safety net).
    UnionAmbiguous { matched: Vec<String> },
    /// A union decoder found no branch whose discriminator rule matches the
    /// incoming value.
    UnionNoMatch,
}

impl Display for RenderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::ValueMismatch { path, reason } => {
                write!(f, "value mismatch at {path}: {reason}")
            }
            RenderError::Unsupported(what) => write!(f, "unsupported: {what}"),
            RenderError::Canonical(inner) => write!(f, "canonical: {inner}"),
            RenderError::Json(msg) => write!(f, "json: {msg}"),
            RenderError::UnexpectedField { record, field } => {
                write!(f, "unexpected field `{field}` on record `{record}`")
            }
            RenderError::DuplicateFlag { flag } => write!(f, "duplicate flag `{flag}`"),
            RenderError::UnionTagMismatch { tag, reason } => {
                write!(
                    f,
                    "union branch `{tag}` rejected by discriminator: {reason}"
                )
            }
            RenderError::UnionAmbiguous { matched } => write!(
                f,
                "value matches multiple union branches: {}",
                matched.join(", ")
            ),
            RenderError::UnionNoMatch => write!(f, "no union branch matched"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<ParseError> for RenderError {
    fn from(value: ParseError) -> Self {
        RenderError::Canonical(value)
    }
}
