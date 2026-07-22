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

//! Compatibility layer around [`golem_schema::schema::tool`].
//!
//! The native tool model, its validation, and the canonical input model live
//! in `golem-schema` so the Rust SDK and the Golem services share one
//! implementation. This module re-exports them and keeps only the WIT wire
//! conversions ([`wit`]), which depend on the host-side generated bindings.

pub use golem_schema::schema::tool::*;

#[cfg(feature = "full")]
pub mod wit;

#[cfg(test)]
mod tests;
