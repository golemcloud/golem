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
use cli_table::{format::Justify, Table};
use golem_common::model::http_api_definition::{GatewayBinding, HttpApiDefinition, HttpApiRoute};
use serde::{Deserialize, Serialize};

#[derive(Table)]
struct HttpApiRouteTableView {
    #[table(title = "Method")]
    pub method: String,
    #[table(title = "Path")]
    pub path: String,
    #[table(title = "Component Name")]
    pub component_name: String,
    #[table(title = "Binding Type")]
    pub binding_type: String,
    #[table(title = "Security Type")]
    pub security: String,
}

impl From<&HttpApiRoute> for HttpApiRouteTableView {
    fn from(value: &HttpApiRoute) -> Self {
        Self {
            method: value.method.to_string(),
            path: value.path.to_string(),
            component_name: value
                .binding
                .component_name()
                .map(|cn| cn.to_string())
                .unwrap_or_else(|| "<NA>".to_string()),
            binding_type: match &value.binding {
                GatewayBinding::Worker(_) => "Agent",
                GatewayBinding::FileServer(_) => "FileServer",
                GatewayBinding::HttpHandler(_) => "HTTP Hanlder",
                GatewayBinding::CorsPreflight(_) => "CORS",
                GatewayBinding::SwaggerUi(_) => "Swagger UI",
            }
            .to_string(),
            security: value
                .security
                .as_ref()
                .map(|s| s.0.to_string())
                .unwrap_or_else(|| "<NA>".to_string()),
        }
    }
}

fn http_api_definition_fields(def: &HttpApiDefinition) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("ID", &def.id, format_main_id)
        .fmt_field("Version", &def.version, format_main_id)
        .fmt_field("Created at", &def.created_at, |d| d.to_string())
        .fmt_field("Updated at", &def.updated_at, |d| d.to_string())
        .fmt_field_optional(
            "Routes",
            def.routes.as_slice(),
            !def.routes.is_empty(),
            format_table::<_, HttpApiRouteTableView>,
        );

    fields.build()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpApiDefinitionGetView(pub HttpApiDefinition);

impl MessageWithFields for HttpApiDefinitionGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for API definition {} version {}",
            format_message_highlight(&self.0.id),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        http_api_definition_fields(&self.0)
    }
}

#[derive(Table)]
struct HttpApiDefinitionTableView {
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Version")]
    pub version: String,
    #[table(title = "Route count", justify = "Justify::Right")]
    pub route_count: usize,
}

impl From<&HttpApiDefinition> for HttpApiDefinitionTableView {
    fn from(value: &HttpApiDefinition) -> Self {
        Self {
            id: value.id.to_string(),
            version: value.version.to_string(),
            route_count: value.routes.len(),
        }
    }
}

impl TextView for Vec<HttpApiDefinition> {
    fn log(&self) {
        log_table::<_, HttpApiDefinitionTableView>(self);
    }
}
