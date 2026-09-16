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

use crate::error::HintError;
use crate::log::log_warn;
use crate::log::{LogColorize, logln};
use crate::model::app_raw::Environment;
use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::*;
use anyhow::bail;
use golem_common::base_model::environment_tool_grant::{
    EnvironmentToolGrantId, EnvironmentToolGrantLifecycle, EnvironmentToolGrantWithDetails,
};
use golem_common::base_model::tool_release::ToolReleaseId;
use golem_common::model::account::AccountId;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{
    EnvironmentCurrentDeploymentView, EnvironmentId, EnvironmentName, EnvironmentWithDetails,
};
use indoc::formatdoc;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::str::FromStr;

#[derive(Clone, PartialEq, Debug)]
pub enum EnvironmentReference {
    Environment {
        environment_name: EnvironmentName,
    },
    ApplicationEnvironment {
        application_name: ApplicationName,
        environment_name: EnvironmentName,
    },
    AccountApplicationEnvironment {
        account_email: String,
        application_name: ApplicationName,
        environment_name: EnvironmentName,
    },
}

impl EnvironmentReference {
    pub fn is_manifest_scoped(&self) -> bool {
        match &self {
            Self::Environment { .. } => true,
            Self::ApplicationEnvironment { .. } => false,
            Self::AccountApplicationEnvironment { .. } => false,
        }
    }
}

