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

use crate::model::ProjectName;
use crate::model::{AccountId, ProjectId};
use golem_client::model::{Project, ProjectType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub project_id: ProjectId,
    pub name: ProjectName,
    pub owner_account_id: AccountId,
    pub description: String,
    pub default_environment_id: String,
    pub project_type: ProjectType,
}

impl From<Project> for ProjectView {
    fn from(value: Project) -> Self {
        Self {
            project_id: value.project_id.into(),
            name: value.project_data.name.into(),
            owner_account_id: value.project_data.owner_account_id.into(),
            description: value.project_data.description.to_string(),
            default_environment_id: value.project_data.default_environment_id.to_string(),
            project_type: value.project_data.project_type.clone(),
        }
    }
}
