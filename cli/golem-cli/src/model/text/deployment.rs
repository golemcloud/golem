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

use crate::model::text::fmt::{
    format_id, format_main_id, log_table, FieldsBuilder, MessageWithFields, TextView,
};
use cli_table::format::Justify;
use cli_table::Table;
use golem_client::model::Deployment;
use golem_common::model::application::ApplicationName;
use golem_common::model::deployment::CurrentDeployment;
use golem_common::model::environment::EnvironmentName;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentNewView {
    pub application_name: ApplicationName,
    pub environment_name: EnvironmentName,
    pub deployment: CurrentDeployment,
}

impl MessageWithFields for DeploymentNewView {
    fn message(&self) -> String {
        "Created new deployment".to_owned()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Application", &self.application_name.0, format_id)
            .fmt_field("Environment", &self.environment_name.0, format_id)
            .fmt_field(
                "Environment ID",
                &self.deployment.environment_id,
                format_main_id,
            )
            .fmt_field(
                "Deployment Revision",
                &self.deployment.revision,
                format_main_id,
            )
            .fmt_field("Hash", &self.deployment.deployment_hash, format_id)
            .field("Deploy Revision", &self.deployment.current_revision);

        fields.build()
    }
}

#[derive(Table)]
struct DeploymentTableView {
    #[table(title = "Deployment Revision", justify = "Justify::Right")]
    pub deployment_revision: u64,
    #[table(title = "Deployment Version", justify = "Justify::Right")]
    pub deployment_version: String,
    #[table(title = "Hash")]
    pub hash: String,
}

impl From<&Deployment> for DeploymentTableView {
    fn from(value: &Deployment) -> Self {
        Self {
            deployment_revision: value.revision.get(),
            deployment_version: value.version.0.clone(),
            hash: value.deployment_hash.to_string(),
        }
    }
}

impl TextView for Vec<Deployment> {
    fn log(&self) {
        log_table::<_, DeploymentTableView>(self.as_slice())
    }
}
