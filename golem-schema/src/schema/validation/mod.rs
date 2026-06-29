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

//! Validation utilities for the in-memory schema model.
//!
//! Three core validators, each with its own error type:
//!
//! - [`well_formedness::validate_graph`] — structural integrity of a
//!   [`super::SchemaGraph`] (no duplicate type ids, no dangling refs,
//!   non-empty discriminated sums, well-formed scalar constraints, etc.).
//! - [`value::validate_value`] — a [`super::SchemaValue`] structurally
//!   conforms to a given [`super::SchemaType`] inside a graph.
//! - [`subtyping::is_assignable`] — width / depth / scalar-narrowing
//!   subtyping with cycle detection over [`super::SchemaType::Ref`].
//!
//! These validators are pure in-memory checks. They do not depend on the
//! generated WIT bindings under [`super::wit`].

pub mod subtyping;
pub mod value;
pub mod well_formedness;

#[cfg(test)]
mod tests;

pub use subtyping::{is_assignable, is_equivalent_cross_graph};
pub use value::{ValueError, ValuePath, ValuePathSegment, validate_value};
pub use well_formedness::{SchemaError, validate_graph};
