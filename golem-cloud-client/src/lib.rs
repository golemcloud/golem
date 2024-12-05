use golem_common::model::plugin::ComponentPluginScope;
use golem_common::model::{Empty, ProjectId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Display, Formatter};

include!(concat!(env!("OUT_DIR"), "/src/lib.rs"));

#[cfg(test)]
test_r::enable!();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPluginScope {
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CloudPluginScope {
    Global(Empty),
    Component(ComponentPluginScope),
    Project(ProjectPluginScope),
}

impl Display for CloudPluginScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CloudPluginScope::Global(_) => write!(f, "global"),
            CloudPluginScope::Component(scope) => write!(f, "component:{}", scope.component_id),
            CloudPluginScope::Project(scope) => write!(f, "project:{}", scope.project_id),
        }
    }
}

impl Default for CloudPluginScope {
    fn default() -> Self {
        CloudPluginScope::Global(Empty {})
    }
}
