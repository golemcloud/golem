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

use super::audit::ImmutableAuditFields;
use super::hash::SqlBlake3Hash;
use anyhow::anyhow;
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use golem_common::model::plugin_registration::WasmContentHash;
use golem_common::model::plugin_registration::{
    ComponentTransformerPluginSpec, OplogProcessorPluginSpec, PluginRegistrationId,
};
use golem_service_base::model::plugin_registration::{
    AppPluginSpec, LibraryPluginSpec, PluginRegistration, PluginSpec,
};
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

const APP_PLUGIN_TYPE: i16 = 0;
const LIBRARY_PLUGIN_TYPE: i16 = 1;
const COMPONENT_TRANSFORMER_PLUGIN_TYPE: i16 = 2;
const OPLOG_PROCESSOR_PLUGIN_TYPE: i16 = 3;

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct PluginRecord {
    pub plugin_id: Uuid,
    pub account_id: Uuid,

    pub name: String,
    pub version: String,

    #[sqlx(flatten)]
    pub audit: ImmutableAuditFields,

    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub plugin_type: i16,

    // for ComponentTransformer plugin type
    pub provided_wit_package: Option<String>,
    pub json_schema: Option<Json<serde_json::Value>>,
    pub validate_url: Option<String>,
    pub transform_url: Option<String>,

    // for OplogProcessor plugin type
    pub component_id: Option<Uuid>,
    pub component_revision_id: Option<i64>,

    // for Library and App plugin type
    pub wasm_content_hash: Option<SqlBlake3Hash>,
}

impl PluginRecord {
    pub fn from_model(model: PluginRegistration, audit: ImmutableAuditFields) -> Self {
        match model.spec {
            PluginSpec::App(inner) => Self {
                plugin_id: model.id.0,
                account_id: model.account_id.0,
                name: model.name,
                version: model.version,
                audit,
                description: model.description,
                icon: model.icon,
                homepage: model.homepage,
                plugin_type: APP_PLUGIN_TYPE,
                provided_wit_package: None,
                json_schema: None,
                validate_url: None,
                transform_url: None,
                component_id: None,
                component_revision_id: None,
                wasm_content_hash: Some(inner.wasm_content_hash.0.into()),
            },
            PluginSpec::Library(inner) => Self {
                plugin_id: model.id.0,
                account_id: model.account_id.0,
                name: model.name,
                version: model.version,
                audit,
                description: model.description,
                icon: model.icon,
                homepage: model.homepage,
                plugin_type: LIBRARY_PLUGIN_TYPE,
                provided_wit_package: None,
                json_schema: None,
                validate_url: None,
                transform_url: None,
                component_id: None,
                component_revision_id: None,
                wasm_content_hash: Some(inner.wasm_content_hash.0.into()),
            },
            PluginSpec::ComponentTransformer(inner) => Self {
                plugin_id: model.id.0,
                account_id: model.account_id.0,
                name: model.name,
                version: model.version,
                audit,
                description: model.description,
                icon: model.icon,
                homepage: model.homepage,
                plugin_type: COMPONENT_TRANSFORMER_PLUGIN_TYPE,
                provided_wit_package: inner.provided_wit_package,
                json_schema: inner.json_schema.map(Json),
                validate_url: Some(inner.validate_url),
                transform_url: Some(inner.transform_url),
                component_id: None,
                component_revision_id: None,
                wasm_content_hash: None,
            },
            PluginSpec::OplogProcessor(inner) => Self {
                plugin_id: model.id.0,
                account_id: model.account_id.0,
                name: model.name,
                version: model.version,
                audit,
                description: model.description,
                icon: model.icon,
                homepage: model.homepage,
                plugin_type: OPLOG_PROCESSOR_PLUGIN_TYPE,
                provided_wit_package: None,
                json_schema: None,
                validate_url: None,
                transform_url: None,
                component_id: Some(inner.component_id.0),
                component_revision_id: Some(inner.component_revision.into()),
                wasm_content_hash: None,
            },
        }
    }
}

impl TryFrom<PluginRecord> for PluginRegistration {
    type Error = anyhow::Error;

    fn try_from(value: PluginRecord) -> Result<Self, Self::Error> {
        match value.plugin_type {
            APP_PLUGIN_TYPE => Ok(Self {
                id: PluginRegistrationId(value.plugin_id),
                account_id: AccountId(value.account_id),
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                spec: PluginSpec::App(AppPluginSpec {
                    wasm_content_hash: WasmContentHash(
                        value
                            .wasm_content_hash
                            .ok_or(anyhow!("no wasm_content_hash field"))?
                            .into(),
                    ),
                }),
            }),
            LIBRARY_PLUGIN_TYPE => Ok(Self {
                id: PluginRegistrationId(value.plugin_id),
                account_id: AccountId(value.account_id),
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                spec: PluginSpec::Library(LibraryPluginSpec {
                    wasm_content_hash: WasmContentHash(
                        value
                            .wasm_content_hash
                            .ok_or(anyhow!("no wasm_content_hash field"))?
                            .into(),
                    ),
                }),
            }),
            COMPONENT_TRANSFORMER_PLUGIN_TYPE => Ok(Self {
                id: PluginRegistrationId(value.plugin_id),
                account_id: AccountId(value.account_id),
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                spec: PluginSpec::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: value.provided_wit_package,
                    json_schema: value.json_schema.map(|v| v.0),
                    validate_url: value.validate_url.ok_or(anyhow!("no validate_url field"))?,
                    transform_url: value
                        .transform_url
                        .ok_or(anyhow!("no transform_url field"))?,
                }),
            }),
            OPLOG_PROCESSOR_PLUGIN_TYPE => Ok(Self {
                id: PluginRegistrationId(value.plugin_id),
                account_id: AccountId(value.account_id),
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                spec: PluginSpec::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: ComponentId(
                        value.component_id.ok_or(anyhow!("no component_id field"))?,
                    ),
                    component_revision: value
                        .component_revision_id
                        .ok_or(anyhow!("no component_revision field"))?
                        .into(),
                }),
            }),
            other => Err(anyhow!("Unknown plugin type {other}"))?,
        }
    }
}
