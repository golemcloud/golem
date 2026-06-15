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

use crate::model::cli_output::CliOutput;
use crate::model::text::fmt::*;

use golem_common::model::domain_registration::{Domain, DomainRegistration, DomainRegistrationId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainRegistrationNewView(pub DomainRegistration);

impl MessageWithFields for DomainRegistrationNewView {
    fn message(&self) -> String {
        format!(
            "Created new API domain registration {}",
            format_message_highlight(&self.0.domain.0)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Domain name", &self.0.domain.0, format_main_id)
            .fmt_field("ID", &self.0.id, format_main_id)
            .fmt_field("Environment ID", &self.0.environment_id, format_id);

        fields.build()
    }
}

impl CliOutput for DomainRegistrationNewView {
    const KIND: &'static str = "api.domain.register.result";
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainRegistrationDeleteResult {
    pub deleted: bool,
    pub domain: Domain,
    pub id: DomainRegistrationId,
}

impl TextView for DomainRegistrationDeleteResult {
    fn log(&self) {}
}

impl CliOutput for DomainRegistrationDeleteResult {
    const KIND: &'static str = "api.domain.delete.result";
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDomainListView {
    pub domains: Vec<DomainRegistration>,
}

impl TextView for HttpApiDomainListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Domain"),
            Column::new("ID").fixed(),
            Column::new("Environment ID").fixed(),
        ]);
        for reg in &self.domains {
            table.add_row(vec![
                reg.domain.0.clone(),
                reg.id.0.to_string(),
                reg.environment_id.0.to_string(),
            ]);
        }
        log_table(table);
    }
}

impl CliOutput for HttpApiDomainListView {
    const KIND: &'static str = "api.domain.list.result";
}
