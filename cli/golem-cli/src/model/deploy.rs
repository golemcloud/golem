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

use crate::command::shared_args::{DeployArgs, ForceBuildArg};
use crate::model::worker::WorkerName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_templates::model::GuestLanguage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TryUpdateAllWorkersResult {
    pub triggered: Vec<WorkerUpdateAttempt>,
    pub failed: Vec<WorkerUpdateAttempt>,
}

impl TryUpdateAllWorkersResult {
    pub fn extend(&mut self, other: TryUpdateAllWorkersResult) {
        self.triggered.extend(other.triggered);
        self.failed.extend(other.failed);
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerUpdateAttempt {
    pub component_name: ComponentName,
    pub target_revision: ComponentRevision,
    pub worker_name: WorkerName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeployConfig {
    pub plan: bool,
    pub stage: bool,
    pub approve_staging_steps: bool,
    pub force_build: Option<ForceBuildArg>,
    pub deploy_args: DeployArgs,
    pub repl_bridge_sdk_target: Option<GuestLanguage>,
}
