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

use crate::log::LogColorize;
use crate::model::environment::{
    EnvironmentReference, ResolvedEnvironmentIdentity, ResolvedEnvironmentIdentitySource,
};
use crate::model::text::fmt::{Column, TextView, log_table, new_table};
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

impl TextView for Vec<EnvironmentWithDetails> {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Application Name"),
            Column::new("Environment Name"),
            Column::new("Deployment Revision").fixed_right(),
            Column::new("Deployment Version").fixed(),
        ]);
        for env in self {
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
