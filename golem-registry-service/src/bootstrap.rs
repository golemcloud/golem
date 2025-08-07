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

use crate::config::RegistryServiceConfig;
use golem_common::config::DbConfig;
use golem_service_base::db::Pool;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;

#[derive(Clone)]
pub struct Services {}

impl Services {
    pub async fn new(config: &RegistryServiceConfig) -> Result<Self, String> {
        match config.db.clone() {
            DbConfig::Postgres(db_config) => {
                let db_pool = PostgresPool::configured(&db_config)
                    .await
                    .map_err(|e| e.to_string())?;
                Self::make_with_db(config, db_pool).await
            }
            DbConfig::Sqlite(db_config) => {
                let db_pool = SqlitePool::configured(&db_config)
                    .await
                    .map_err(|e| e.to_string())?;
                Self::make_with_db(config, db_pool).await
            }
        }
    }

    async fn make_with_db<DBP>(
        _config: &RegistryServiceConfig,
        _db_pool: DBP,
    ) -> Result<Self, String>
    where
        DBP: Pool + Clone + Send + Sync + 'static,
    {
        Ok(Self {})
    }
}
