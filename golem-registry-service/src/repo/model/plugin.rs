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

use crate::repo::model::audit::DeletableRevisionAuditFields;
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct PluginRecord {
    pub plugin_id: Uuid,
    pub account_id: Uuid,

    pub name: String,
    pub version: String,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,

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

    // for LibraryPlugin plugin type
    pub blob_storage_key: Option<String>,
}
