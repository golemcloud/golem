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

//! Schema and value renderers.
//!
//! All public entry points operate over `(SchemaGraph, SchemaType, …)`
//! triples. The traversal skeleton lives in [`walker`]; every renderer
//! either implements [`walker::SchemaWalker`] directly or drives it
//! through [`walker::walk`].

pub mod cli_text;
pub mod docs;
pub mod error;
pub mod json_schema;
pub mod json_value;
pub mod openapi;
pub mod walker;

#[cfg(test)]
mod tests;

pub use cli_text::{type_to_cli_text, value_to_cli_text, value_to_cli_text_unredacted};
pub use docs::graph_to_markdown;
pub use error::RenderError;
pub use json_schema::to_json_schema;
pub use json_value::{from_json_value, to_json_value};
pub use openapi::to_openapi_components;
pub use walker::{SchemaWalker, WalkerError, resolve_ref, walk};
