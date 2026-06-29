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

//! Canonical text and JSON encodings for the rich scalar and capability
//! payloads of [`super::SchemaValue`].
//!
//! Every supported scalar exposes a uniform four-function API:
//!
//! ```text
//! pub fn to_text(payload: &Payload) -> String;
//! pub fn from_text(s: &str) -> Result<Payload, ParseError>;
//! pub fn to_json(payload: &Payload) -> serde_json::Value;
//! pub fn from_json(value: &serde_json::Value) -> Result<Payload, ParseError>;
//! ```
//!
//! The shared error type lives in [`error`].

pub mod binary;
pub mod datetime;
pub mod duration;
pub mod error;
pub mod path;
pub mod quantity;
pub mod quota_token;
pub mod secret;
pub mod text;
pub mod url;

pub use error::ParseError;
