use golem_common::model::{ComponentFilePath, ComponentFilePermissions};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentType {
    Ephemeral,
    Durable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialComponentFile {
    pub source_path: String,
    pub target_path: ComponentFilePath,
    pub permissions: Option<ComponentFilePermissions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GolemComponentPropertiesExt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_type: Option<ComponentType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<InitialComponentFile>,
}
