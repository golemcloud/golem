use golem_common::model::ProjectId;
use golem_service_base::model::ComponentName;
use poem_openapi::Object;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentQuery {
    pub project_id: Option<ProjectId>,
    pub component_name: ComponentName,
}
