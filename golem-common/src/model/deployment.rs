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

use super::component::{ComponentId, ComponentName, ComponentRevision};
use super::diff::Hash;
use super::environment::EnvironmentId;
use crate::{declare_revision, declare_structs};

declare_revision!(DeploymentRevision);

declare_structs! {
    pub struct Deployment {
        pub environment_id: EnvironmentId,
        pub revision: DeploymentRevision,
        pub version: String,
        pub deployment_hash: Hash
    }

    pub struct DeploymentCreation {
        pub current_deployment_revision: Option<DeploymentRevision>,
        pub expected_deployment_hash: Hash,
        pub version: String
    }

    /// Summary of all entities tracked by the deployment
    pub struct DeploymentPlan {
        pub deployment_hash: Hash,
        pub components: Vec<DeploymentPlanComponentEntry>,
        // TODO: http_api_definitons, http_api_deployments
    }

    pub struct DeploymentPlanComponentEntry {
        pub id: ComponentId,
        pub revision: ComponentRevision,
        pub name: ComponentName,
        pub hash: Hash,
    }
}
