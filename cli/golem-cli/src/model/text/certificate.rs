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
use golem_client::model::Certificate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn certificate_fields(certificate: &Certificate) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Certificate ID", &certificate.id, format_main_id)
        .fmt_field("Domain name", &certificate.domain_name, format_main_id)
        .fmt_field("Project ID", &certificate.project_id, format_id)
        .fmt_field_option("Created at", &certificate.created_at, |d| d.to_string());

    fields.build()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificateNewView(pub Certificate);

impl MessageWithFields for CertificateNewView {
    fn message(&self) -> String {
        format!(
            "Created new certificate {}",
            format_message_highlight(&self.0.domain_name)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        certificate_fields(&self.0)
    }
}

#[derive(Table)]
struct CertificateTableView {
    #[table(title = "Domain")]
    pub domain_name: String,
    #[table(title = "Certificate ID")]
    pub id: Uuid,
    #[table(title = "Project")]
    pub project_id: Uuid,
}

impl From<&Certificate> for CertificateTableView {
    fn from(value: &Certificate) -> Self {
        CertificateTableView {
            domain_name: value.domain_name.to_string(),
            id: value.id,
            project_id: value.project_id,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificateListView(pub Vec<Certificate>);

impl TextView for CertificateListView {
    fn log(&self) {
        log_table::<_, CertificateTableView>(&self.0);
    }
}
