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

mod sqlx_rdbms;
pub mod types;

use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::{Rdbms, RdbmsType};
use std::fmt::Display;
use std::sync::Arc;

pub(crate) const POSTGRES: &str = "postgres";

#[derive(Debug, Clone, Default)]
pub struct PostgresType;

impl PostgresType {
    pub fn new_rdbms(config: RdbmsConfig) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        sqlx_rdbms::new(config)
    }
}

impl RdbmsType for PostgresType {
    type DbColumn = types::DbColumn;
    type DbValue = types::DbValue;
}

impl Display for PostgresType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", POSTGRES)
    }
}
