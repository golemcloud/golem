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
use crate::model::ComponentName;
use cli_table::{format::Justify, Table};
use golem_client::model::{HttpApiDefinitionResponseData, RouteResponseData};
use serde::{Deserialize, Serialize};

#[derive(Table)]
struct RouteTableView {
    #[table(title = "Method")]
    pub method: String,
    #[table(title = "Path")]
    pub path: String,
    #[table(title = "Component Name")]
    pub component_name: ComponentName,
    #[table(title = "Component Version")]
    pub component_version: String,
    #[table(title = "Binding Type")]
    pub binding_type: String,
}

impl From<&RouteResponseData> for RouteTableView {
    fn from(value: &RouteResponseData) -> Self {
        Self {
            method: value.method.to_string(),
            path: value.path.to_string(),
            component_name: match &value.binding.component {
                Some(component) => component.name.clone(),
                None => "<NA>".to_string(),
            }
            .into(),
            component_version: match &value.binding.component {
                Some(component) => component.version.to_string(),
                None => "<NA>".to_string(),
            },
            binding_type: match &value.binding.binding_type {
                Some(binding_type) => binding_type.to_string(),
                None => "<NA>".to_string(),
            },
        }
    }
}

fn api_definition_fields(def: &HttpApiDefinitionResponseData) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("ID", &def.id, format_main_id)
        .fmt_field("Version", &def.version, format_main_id)
        .fmt_field_option("Created at", &def.created_at, |d| d.to_string())
        .fmt_field_optional("Draft", &def.draft, def.draft, |d| d.to_string())
        .fmt_field_optional(
            "Routes",
            def.routes.as_slice(),
            !def.routes.is_empty(),
            format_table::<_, RouteTableView>,
        );

    fields.build()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionGetView(pub HttpApiDefinitionResponseData);

impl MessageWithFields for ApiDefinitionGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for API definition {} version {}",
            format_message_highlight(&self.0.id),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        api_definition_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionNewView(pub HttpApiDefinitionResponseData);

impl MessageWithFields for ApiDefinitionNewView {
    fn message(&self) -> String {
        format!(
            "Created API definition {} with version {}",
            format_message_highlight(&self.0.id),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        api_definition_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionUpdateView(pub HttpApiDefinitionResponseData);

impl MessageWithFields for ApiDefinitionUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated API definition {} with version {}",
            format_message_highlight(&self.0.id),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        api_definition_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionImportView(pub HttpApiDefinitionResponseData);

impl MessageWithFields for ApiDefinitionImportView {
    fn message(&self) -> String {
        format!(
            "Imported API definition {} with version {}",
            format_message_highlight(&self.0.id),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        api_definition_fields(&self.0)
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

impl From<&HttpApiDefinitionResponseData> for HttpApiDefinitionTableView {
    fn from(value: &HttpApiDefinitionResponseData) -> Self {
        Self {
            id: value.id.to_string(),
            version: value.version.to_string(),
            route_count: value.routes.len(),
        }
    }
}

impl TextView for Vec<HttpApiDefinitionResponseData> {
    fn log(&self) {
        log_table::<_, HttpApiDefinitionTableView>(self);
    }
}
