use crate::model::{CloudComponentOwner, CloudPluginScope, ProjectPluginScope};
use cloud_common::repo::CloudPluginOwnerRow;
use golem_common::model::plugin::ComponentPluginScope;
use golem_common::model::{AccountId, ComponentId, Empty, ProjectId};
use golem_common::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::Display;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct CloudComponentOwnerRow {
    pub account_id: String,
    pub project_id: Uuid,
}

impl Display for CloudComponentOwnerRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.account_id, self.project_id)
    }
}

impl From<CloudComponentOwner> for CloudComponentOwnerRow {
    fn from(owner: CloudComponentOwner) -> Self {
        CloudComponentOwnerRow {
            account_id: owner.account_id.value,
            project_id: owner.project_id.0,
        }
    }
}

impl TryFrom<CloudComponentOwnerRow> for CloudComponentOwner {
    type Error = String;

    fn try_from(value: CloudComponentOwnerRow) -> Result<Self, Self::Error> {
        Ok(CloudComponentOwner {
            account_id: AccountId {
                value: value.account_id,
            },
            project_id: ProjectId(value.project_id),
        })
    }
}

impl<DB: Database> RowMeta<DB> for CloudComponentOwnerRow
where
    String: for<'q> Encode<'q, DB> + Type<DB>,
{
    // NOTE: We could store account_id and project_id in separate columns, but this abstraction was
    // introduced when the `components` table already used the generic "namespace" column so
    // we need to keep that to be able to join the tables.

    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("namespace");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        builder.push("namespace = ");
        let namespace_string = self.to_string();
        builder.push_bind(namespace_string);
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push(self.to_string());
    }
}

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

impl From<CloudComponentOwnerRow> for CloudPluginOwnerRow {
    fn from(value: CloudComponentOwnerRow) -> Self {
        CloudPluginOwnerRow {
            account_id: value.account_id,
        }
    }
}
