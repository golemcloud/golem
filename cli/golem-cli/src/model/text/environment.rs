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

use crate::log::LogColorize;
use crate::model::environment::{
    EnvironmentReference, ResolvedEnvironmentIdentity, ResolvedEnvironmentIdentitySource,
};
use crate::model::text::fmt::{log_table, TextView};
use cli_table::format::Justify;
use cli_table::Table;
use golem_client::model::EnvironmentWithDetails;

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

#[derive(Table)]
struct EnvironmentSummaryTableView {
    #[table(title = "Application Name")]
    pub application_name: String,
    #[table(title = "Environment Name")]
    pub environment_name: String,
    #[table(title = "Deployment Revision", justify = "Justify::Right")]
    pub deployment_revision: String,
    #[table(title = "Deployment Version")]
    pub deployment_version: String,
}

impl From<&EnvironmentWithDetails> for EnvironmentSummaryTableView {
    fn from(value: &EnvironmentWithDetails) -> Self {
        Self {
            application_name: value.application.name.0.clone(),
            environment_name: value.environment.name.0.clone(),
            deployment_revision: value
                .environment
                .current_deployment
                .as_ref()
                .map(|d| d.deployment_revision.get().to_string())
                .unwrap_or_default(),
            deployment_version: value
                .environment
                .current_deployment
                .as_ref()
                .map(|d| d.deployment_version.0.clone())
                .unwrap_or_default(),
        }
    }
}

impl TextView for Vec<EnvironmentWithDetails> {
    fn log(&self) {
        log_table::<_, EnvironmentSummaryTableView>(self.as_slice())
    }
}
