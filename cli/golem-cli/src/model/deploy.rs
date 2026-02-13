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

use crate::command::shared_args::{ForceBuildArg, PostDeployArgs};
use crate::model::worker::WorkerName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_templates::model::GuestLanguage;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    pub post_deploy_args: PostDeployArgs,
    pub repl_bridge_sdk_target: Option<GuestLanguage>,
    pub skip_build: bool,
}

pub enum DeploySummary {
    PlanOk,
    PlanUpToDate,
    StagingOk, // Only for internal testing purposes
    DeployOk(PostDeployResult),
    DeployUpToDate(PostDeployResult),
    RollbackOk(PostDeployResult),
    RollbackUpToDate(PostDeployResult),
}

#[derive(Error, Debug)]
pub enum DeployError {
    #[error("Cancelled")]
    Cancelled,
    #[error("Build error: {0}")]
    BuildError(anyhow::Error),
    #[error("Prepare error: {0}")]
    PrepareError(anyhow::Error),
    #[error("Plan error: {0}")]
    PlanError(anyhow::Error),
    #[error("Environment check error: {0}")]
    EnvironmentCheckError(anyhow::Error),
    #[error("Staging error: {0}")]
    StagingError(anyhow::Error),
    #[error("Deploy error: {0}")]
    DeployError(anyhow::Error),
    #[error("Rollback error: {0}")]
    RollbackError(anyhow::Error),
}

pub type DeployResult = Result<DeploySummary, DeployError>;

pub enum PostDeploySummary {
    NoRequestedChanges,
    NoDeployment,
    AgentUpdateOk,
    AgentRedeployOk,
    AgentDeleteOk,
}

#[derive(Error, Debug)]
pub enum PostDeployError {
    #[error("Prepare error: {0}")]
    PrepareError(anyhow::Error),
    #[error("Agent update error: {0}")]
    AgentUpdateError(anyhow::Error),
    #[error("Agent redeploy error: {0}")]
    AgentRedeployError(anyhow::Error),
    #[error("Agent delete error: {0}")]
    AgentDeleteError(anyhow::Error),
}

pub type PostDeployResult = Result<PostDeploySummary, PostDeployError>;
