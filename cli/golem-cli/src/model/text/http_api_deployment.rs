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

use crate::model::text::fmt::{
    Column, FieldsBuilder, MessageWithFields, TextView, format_main_id, format_message_highlight,
    log_table, new_table,
};
use golem_common::model::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentAgentSecurity};
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpApiDeploymentGetView(pub HttpApiDeployment);

impl MessageWithFields for HttpApiDeploymentGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for HTTP API deployment, domain: {}",
            format_message_highlight(&self.0.domain),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        http_api_deployment_fields(&self.0)
    }
}

fn http_api_deployment_fields(dep: &HttpApiDeployment) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Domain", &dep.domain, format_main_id)
        .fmt_field("ID", &dep.id, format_main_id)
        .fmt_field("Environment ID", &dep.environment_id, format_main_id)
        .fmt_field("Revision", &dep.revision, format_main_id)
        .fmt_field("Created at", &dep.created_at, |d| d.to_string())
        .fmt_field("Webhooks url", &dep.webhooks_url, |d| d.clone())
        .fmt_field("Agents", &dep.agents, |agents| {
            let mut result = String::new();
            for (agent_name, agent_options) in agents {
                result.push_str(&format!("- Agent name: {}", agent_name));
                match &agent_options.security {
                    None => {}
                    Some(HttpApiDeploymentAgentSecurity::SecurityScheme(inner)) => {
                        result.push_str(&format!("  Security scheme: {}", inner.security_scheme));
                    }
                    Some(HttpApiDeploymentAgentSecurity::TestSessionHeader(inner)) => {
                        result.push_str(&format!("  Test session header: {}", inner.header_name));
                    }
                }
            }
            result
        });

    fields.build()
}

impl TextView for Vec<HttpApiDeployment> {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Domain"),
            Column::new("ID").fixed(),
            Column::new("Environment ID").fixed(),
            Column::new("Revision").fixed(),
        ]);
        for dep in self {
            table.add_row(vec![
                dep.domain.to_string(),
                dep.id.to_string(),
                dep.environment_id.to_string(),
                dep.revision.to_string(),
            ]);
        }
        log_table(table);
    }
}
