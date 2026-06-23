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
//! The wire types themselves are generated in this crate. The active generated
//! module is selected by the mutually-exclusive `guest` and `host` features and
//! re-exported through [`wire`].

mod decode;
mod encode;

#[cfg(any(
    all(feature = "guest", not(feature = "host")),
    all(feature = "host", not(feature = "guest"))
))]
mod model;

#[cfg(all(feature = "guest", not(feature = "host")))]
mod guest;

#[cfg(all(feature = "host", not(feature = "guest")))]
mod host;

/// Generated `golem:core@2.0.0` types used as the wire shape.
#[cfg(all(feature = "guest", not(feature = "host")))]
pub use guest::generated::golem::core::types as wire;

/// Generated `golem:core@2.0.0` types used as the wire shape.
#[cfg(all(feature = "host", not(feature = "guest")))]
pub use host::generated::golem::core::types as wire;

pub use decode::{
    DecodeError, GraphDecoder, decode_graph, decode_metadata, decode_typed, decode_value,
};
pub use encode::{
    EncodeError, GraphEncoder, encode_graph, encode_metadata, encode_typed, encode_value,
};
#[cfg(any(
    all(feature = "guest", not(feature = "host")),
    all(feature = "host", not(feature = "guest"))
))]
pub use model::*;

#[cfg(any(
    all(feature = "guest", not(feature = "host")),
    all(feature = "host", not(feature = "guest"))
))]
impl From<wire::Uuid> for uuid::Uuid {
    fn from(value: wire::Uuid) -> Self {
        uuid::Uuid::from_u64_pair(value.high_bits, value.low_bits)
    }
}

#[cfg(any(
    all(feature = "guest", not(feature = "host")),
    all(feature = "host", not(feature = "guest"))
))]
impl From<uuid::Uuid> for wire::Uuid {
    fn from(uuid: uuid::Uuid) -> Self {
        let (high_bits, low_bits) = uuid.as_u64_pair();
        Self {
            high_bits,
            low_bits,
        }
    }
}
