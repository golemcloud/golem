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

use crate::error::HintError;
use crate::log::{logln, LogColorize};
use crate::model::app_raw::Environment;
use crate::model::text::environment::format_resolved_environment_identity;
use crate::log::log_warn;
use anyhow::bail;
use golem_common::model::account::AccountId;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{
    EnvironmentCurrentDeploymentView, EnvironmentId, EnvironmentName, EnvironmentWithDetails,
};
use indoc::formatdoc;
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
            application_name: summary.application.name,
            environment_id: summary.environment.id,
            environment_name: summary.environment.name.clone(),
            server_environment: golem_common::model::environment::Environment {
                id: summary.environment.id,
                revision: summary.environment.revision,
                application_id: summary.application.id,
                name: summary.environment.name,
                compatibility_check: summary.environment.compatibility_check,
                version_check: summary.environment.version_check,
                security_overrides: summary.environment.security_overrides,
                owner_account_id: summary.account.id,
                roles_from_active_shares: summary.environment.roles_from_active_shares,
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
                logln("Use 'golem deploy' for deploying, or select a different environment.");
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
