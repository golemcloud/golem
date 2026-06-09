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

//! Round-trip conversion between the recursive in-memory schema model in
//! [`super`] and the flat, index-based wire types from the
//! `golem:core@2.0.0` WIT package.
//!
//! The wire types themselves are the `wit-bindgen` / `wasmtime`-generated
//! bindings re-exported by `golem-wasm` as [`wire`]; this module does not
//! declare its own mirror of those types.

mod decode;
mod encode;

/// Generated `golem:core@2.0.0` types used as the wire shape.
pub use golem_wasm::golem_core_2_0_x::types as wire;

pub use decode::{
    DecodeError, GraphDecoder, decode_graph, decode_metadata, decode_typed, decode_value,
};
pub use encode::{
    EncodeError, GraphEncoder, encode_graph, encode_metadata, encode_typed, encode_value,
};
