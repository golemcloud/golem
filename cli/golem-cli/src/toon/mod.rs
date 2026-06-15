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

//! Minimal TOON (Token-Oriented Object Notation) implementation used for the
//! CLI `toon` output format.
//!
//! Implements the encoder side of the TOON specification v3.3
//! (<https://github.com/toon-format/spec>) with the default options:
//! comma delimiter, two-space indentation and no key folding.
//!
//! A strict decoder is implemented for tests only; it is used as the oracle
//! for round-trip property tests covering the full `serde_json::Value` model.

mod encode;

#[cfg(test)]
mod decode;
#[cfg(test)]
mod tests;

pub use encode::{ToonEncodeError, encode, encode_value};
