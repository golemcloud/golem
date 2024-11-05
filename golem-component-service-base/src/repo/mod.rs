// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use sqlx::{Database, QueryBuilder};

pub mod component;
pub mod plugin;
pub mod plugin_installation;

pub trait RowMeta<DB: Database> {
    fn add_column_list(builder: &mut QueryBuilder<DB>) -> bool;
    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>);
    fn push_bind<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) -> bool;
}
