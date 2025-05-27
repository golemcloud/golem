use crate::model::ProjectPluginInstallationTarget;
use golem_common::model::ProjectId;
use golem_common::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder};
use std::fmt::Display;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ProjectPluginInstallationTargetRow {
    pub project_id: Uuid,
}

impl Display for ProjectPluginInstallationTargetRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.project_id)
    }
}

impl From<ProjectPluginInstallationTarget> for ProjectPluginInstallationTargetRow {
    fn from(target: ProjectPluginInstallationTarget) -> Self {
        ProjectPluginInstallationTargetRow {
            project_id: target.project_id.0,
        }
    }
}

impl TryFrom<ProjectPluginInstallationTargetRow> for ProjectPluginInstallationTarget {
    type Error = String;

    fn try_from(value: ProjectPluginInstallationTargetRow) -> Result<Self, Self::Error> {
        Ok(ProjectPluginInstallationTarget {
            project_id: ProjectId(value.project_id),
        })
    }
}

impl<DB: Database> RowMeta<DB> for ProjectPluginInstallationTargetRow
where
    Uuid: for<'q> Encode<'q, DB> + sqlx::Type<DB>,
{
    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("project_id");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        builder.push("project_id = ");
        builder.push_bind(self.project_id);
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.project_id);
    }
}
