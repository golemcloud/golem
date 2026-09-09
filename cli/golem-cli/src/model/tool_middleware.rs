// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::log::logln;
use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::{Column, TextOutput, log_table, new_table_full_condensed};
use golem_common::model::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantId, EnvironmentToolMiddlewareGrantLifecycle,
    EnvironmentToolMiddlewareGrantWithDetails,
};
use golem_common::model::tool_middleware::RegisteredToolMiddleware;
use golem_common::model::tool_middleware_release::{
    ToolMiddlewareRelease, ToolMiddlewareReleaseId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployedToolMiddlewareView {
    pub middleware: RegisteredToolMiddleware,
}
impl StructuredOutput for DeployedToolMiddlewareView {
    const KIND: &'static str = "tool.middleware.get";
}
impl TextOutput for DeployedToolMiddlewareView {
    fn log(&self) {
        log_deployed(std::slice::from_ref(&self.middleware));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployedToolMiddlewareListView {
    pub middlewares: Vec<RegisteredToolMiddleware>,
}
impl StructuredOutput for DeployedToolMiddlewareListView {
    const KIND: &'static str = "tool.middleware.list";
}
impl TextOutput for DeployedToolMiddlewareListView {
    fn log(&self) {
        log_deployed(&self.middlewares);
    }
}

fn log_deployed(values: &[RegisteredToolMiddleware]) {
    let mut table = new_table_full_condensed(vec![
        Column::new("Middleware"),
        Column::new("Release ID"),
        Column::new("Owner"),
        Column::new("Deployment Revision"),
        Column::new("Metadata Version"),
    ]);
    for value in values {
        table.add_row(vec![
            value.definition.name.clone(),
            value
                .release_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            value.owner_account_email.to_string(),
            value.deployment_revision.to_string(),
            value.metadata_version.clone(),
        ]);
    }
    log_table(table);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMiddlewareReleaseView {
    pub release: ToolMiddlewareRelease,
}
impl StructuredOutput for ToolMiddlewareReleaseView {
    const KIND: &'static str = "tool.middleware.release";
}
impl TextOutput for ToolMiddlewareReleaseView {
    fn log(&self) {
        log_releases(std::slice::from_ref(&self.release));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMiddlewareReleaseListView {
    pub releases: Vec<ToolMiddlewareRelease>,
}
impl StructuredOutput for ToolMiddlewareReleaseListView {
    const KIND: &'static str = "tool.middleware.release.list";
}
impl TextOutput for ToolMiddlewareReleaseListView {
    fn log(&self) {
        log_releases(&self.releases);
    }
}

fn log_releases(values: &[ToolMiddlewareRelease]) {
    let mut table = new_table_full_condensed(vec![
        Column::new("Release ID"),
        Column::new("Middleware"),
        Column::new("Version"),
        Column::new("Lifecycle"),
        Column::new("Immutable"),
    ]);
    for value in values {
        table.add_row(vec![
            value.id.to_string(),
            value.name.to_string(),
            value.version.clone(),
            value.lifecycle.to_string(),
            value.immutable.to_string(),
        ]);
    }
    log_table(table);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolMiddlewareGrantView {
    pub grant_id: EnvironmentToolMiddlewareGrantId,
    pub release_id: ToolMiddlewareReleaseId,
    pub middleware_name: String,
    pub middleware_version: String,
    pub owner: String,
    pub protected: bool,
    pub automatic: bool,
    pub lifecycle: EnvironmentToolMiddlewareGrantLifecycle,
}
impl From<EnvironmentToolMiddlewareGrantWithDetails> for EnvironmentToolMiddlewareGrantView {
    fn from(value: EnvironmentToolMiddlewareGrantWithDetails) -> Self {
        Self {
            grant_id: value.grant.id,
            release_id: value.release.id,
            middleware_name: value.release.name.into_inner(),
            middleware_version: value.release.version,
            owner: value.release_owner.email.into_inner(),
            protected: value.grant.protected,
            automatic: value.grant.automatic,
            lifecycle: value.grant.lifecycle,
        }
    }
}
fn log_grants(values: &[EnvironmentToolMiddlewareGrantView]) {
    let mut table = new_table_full_condensed(vec![
        Column::new("Grant ID"),
        Column::new("Release ID"),
        Column::new("Middleware"),
        Column::new("Version"),
        Column::new("Owner"),
        Column::new("Protected"),
        Column::new("Automatic"),
        Column::new("Lifecycle"),
    ]);
    for value in values {
        table.add_row(vec![
            value.grant_id.to_string(),
            value.release_id.to_string(),
            value.middleware_name.clone(),
            value.middleware_version.clone(),
            value.owner.clone(),
            value.protected.to_string(),
            value.automatic.to_string(),
            value.lifecycle.to_string(),
        ]);
    }
    log_table(table);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolMiddlewareGrantCreateView {
    pub grant: EnvironmentToolMiddlewareGrantView,
}
impl StructuredOutput for EnvironmentToolMiddlewareGrantCreateView {
    const KIND: &'static str = "tool.middleware.grant.create";
}
impl TextOutput for EnvironmentToolMiddlewareGrantCreateView {
    fn log(&self) {
        log_grants(std::slice::from_ref(&self.grant));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolMiddlewareGrantGetView {
    pub grant: EnvironmentToolMiddlewareGrantView,
}
impl StructuredOutput for EnvironmentToolMiddlewareGrantGetView {
    const KIND: &'static str = "tool.middleware.grant.get";
}
impl TextOutput for EnvironmentToolMiddlewareGrantGetView {
    fn log(&self) {
        log_grants(std::slice::from_ref(&self.grant));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolMiddlewareGrantRestoreView {
    pub grant: EnvironmentToolMiddlewareGrantView,
}
impl StructuredOutput for EnvironmentToolMiddlewareGrantRestoreView {
    const KIND: &'static str = "tool.middleware.grant.restore";
}
impl TextOutput for EnvironmentToolMiddlewareGrantRestoreView {
    fn log(&self) {
        log_grants(std::slice::from_ref(&self.grant));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolMiddlewareGrantListView {
    pub grants: Vec<EnvironmentToolMiddlewareGrantView>,
}
impl StructuredOutput for EnvironmentToolMiddlewareGrantListView {
    const KIND: &'static str = "tool.middleware.grant.list";
}
impl TextOutput for EnvironmentToolMiddlewareGrantListView {
    fn log(&self) {
        log_grants(&self.grants);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolMiddlewareGrantDeleteView {
    pub grant_id: EnvironmentToolMiddlewareGrantId,
}
impl StructuredOutput for EnvironmentToolMiddlewareGrantDeleteView {
    const KIND: &'static str = "tool.middleware.grant.delete";
}
impl TextOutput for EnvironmentToolMiddlewareGrantDeleteView {
    fn log(&self) {
        logln(format!(
            "Deleted environment tool middleware grant {}",
            self.grant_id
        ));
    }
}
