use golem_common::model::component::VersionedComponentId;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::{AccountId, ComponentType, InitialComponentFile, ProjectId};
use golem_component_service_base::api::dto::PluginInstallation;
use golem_service_base::model::ComponentName;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub account_id: AccountId,
    pub project_id: ProjectId,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
    pub env: HashMap<String, String>,
}
