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
use golem_client::model::SecuritySchemeDto;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSecuritySchemeCreateView(pub SecuritySchemeDto);

impl MessageWithFields for HttpSecuritySchemeCreateView {
    fn message(&self) -> String {
        format!(
            "Created new HTTP API Security scheme {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        security_scheme_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSecuritySchemeGetView(pub SecuritySchemeDto);

impl MessageWithFields for HttpSecuritySchemeGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for HTTP API Security scheme {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        security_scheme_view_fields(&self.0)
    }
}

fn security_scheme_view_fields(view: &SecuritySchemeDto) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Name", &view.name.0, format_main_id)
        .fmt_field("ID", &view.id, format_id)
        .fmt_field("Revision", &view.revision.get(), format_id)
        .field("Provider", &view.provider_type)
        .field("Client ID", &view.client_id)
        .field("Redirect URL", &view.redirect_url)
        .field("Scopes", &view.scopes.join("\n"));

    fields.build()
}

#[derive(Table)]
struct HttpApiSecuritySchemeTableView {
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Revision", Justify = "Right")]
    pub revision: u64,
    #[table(title = "Provider")]
    pub provider: String,
    #[table(title = "Client ID")]
    pub client_id: String,
    #[table(title = "Redirect URL")]
    pub redirect_url: String,
    #[table(title = "Scopes")]
    pub scopes: String,
}

impl From<&SecuritySchemeDto> for HttpApiSecuritySchemeTableView {
    fn from(value: &SecuritySchemeDto) -> Self {
        Self {
            name: value.name.0.clone(),
            id: value.id.to_string(),
            revision: value.revision.get(),
            provider: value.provider_type.to_string(),
            client_id: value.client_id.clone(),
            redirect_url: value.redirect_url.clone(),
            scopes: value.scopes.join("\n"),
        }
    }
}

impl TextView for Vec<SecuritySchemeDto> {
    fn log(&self) {
        log_table::<_, HttpApiSecuritySchemeTableView>(self.as_slice())
    }
}
