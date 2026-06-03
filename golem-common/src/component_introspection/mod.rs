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

//! Component introspection helpers extracted from `golem-wasm`.
//!
//! This module isolates the WASM-file parsing surface (reading exports, linear
//! memories, producers, root package metadata, and resolving WIT type
//! references) from the rest of the value/type machinery in `golem-wasm`. It
//! still relies on `golem-wasm`'s analysis result types (`AnalysedExport`,
//! `AnalysedFunction`, `AnalysedType`, …) so that callers can migrate to it
//! without changing their data model.
//!
//! The module may later be promoted into its own crate; for now it lives under
//! `golem-common` to avoid a new workspace member during Wave 1 of the
//! value/type refactor.

pub mod metadata;
pub mod wit_parser;
