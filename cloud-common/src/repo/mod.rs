use crate::model::CloudPluginOwner;
use golem_common::model::AccountId;
use golem_common::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::Display;

pub mod component;
pub mod plugin;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct CloudPluginOwnerRow {
    pub account_id: String,
}

impl Display for CloudPluginOwnerRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.account_id)
    }
}

impl From<CloudPluginOwner> for CloudPluginOwnerRow {
    fn from(owner: CloudPluginOwner) -> Self {
        CloudPluginOwnerRow {
            account_id: owner.account_id.value,
        }
    }
}

impl TryFrom<CloudPluginOwnerRow> for CloudPluginOwner {
    type Error = String;

    fn try_from(value: CloudPluginOwnerRow) -> Result<Self, Self::Error> {
        Ok(CloudPluginOwner {
            account_id: AccountId {
                value: value.account_id,
            },
        })
    }
}

impl<DB: Database> RowMeta<DB> for CloudPluginOwnerRow
where
    String: for<'q> Encode<'q, DB> + Type<DB>,
{
    // NOTE: We could store account_id and project_id in separate columns, but this abstraction was
    // introduced when the `components` table already used the generic "namespace" column so
    // we need to keep that to be able to join the tables.

    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("account_id");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        builder.push("account_id = ");
        builder.push_bind(&self.account_id);
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(&self.account_id);
    }
}
