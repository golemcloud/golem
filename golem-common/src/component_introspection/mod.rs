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

//! Component introspection helpers.
//!
//! This module isolates the WASM-file parsing surface (reading exported
//! interface/function names, linear memories, producers, and root package
//! metadata). It does not depend on the legacy value/type machinery: it only
//! reads the structural information needed to build [`crate::base_model::component_metadata::KnownExports`]
//! and the rest of `ComponentMetadata`.

pub mod error;
pub mod metadata;
pub mod wit_parser;

pub use error::{
    AnalysisFailure, AnalysisResult, AnalysisWarning, InterfaceCouldNotBeAnalyzedWarning,
};
pub use wit_parser::{ExportedInstance, TopLevelExport};
