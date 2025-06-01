use golem_common::model::ProjectId;
use golem_component_service_base::model::ComponentSearchParameters;
use golem_service_base::model::ComponentName;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentQuery {
    pub project_id: Option<ProjectId>,
    pub component_name: ComponentName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentSearch {
    pub project_id: Option<ProjectId>,
    pub components: Vec<ComponentSearchParameters>,
}
