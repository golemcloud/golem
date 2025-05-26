use crate::model::CloudComponentOwner;
use golem_common::model::{AccountId, ProjectId};
use golem_common::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::Display;
use uuid::Uuid;

use super::CloudPluginOwnerRow;

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
        builder.push_bind(self.to_string());
    }
}

impl From<CloudComponentOwnerRow> for CloudPluginOwnerRow {
    fn from(value: CloudComponentOwnerRow) -> Self {
        CloudPluginOwnerRow {
            account_id: value.account_id,
        }
    }
}
