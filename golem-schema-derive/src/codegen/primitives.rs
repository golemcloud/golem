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

//! Primitive support for the derive macros.
//!
//! Concrete `IntoSchema` / `FromSchema` impls for all primitives, `Option`,
//! `Vec`, tuples, `Result`, `HashMap` / `BTreeMap`, `chrono::DateTime<Utc>`,
//! `chrono::Duration`, and `std::time::Duration` live in
//! `golem_common::schema::conversion`. The derive macros only need to delegate
//! through `<T as IntoSchema>::register(builder)` / `<T as FromSchema>::…`,
//! which automatically picks up those impls for any type whose `T: IntoSchema`
//! bound is satisfied.
//!
//! This module is kept as a documentation anchor; no code lives here today.