impl TryFrom<&str> for EnvironmentReference {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl TryFrom<String> for EnvironmentReference {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl FromStr for EnvironmentReference {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let segments = s.split("/").collect::<Vec<_>>();
        match segments.len() {
            1 => Ok(Self::Environment {
                environment_name: segments[0].parse()?,
            }),
            2 => Ok(Self::ApplicationEnvironment {
                application_name: segments[0].parse()?,
                environment_name: segments[1].parse()?,
            }),
            3 => Ok(Self::AccountApplicationEnvironment {
                account_email: segments[0].into(),
                application_name: segments[1].parse()?,
                environment_name: segments[2].parse()?,
            }),
            _ => Err(formatdoc! {"
                Unknown format for environment: {}. Expected one of:
                - <ENVIRONMENT_NAME>
                - <APPLICATION_NAME>/<ENVIRONMENT_NAME>
                - <ACCOUNT_EMAIL>/<APPLICATION_NAME>/<ENVIRONMENT_NAME>
                ",
                s.log_color_highlight()
            }),
        }
    }
}

impl Display for EnvironmentReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Environment { environment_name } => write!(f, "{}", environment_name.0),
            Self::ApplicationEnvironment {
                application_name,
                environment_name,
            } => write!(f, "{}/{}", application_name.0, environment_name.0),
            Self::AccountApplicationEnvironment {
                account_email,
                environment_name,
                application_name,
            } => write!(
                f,
                "{}/{}/{}",
                account_email, application_name.0, environment_name.0
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ResolvedEnvironmentIdentitySource {
    Reference(EnvironmentReference),
    DefaultFromManifest,
}

impl ResolvedEnvironmentIdentitySource {
    pub fn is_manifest_scoped(&self) -> bool {
        match self {
            ResolvedEnvironmentIdentitySource::Reference(env_ref) => env_ref.is_manifest_scoped(),
            ResolvedEnvironmentIdentitySource::DefaultFromManifest => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedEnvironmentIdentity {
    pub source: ResolvedEnvironmentIdentitySource,

    pub account_id: AccountId,
    pub application_id: ApplicationId,
    pub application_name: ApplicationName,
    pub environment_id: EnvironmentId,
    pub environment_name: EnvironmentName,

    pub server_environment: golem_client::model::Environment,
}

impl ResolvedEnvironmentIdentity {
    pub fn from_app_and_env(
        environment_reference: Option<&EnvironmentReference>,
        application: golem_client::model::Application,
        environment: golem_client::model::Environment,
    ) -> Self {
        Self {
            source: match environment_reference {
                Some(env_ref) => ResolvedEnvironmentIdentitySource::Reference(env_ref.clone()),
                None => ResolvedEnvironmentIdentitySource::DefaultFromManifest,
            },
            account_id: application.account_id,
            application_id: application.id,
            application_name: application.name,
            environment_id: environment.id,
            environment_name: environment.name.clone(),
            server_environment: environment,
        }
    }

    pub fn from_summary(
        environment_reference: Option<&EnvironmentReference>,
        summary: EnvironmentWithDetails,
    ) -> Self {
        Self {
            source: match environment_reference {
                Some(env_ref) => ResolvedEnvironmentIdentitySource::Reference(env_ref.clone()),
                None => ResolvedEnvironmentIdentitySource::DefaultFromManifest,
            },
            account_id: summary.account.id,
            application_id: summary.application.id,
            application_name: summary.application.name.clone(),
            environment_id: summary.environment.id,
            environment_name: summary.environment.name.clone(),
            server_environment: golem_common::model::environment::Environment {
                id: summary.environment.id,
                revision: summary.environment.revision,
                application_id: summary.application.id,
                application_name: summary.application.name.clone(),
                name: summary.environment.name,
                diff_model_version: summary.environment.diff_model_version,
                compatibility_check: summary.environment.compatibility_check,
                version_check: summary.environment.version_check,
                security_overrides: summary.environment.security_overrides,
                owner_account_id: summary.account.id,
                owner_account_email: summary.account.email,
                current_deployment: summary.environment.current_deployment,
            },
        }
    }

    pub fn is_manifest_scoped(&self) -> bool {
        self.source.is_manifest_scoped()
    }

    pub fn text_format(&self) -> String {
        format_resolved_environment_identity(self)
    }

    pub fn current_deployment(&self) -> Option<&EnvironmentCurrentDeploymentView> {
        self.server_environment.current_deployment.as_ref()
    }

    pub fn current_deployment_or_err(&self) -> anyhow::Result<&EnvironmentCurrentDeploymentView> {
        match self.server_environment.current_deployment.as_ref() {
            Some(deployment) => Ok(deployment),
            None => {
                bail!(HintError::EnvironmentHasNoDeployment);
            }
        }
    }

    pub async fn with_current_deployment_revision_or_default_warn<F, Fut, R>(
        &self,
        f: F,
    ) -> anyhow::Result<R>
    where
        F: FnOnce(DeploymentRevision) -> Fut,
        Fut: Future<Output = anyhow::Result<R>>,
        R: Default,
    {
        match self.current_deployment() {
            Some(deployment) => f(deployment.deployment_revision).await,
            None => {
                logln("");
                log_warn(format!(
                    "The current environment {} has no deployment.",
                    self.text_format()
                ));
                logln(
                    "Use the 'golem deploy' CLI command, or the '.deploy' REPL command, or select a different environment.",
                );
                logln("");
                Ok(R::default())
            }
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub enum EnvironmentResolveMode {
    ManifestOnly, // The environment must be one of the ones defined in the manifest
    Any, // The environment can be one of the manifest ones, or any other "more" qualified reference
}

impl EnvironmentResolveMode {
    pub fn allowed(&self, environment: &EnvironmentReference) -> bool {
        match self {
            EnvironmentResolveMode::ManifestOnly => match environment {
                EnvironmentReference::Environment { .. } => true,
                EnvironmentReference::ApplicationEnvironment { .. } => false,
                EnvironmentReference::AccountApplicationEnvironment { .. } => false,
            },
            EnvironmentResolveMode::Any => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelectedManifestEnvironment {
    pub application_name: ApplicationName,
    pub environment_name: EnvironmentName,
    pub environment: Environment,
}

pub fn format_resolved_environment_identity(environment: &ResolvedEnvironmentIdentity) -> String {
    match &environment.source {
        ResolvedEnvironmentIdentitySource::Reference(environment_reference) => {
            match environment_reference {
                EnvironmentReference::Environment { environment_name } => {
                    format!(
                        "{}/{}",
                        environment.application_name.0.log_color_highlight(),
                        environment_name.0.log_color_highlight()
                    )
                }
                EnvironmentReference::ApplicationEnvironment {
                    application_name,
                    environment_name,
                } => {
                    format!(
                        "{}/{}",
                        application_name.0.log_color_highlight(),
                        environment_name.0.log_color_highlight()
                    )
                }
                EnvironmentReference::AccountApplicationEnvironment {
                    account_email,
                    application_name,
                    environment_name,
                } => {
                    format!(
                        "{}/{}/{}",
                        account_email.log_color_highlight(),
                        application_name.0.log_color_highlight(),
                        environment_name.0.log_color_highlight()
                    )
                }
            }
        }
        ResolvedEnvironmentIdentitySource::DefaultFromManifest => format!(
            "{}/{}",
            environment.application_name.0.log_color_highlight(),
            environment.environment_name.0.log_color_highlight(),
        ),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentListView {
    pub environments: Vec<EnvironmentWithDetails>,
}

impl StructuredOutput for EnvironmentListView {
    const KIND: &'static str = "environment.list";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSyncDeploymentOptionsResult {
    pub updated: bool,
}

impl StructuredOutput for EnvironmentSyncDeploymentOptionsResult {
    const KIND: &'static str = "environment.sync-deployment-options";
}

impl NoTextOutput for EnvironmentSyncDeploymentOptionsResult {}
impl TextOutput for EnvironmentSyncDeploymentOptionsResult {}

impl TextOutput for EnvironmentListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Application Name"),
            Column::new("Environment Name"),
            Column::new("Deployment Revision").fixed_right(),
            Column::new("Deployment Version").fixed(),
        ]);
        for env in &self.environments {
            table.add_row(vec![
                env.application.name.0.clone(),
                env.environment.name.0.clone(),
                env.environment
                    .current_deployment
                    .as_ref()
                    .map(|d| d.deployment_revision.get().to_string())
                    .unwrap_or_default(),
                env.environment
                    .current_deployment
                    .as_ref()
                    .map(|d| d.deployment_version.0.clone())
                    .unwrap_or_default(),
            ]);
        }
        log_table(table);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantView {
    pub grant_id: EnvironmentToolGrantId,
    pub release_id: ToolReleaseId,
    pub tool_name: String,
    pub tool_version: String,
    pub owner: String,
    pub protected: bool,
    pub automatic: bool,
    pub lifecycle: EnvironmentToolGrantLifecycle,
}

impl From<EnvironmentToolGrantWithDetails> for EnvironmentToolGrantView {
    fn from(value: EnvironmentToolGrantWithDetails) -> Self {
        Self {
            grant_id: value.grant.id,
            release_id: value.release.id,
            tool_name: value.release.name.into_inner(),
            tool_version: value.release.version,
            owner: value.release_owner.email.into_inner(),
            protected: value.grant.protected,
            automatic: value.grant.automatic,
            lifecycle: value.grant.lifecycle,
        }
    }
}

impl EnvironmentToolGrantView {
    fn row(&self) -> Vec<String> {
        vec![
            self.grant_id.to_string(),
            self.release_id.to_string(),
            self.tool_name.clone(),
            self.tool_version.clone(),
            self.owner.clone(),
            self.protected.to_string(),
            self.automatic.to_string(),
            self.lifecycle.to_string(),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantCreateView {
    pub grant: EnvironmentToolGrantView,
}

impl StructuredOutput for EnvironmentToolGrantCreateView {
    const KIND: &'static str = "tool.grant.create";
}

impl TextOutput for EnvironmentToolGrantCreateView {
    fn log(&self) {
        log_text_view(&self.grant);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantGetView {
    pub grant: EnvironmentToolGrantView,
}

impl StructuredOutput for EnvironmentToolGrantGetView {
    const KIND: &'static str = "tool.grant.get";
}

impl TextOutput for EnvironmentToolGrantGetView {
    fn log(&self) {
        log_text_view(&self.grant);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantRestoreView {
    pub grant: EnvironmentToolGrantView,
}

impl StructuredOutput for EnvironmentToolGrantRestoreView {
    const KIND: &'static str = "tool.grant.restore";
}

impl TextOutput for EnvironmentToolGrantRestoreView {
    fn log(&self) {
        log_text_view(&self.grant);
    }
}

impl TextOutput for EnvironmentToolGrantView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Grant ID"),
            Column::new("Release ID"),
            Column::new("Tool"),
            Column::new("Version"),
            Column::new("Owner"),
            Column::new("Protected"),
            Column::new("Automatic"),
            Column::new("Lifecycle"),
        ]);
        table.add_row(self.row());
        log_table(table);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantListView {
    pub grants: Vec<EnvironmentToolGrantView>,
}
impl StructuredOutput for EnvironmentToolGrantListView {
    const KIND: &'static str = "tool.grant.list";
}
impl TextOutput for EnvironmentToolGrantListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Grant ID"),
            Column::new("Release ID"),
            Column::new("Tool"),
            Column::new("Version"),
            Column::new("Owner"),
            Column::new("Protected"),
            Column::new("Automatic"),
            Column::new("Lifecycle"),
        ]);
        for grant in &self.grants {
            table.add_row(grant.row());
        }
        log_table(table);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantDeleteView {
    pub grant_id: EnvironmentToolGrantId,
}
impl StructuredOutput for EnvironmentToolGrantDeleteView {
    const KIND: &'static str = "tool.grant.delete";
}
impl TextOutput for EnvironmentToolGrantDeleteView {
    fn log(&self) {
        logln(format!("Deleted environment tool grant {}", self.grant_id));
    }
}
