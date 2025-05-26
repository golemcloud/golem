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

use crate::model::plugin::{ComponentPluginScope, DefaultPluginOwner, DefaultPluginScope};
use crate::model::{ComponentId, Empty};
use crate::repo::component::DefaultComponentOwnerRow;
use crate::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct DefaultPluginOwnerRow {}

impl Display for DefaultPluginOwnerRow {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "default")
    }
}

impl From<DefaultPluginOwner> for DefaultPluginOwnerRow {
    fn from(_: DefaultPluginOwner) -> Self {
        Self {}
    }
}

impl TryFrom<DefaultPluginOwnerRow> for DefaultPluginOwner {
    type Error = String;

    fn try_from(_: DefaultPluginOwnerRow) -> Result<Self, Self::Error> {
        Ok(DefaultPluginOwner {})
    }
}

impl<DB: Database> RowMeta<DB> for DefaultPluginOwnerRow {
    fn add_column_list<Sep: Display>(_builder: &mut Separated<DB, Sep>) {}

    fn add_where_clause(&self, builder: &mut QueryBuilder<DB>) {
        builder.push("1 = 1");
    }

    fn push_bind<'a, Sep: Display>(&'a self, _builder: &mut Separated<'_, 'a, DB, Sep>) {}
}

impl From<DefaultComponentOwnerRow> for DefaultPluginOwnerRow {
    fn from(_value: DefaultComponentOwnerRow) -> Self {
        Self {}
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct DefaultPluginScopeRow {
    scope_component_id: Option<Uuid>,
}

impl From<DefaultPluginScope> for DefaultPluginScopeRow {
    fn from(value: DefaultPluginScope) -> Self {
        match value {
            DefaultPluginScope::Global(_) => Self {
                scope_component_id: None,
            },
            DefaultPluginScope::Component(component) => Self {
                scope_component_id: Some(component.component_id.0),
            },
        }
    }
}

impl TryFrom<DefaultPluginScopeRow> for DefaultPluginScope {
    type Error = String;

    fn try_from(value: DefaultPluginScopeRow) -> Result<Self, Self::Error> {
        match value.scope_component_id {
            Some(component_id) => Ok(DefaultPluginScope::Component(ComponentPluginScope {
                component_id: ComponentId(component_id),
            })),
            None => Ok(DefaultPluginScope::Global(Empty {})),
        }
    }
}

impl<DB: Database> RowMeta<DB> for DefaultPluginScopeRow
where
    Uuid: for<'q> Encode<'q, DB> + Type<DB>,
    Option<Uuid>: for<'q> Encode<'q, DB> + Type<DB>,
{
    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("scope_component_id");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        if let Some(component_id) = &self.scope_component_id {
            builder.push("scope_component_id = ");
            builder.push_bind(component_id);
        } else {
            builder.push("scope_component_id IS NULL");
        }
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.scope_component_id);
    }
}
