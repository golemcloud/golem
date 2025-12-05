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
use super::domain_registration::Domain;
use super::environment::EnvironmentId;
use super::http_api_definition::{
    HttpApiDefinitionId, HttpApiDefinitionName, HttpApiDefinitionRevision,
};
use super::http_api_deployment::{HttpApiDeploymentId, HttpApiDeploymentRevision};
use crate::model::diff;
use crate::{declare_revision, declare_structs};

declare_revision!(DeploymentRevision);

// Revision of the environment current_revision field, counting rollbacks as well as normal deployments
declare_revision!(CurrentDeploymentRevision);

declare_structs! {
    pub struct Deployment {
        pub environment_id: EnvironmentId,
        pub revision: DeploymentRevision,
        pub version: String,
        pub deployment_hash: Hash,
    }

    pub struct CurrentDeployment {
        pub environment_id: EnvironmentId,
        pub revision: DeploymentRevision,
        pub version: String,
        pub deployment_hash: Hash,

        pub current_revision: CurrentDeploymentRevision,
    }

    pub struct DeploymentCreation {
        pub current_revision: Option<CurrentDeploymentRevision>,
        pub expected_deployment_hash: Hash,
        pub version: String
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
        pub http_api_definitions: Vec<DeploymentPlanHttpApiDefintionEntry>,
        pub http_api_deployments: Vec<DeploymentPlanHttpApiDeploymentEntry>
    }

    /// Summary of all entities tracked by the deployment
    pub struct DeploymentSummary {
        pub deployment_revision: DeploymentRevision,
        pub deployment_hash: Hash,
        pub components: Vec<DeploymentPlanComponentEntry>,
        pub http_api_definitions: Vec<DeploymentPlanHttpApiDefintionEntry>,
        pub http_api_deployments: Vec<DeploymentPlanHttpApiDeploymentEntry>
    }

    pub struct DeploymentPlanComponentEntry {
        pub id: ComponentId,
        pub revision: ComponentRevision,
        pub name: ComponentName,
        pub hash: Hash,
    }

    pub struct DeploymentPlanHttpApiDefintionEntry {
        pub id: HttpApiDefinitionId,
        pub revision: HttpApiDefinitionRevision,
        pub name: HttpApiDefinitionName,
        pub hash: Hash,
    }

    pub struct DeploymentPlanHttpApiDeploymentEntry {
        pub id: HttpApiDeploymentId,
        pub revision: HttpApiDeploymentRevision,
        pub domain: Domain,
        pub hash: Hash,
    }
}

impl DeploymentPlan {
    pub fn to_diffable(&self) -> diff::Deployment {
        diff::Deployment {
            components: self
                .components
                .iter()
                .map(|component| (component.name.0.clone(), component.hash.into()))
                .collect(),
            http_api_definitions: self
                .http_api_definitions
                .iter()
                .map(|had| (had.name.0.clone(), had.hash.into()))
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|had| (had.domain.0.clone(), had.hash.into()))
                .collect(),
        }
    }
}

impl DeploymentSummary {
    pub fn to_diffable(&self) -> diff::Deployment {
        diff::Deployment {
            components: self
                .components
                .iter()
                .map(|component| (component.name.0.clone(), component.hash.into()))
                .collect(),
            http_api_definitions: self
                .http_api_definitions
                .iter()
                .map(|had| (had.name.0.clone(), had.hash.into()))
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|had| (had.domain.0.clone(), had.hash.into()))
                .collect(),
        }
    }
}
