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

use serde::{Deserialize, Serialize};
use super::{ApiDefinitionId, EnvironmentId, ProjectId};
use chrono::DateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[repr(transparent)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
pub struct ApiSite(pub String);


#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiDeployment {
    pub api_definitions: Vec<ApiDefinitionId>,
    pub environment_id: EnvironmentId,
    pub site: ApiSite,
    pub created_at: DateTime<chrono::Utc>,
}
