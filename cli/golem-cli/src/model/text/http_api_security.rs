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
use crate::model::masking::Masked;
use crate::model::text::fmt::*;

use golem_client::model::SecuritySchemeDto;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSecuritySchemeCreateView(pub SecuritySchemeDto);

impl Masked for HttpSecuritySchemeCreateView {}

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

impl CliOutput for HttpSecuritySchemeCreateView {
    const KIND: &'static str = "api.security-scheme.create";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSecuritySchemeGetView(pub SecuritySchemeDto);

impl Masked for HttpSecuritySchemeGetView {}

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

impl CliOutput for HttpSecuritySchemeGetView {
    const KIND: &'static str = "api.security-scheme.get";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSecuritySchemeUpdateView(pub SecuritySchemeDto);

impl Masked for HttpSecuritySchemeUpdateView {}

impl MessageWithFields for HttpSecuritySchemeUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated HTTP API Security scheme {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        security_scheme_view_fields(&self.0)
    }
}

impl CliOutput for HttpSecuritySchemeUpdateView {
    const KIND: &'static str = "api.security-scheme.update";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSecuritySchemeDeleteView(pub SecuritySchemeDto);

impl Masked for HttpSecuritySchemeDeleteView {}

impl MessageWithFields for HttpSecuritySchemeDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted HTTP API Security scheme {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        security_scheme_view_fields(&self.0)
    }
}

impl CliOutput for HttpSecuritySchemeDeleteView {
    const KIND: &'static str = "api.security-scheme.delete";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpSecuritySchemeListView {
    pub security_schemes: Vec<SecuritySchemeDto>,
}

impl CliOutput for HttpSecuritySchemeListView {
    const KIND: &'static str = "api.security-scheme.list";
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

impl TextView for HttpSecuritySchemeListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Name").fixed(),
            Column::new("ID").fixed(),
            Column::new("Revision").fixed_right(),
            Column::new("Provider").fixed(),
            Column::new("Client ID").fixed(),
            Column::new("Redirect URL"),
            Column::new("Scopes"),
        ]);
        for scheme in &self.security_schemes {
            table.add_row(vec![
                scheme.name.0.clone(),
                scheme.id.to_string(),
                scheme.revision.get().to_string(),
                scheme.provider_type.to_string(),
                scheme.client_id.clone(),
                scheme.redirect_url.clone(),
                scheme.scopes.join("\n"),
            ]);
        }
        log_table(table);
    }
}
