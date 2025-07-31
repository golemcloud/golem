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

pub mod accounts;
pub mod login;
pub mod token;
pub mod plugins;

use poem_openapi::{Object, Enum};
use golem_common::model::{AccountId, ProjectId};
use poem_openapi::types::{ParseFromJSON, ToJSON};
use golem_common::newtype_uuid;

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct Project {
    pub project_id: ProjectId,
    pub project_data: ProjectData,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct ProjectData {
    pub name: String,
    pub owner_account_id: AccountId,
    pub description: String,
    pub default_environment_id: String,
    pub project_type: ProjectType,
}

#[derive(Debug, Clone, Enum)]
pub enum ProjectType {
    Default,
    NonDefault,
}

#[derive(Debug, Clone, Object)]
pub struct GetProjectsResponse {
    pub values: Vec<Project>
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
    pub owner_account_id: AccountId,
    pub description: String,
}

#[derive(Debug, Clone, Object)]
pub struct Page<T: poem_openapi::types::Type + ParseFromJSON + ToJSON> {
    pub values: Vec<T>,
}

newtype_uuid!(ApplicationId);
newtype_uuid!(EnvironmentId);

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct Application {
    pub id: ApplicationId,
    pub account_id: AccountId,
    pub name: String,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct ApplicationData {
    pub name: String,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct Environment {
    pub id: EnvironmentId,
    pub application_id: ApplicationId,
    pub name: String,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct EnvironmentData {
    pub name: String,
}
