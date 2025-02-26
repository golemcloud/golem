use crate::model::{CloudPluginScope, ProjectPluginScope};
use golem_common::model::plugin::ComponentPluginScope;
use golem_common::model::{ComponentId, Empty, ProjectId};
use golem_common::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::Display;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct CloudPluginScopeRow {
    scope_component_id: Option<Uuid>,
    scope_project_id: Option<Uuid>,
}

impl From<CloudPluginScope> for CloudPluginScopeRow {
    fn from(value: CloudPluginScope) -> Self {
        match value {
            CloudPluginScope::Global(_) => Self {
                scope_component_id: None,
                scope_project_id: None,
            },
            CloudPluginScope::Component(component) => Self {
                scope_component_id: Some(component.component_id.0),
                scope_project_id: None,
            },
            CloudPluginScope::Project(project) => Self {
                scope_component_id: None,
                scope_project_id: Some(project.project_id.0),
            },
        }
    }
}

impl TryFrom<CloudPluginScopeRow> for CloudPluginScope {
    type Error = String;

    fn try_from(value: CloudPluginScopeRow) -> Result<Self, Self::Error> {
        match (value.scope_component_id, value.scope_project_id) {
            (Some(component_id), None) => Ok(CloudPluginScope::Component(ComponentPluginScope {
                component_id: ComponentId(component_id),
            })),
            (None, Some(project_id)) => Ok(CloudPluginScope::Project(ProjectPluginScope {
                project_id: ProjectId(project_id),
            })),
            (None, None) => Ok(CloudPluginScope::Global(Empty {})),
            _ => Err("Invalid scope (has both component and project id set)".to_string()),
        }
    }
}

impl<DB: Database> RowMeta<DB> for CloudPluginScopeRow
where
    Uuid: for<'q> Encode<'q, DB> + Type<DB>,
    Option<Uuid>: for<'q> Encode<'q, DB> + Type<DB>,
{
    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("scope_component_id");
        builder.push("scope_project_id");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        if let Some(component_id) = &self.scope_component_id {
            builder.push("scope_component_id = ");
            builder.push_bind(component_id);
        } else if let Some(project_id) = &self.scope_project_id {
            builder.push("scope_project_id = ");
            builder.push_bind(project_id);
        } else {
            builder.push("scope_component_id IS NULL AND scope_project_id IS NULL");
        }
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.scope_component_id);
        builder.push_bind(self.scope_project_id);
    }
}
