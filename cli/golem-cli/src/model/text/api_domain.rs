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

use crate::model::text::fmt::*;
use cli_table::Table;
use golem_client::model::ApiDomain;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiDomainNewView(pub ApiDomain);

impl MessageWithFields for ApiDomainNewView {
    fn message(&self) -> String {
        format!(
            "Created new API domain {}",
            format_message_highlight(&self.0.domain_name)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Domain name", &self.0.domain_name, format_main_id)
            .fmt_field("Project ID", &self.0.project_id, format_id)
            .fmt_field_option("Created at", &self.0.created_at, |d| d.to_string())
            .fmt_field_optional(
                "Name servers",
                &self.0.name_servers,
                !self.0.name_servers.is_empty(),
                |ns| ns.join("\n"),
            );

        fields.build()
    }
}

#[derive(Table)]
struct ApiDomainTableView {
    #[table(title = "Domain")]
    pub domain_name: String,
    #[table(title = "Project")]
    pub project_id: Uuid,
    #[table(title = "Servers")]
    pub name_servers: String,
}

impl From<&ApiDomain> for ApiDomainTableView {
    fn from(value: &ApiDomain) -> Self {
        ApiDomainTableView {
            domain_name: value.domain_name.to_string(),
            project_id: value.project_id,
            name_servers: value.name_servers.join("\n"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiDomainListView(pub Vec<ApiDomain>);

impl TextView for ApiDomainListView {
    fn log(&self) {
        log_table::<_, ApiDomainTableView>(&self.0);
    }
}
