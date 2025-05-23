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

use crate::log::logln;
use crate::model::api::ApiDeployment;
use crate::model::text::fmt::*;
use cli_table::Table;
use golem_client::model::ApiDefinitionInfo;

pub fn format_site(api_deployment: &ApiDeployment) -> String {
    match &api_deployment.site.subdomain {
        Some(subdomain) => format!("{}.{}", subdomain, api_deployment.site.host),
        None => api_deployment.site.host.to_string(),
    }
}

impl TextView for ApiDeployment {
    fn log(&self) {
        for api_defs in &self.api_definitions {
            logln(format!(
                "API {}/{} deployed at {}",
                format_message_highlight(&api_defs.id),
                format_message_highlight(&api_defs.version),
                format_message_highlight(&format_site(self)),
            ));
        }
    }
}

#[derive(Table)]
struct ApiDeploymentTableView {
    #[table(title = "Site")]
    pub site: String,
    #[table(title = "Definition ID")]
    pub id: String,
    #[table(title = "Version")]
    pub version: String,
}

impl From<&(&ApiDeployment, &ApiDefinitionInfo)> for ApiDeploymentTableView {
    fn from(value: &(&ApiDeployment, &ApiDefinitionInfo)) -> Self {
        let (deployment, def) = value;
        ApiDeploymentTableView {
            site: format_site(deployment),
            id: def.id.to_string(),
            version: def.version.to_string(),
        }
    }
}

impl TextView for Vec<ApiDeployment> {
    fn log(&self) {
        log_table::<_, ApiDeploymentTableView>(
            self.iter()
                .flat_map(|deployment| {
                    deployment
                        .api_definitions
                        .iter()
                        .map(move |def| (deployment, def))
                })
                .collect::<Vec<_>>()
                .as_slice(),
        );
    }
}
