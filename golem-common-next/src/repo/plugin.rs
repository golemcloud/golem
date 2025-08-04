// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::plugin::ComponentPluginScope;
use crate::model::plugin::{PluginScope, ProjectPluginScope};
use crate::model::{ComponentId, Empty, ProjectId};
use crate::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::Display;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PluginOwnerRow {
    pub account_id: String,
}

impl Display for PluginOwnerRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.account_id)
    }
}

impl<DB: Database> RowMeta<DB> for PluginOwnerRow
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

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PluginScopeRow {
    scope_component_id: Option<Uuid>,
    scope_project_id: Option<Uuid>,
}

impl From<PluginScope> for PluginScopeRow {
    fn from(value: PluginScope) -> Self {
        match value {
            PluginScope::Global(_) => Self {
                scope_component_id: None,
                scope_project_id: None,
            },
            PluginScope::Component(component) => Self {
                scope_component_id: Some(component.component_id.0),
                scope_project_id: None,
            },
            PluginScope::Project(project) => Self {
                scope_component_id: None,
                scope_project_id: Some(project.project_id.0),
            },
        }
    }
}

impl TryFrom<PluginScopeRow> for PluginScope {
    type Error = String;

    fn try_from(value: PluginScopeRow) -> Result<Self, Self::Error> {
        match (value.scope_component_id, value.scope_project_id) {
            (Some(component_id), None) => Ok(PluginScope::Component(ComponentPluginScope {
                component_id: ComponentId(component_id),
            })),
            (None, Some(project_id)) => Ok(PluginScope::Project(ProjectPluginScope {
                project_id: ProjectId(project_id),
            })),
            (None, None) => Ok(PluginScope::Global(Empty {})),
            _ => Err("Invalid scope (has both component and project id set)".to_string()),
        }
    }
}

impl<DB: Database> RowMeta<DB> for PluginScopeRow
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
