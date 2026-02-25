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

use crate::base_model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::base_model::diff::Hash;
use crate::base_model::domain_registration::Domain;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::http_api_deployment::{HttpApiDeploymentId, HttpApiDeploymentRevision};
use crate::{declare_revision, declare_structs, declare_transparent_newtypes};
use derive_more::Display;

declare_revision!(DeploymentRevision);

// Revision of the environment current_revision field, counting rollbacks as well as normal deployments
declare_revision!(CurrentDeploymentRevision);

declare_transparent_newtypes! {
    #[derive(Display, PartialOrd, Eq, Ord)]
    pub struct DeploymentVersion(pub String);
}

impl From<String> for DeploymentVersion {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DeploymentVersion {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

declare_structs! {
    pub struct Deployment {
        pub environment_id: EnvironmentId,
        pub revision: DeploymentRevision,
        pub version: DeploymentVersion,
        pub deployment_hash: Hash,
    }

    pub struct CurrentDeployment {
        pub environment_id: EnvironmentId,
        pub revision: DeploymentRevision,
        pub version: DeploymentVersion,
        pub deployment_hash: Hash,

        pub current_revision: CurrentDeploymentRevision,
    }

    pub struct DeploymentCreation {
        pub current_revision: Option<CurrentDeploymentRevision>,
        pub expected_deployment_hash: Hash,
        pub version: DeploymentVersion
    }

    pub struct DeploymentRollback {
        pub current_revision: CurrentDeploymentRevision,
        pub deployment_revision: DeploymentRevision,
    }

    /// Planned deployment including the current revision
    pub struct DeploymentPlan {
        pub current_revision: Option<CurrentDeploymentRevision>,
        pub deployment_hash: Hash,
        pub components: Vec<DeploymentPlanComponentEntry>,
        pub http_api_deployments: Vec<DeploymentPlanHttpApiDeploymentEntry> 
        // TODO; should have MCP here
    }

    /// Summary of all entities tracked by the deployment
    pub struct DeploymentSummary {
        pub deployment_revision: DeploymentRevision,
        pub deployment_hash: Hash,
        pub components: Vec<DeploymentPlanComponentEntry>,
        pub http_api_deployments: Vec<DeploymentPlanHttpApiDeploymentEntry>
        // TODO; should have MCP here
    }

    pub struct DeploymentPlanComponentEntry {
        pub id: ComponentId,
        pub revision: ComponentRevision,
        pub name: ComponentName,
        pub hash: Hash,
    }

    pub struct DeploymentPlanHttpApiDeploymentEntry {
        pub id: HttpApiDeploymentId,
        pub revision: HttpApiDeploymentRevision,
        pub domain: Domain,
        pub hash: Hash,
        // TODO; should have MCP here
    }
}
