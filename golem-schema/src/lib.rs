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

//! Schema representation, canonical encoding, and validation.
//!
//! `regex` enables the Rust `regex` crate's complete existing dialect for text
//! restrictions and union discriminators. `url` enables WHATWG URL validation
//! (including IDNA) and conversions for `url::Url`. `rich-validation` enables
//! both; it is included by default and by `host` and `full`.
//!
//! With default features disabled, the schema representation and fixed MIME,
//! quantity-unit, and tool-identifier grammars remain available. Validation
//! requiring a disabled feature returns an explicit unsupported-feature error;
//! it never accepts a value by skipping its regex or URL restriction. Regex
//! union rendering also fails explicitly. Enable the needed features in guests
//! that validate these rich values. URL schema structure and canonical encoding
//! do not require URL parsing; value validation does.

#[cfg(all(feature = "host", feature = "guest"))]
compile_error!("golem-schema features `host` and `guest` are mutually exclusive");

// Allows the `IntoSchema` / `FromSchema` derive macros to be used on types
// defined inside this crate when proc-macro-crate resolves this package as a
// regular crate name.
extern crate self as golem_schema;

#[cfg(test)]
test_r::enable!();

pub mod http;
pub mod model;
pub mod schema;

pub use model::*;
pub use schema::*;
