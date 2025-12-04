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
use golem_common::model::domain_registration::DomainRegistration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Table)]
struct DomainRegistrationTableView {
    #[table(title = "Domain")]
    pub domain: String,
    #[table(title = "ID")]
    pub id: Uuid,
    #[table(title = "Environment ID")]
    pub environment_id: Uuid,
}

impl From<&DomainRegistration> for DomainRegistrationTableView {
    fn from(value: &DomainRegistration) -> Self {
        DomainRegistrationTableView {
            domain: value.domain.0.clone(),
            id: value.id.0,
            environment_id: value.environment_id.0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpApiDomainListView(pub Vec<DomainRegistration>);

impl TextView for HttpApiDomainListView {
    fn log(&self) {
        log_table::<_, DomainRegistrationTableView>(&self.0);
    }
}
