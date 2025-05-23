// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedExport;
use std::fmt::Debug;
use std::path::Path;
use uuid::Uuid;

/// Dependency manager for the Rib REPL environment.
#[async_trait]
pub trait RibDependencyManager {
    /// Deploys all components within the current context if they are not already registered
    /// with the Golem engine. Returns a list of successfully registered components.
    ///
    /// Note: It is the responsibility of the REPL client to resolve component paths.
    /// In the future, Rib may support multiple components.
    async fn get_dependencies(&self) -> anyhow::Result<ReplDependencies>;

    /// Deploys a specific component if the REPL was started with a reference to it.
    ///
    /// Currently, dynamic component loading is not supported.
    ///
    /// # Arguments
    ///
    /// * `source_path` - The file path to the component.
    /// * `component_name` - The name of the component.
    ///
    /// # Returns
    ///
    /// Returns a `ComponentDependency` on success or an error message as a `String` if the operation fails.
    async fn add_component(
        &self,
        source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<RibComponentMetadata>;
}

pub struct ReplDependencies {
    pub component_dependencies: Vec<RibComponentMetadata>,
}

#[derive(Clone, Debug)]
pub struct RibComponentMetadata {
    pub component_id: Uuid,
    pub component_name: String,
    pub metadata: Vec<AnalysedExport>,
}
