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

use crate::model::diff;

pub use crate::base_model::deployment::*;

impl From<CurrentDeployment> for Deployment {
    fn from(value: CurrentDeployment) -> Self {
        Self {
            environment_id: value.environment_id,
            revision: value.revision,
            version: value.version,
            deployment_hash: value.deployment_hash,
        }
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
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|had| (had.domain.0.clone(), had.hash.into()))
                .collect(),
        }
    }
}
