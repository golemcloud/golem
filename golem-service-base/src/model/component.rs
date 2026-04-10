// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use applying::Apply;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentId;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Component {
    pub id: ComponentId,
    pub revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub hash: diff::Hash,
    pub application_id: ApplicationId,
    pub account_id: AccountId,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Hash of the wasm before any transformations
    pub wasm_hash: diff::Hash,
    pub object_store_key: String,
}

impl From<Component> for golem_common::model::component::ComponentDto {
    fn from(value: Component) -> Self {
        Self {
            id: value.id,
            revision: value.revision,
            environment_id: value.environment_id,
            application_id: value.application_id,
            account_id: value.account_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: value.created_at,
            wasm_hash: value.wasm_hash,
            hash: value.hash,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        let id = value
            .component_id
            .ok_or("Missing component id")?
            .try_into()
            .map_err(|e| format!("Invalid component id: {}", e))?;

        let revision = ComponentRevision::try_from(value.revision)?;

        let environment_id = value
            .environment_id
            .ok_or("Missing environment id")?
            .try_into()
            .map_err(|e| format!("Invalid environment id: {}", e))?;

        let application_id = value
            .application_id
            .ok_or("Missing application id")?
            .try_into()
            .map_err(|e| format!("Invalid application id: {}", e))?;

        let account_id = value
            .account_id
            .ok_or("Missing account id")?
            .try_into()
            .map_err(|e| format!("Invalid account id: {}", e))?;

        let component_name = ComponentName(value.component_name);
        let component_size = value.component_size;
        let metadata = value
            .metadata
            .ok_or("Missing metadata")?
            .try_into()
            .map_err(|e| format!("Invalid metadata: {}", e))?;

        let created_at = value
            .created_at
            .ok_or("missing created_at")?
            .apply(SystemTime::try_from)
            .map_err(|_| "Failed to convert timestamp".to_string())?
            .into();

        let hash = value.hash.ok_or("Missing hash field")?.try_into()?;

        let wasm_hash = value
            .wasm_hash
            .ok_or("Missing wasm hash field")?
            .try_into()?;

        Ok(Self {
            id,
            revision,
            environment_id,
            application_id,
            account_id,
            component_name,
            component_size,
            metadata,
            created_at,
            wasm_hash,
            hash,
            object_store_key: value.object_store_key,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            component_id: Some(value.id.into()),
            revision: value.revision.into(),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            account_id: Some(value.account_id.into()),
            application_id: Some(value.application_id.into()),
            environment_id: Some(value.environment_id.into()),
            created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                value.created_at,
            ))),
            wasm_hash: Some(value.wasm_hash.into()),
            hash: Some(value.hash.into()),
            object_store_key: value.object_store_key,
        }
    }
}
