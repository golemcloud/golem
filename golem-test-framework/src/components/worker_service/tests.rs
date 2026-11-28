// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::env_vars;
use crate::components::rdb::borrowed_sqlite::BorrowedSqliteRdb;
use crate::components::rdb::provided_postgres::ProvidedPostgresRdb;
use crate::components::rdb::{PostgresInfo, Rdb};
use crate::components::redis::Redis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::registry_service::RegistryService;
use crate::components::registry_service::provided::ProvidedRegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::shard_manager::provided::ProvidedShardManager;
use golem_common::config::{DbConfig, env_config_provider, server_settings};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use std::sync::Arc;
use test_r::test;
use tracing::Level;
use uuid::Uuid;

#[test]
fn sqlite_gateway_sessions_load_supported_state_after_stack_defaults() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let sqlite_root = jail.directory().join("stack-state");
        let postgres_sqlite_root = std::env::temp_dir().join("golem-worker-service-19093");
        let databases: [(Arc<dyn Rdb>, _); 2] = [
            (Arc::new(BorrowedSqliteRdb::new(&sqlite_root)), sqlite_root),
            (
                Arc::new(ProvidedPostgresRdb::new(PostgresInfo {
                    public_host: "postgres.example".to_string(),
                    public_port: 15432,
                    private_host: "postgres.internal".to_string(),
                    private_port: 5432,
                    database_name: "stack".to_string(),
                    username: "test-user".to_string(),
                    password: String::new(),
                })),
                postgres_sqlite_root,
            ),
        ];
        let shard_manager: Arc<dyn ShardManager> = Arc::new(ProvidedShardManager::new(
            "localhost".to_string(),
            19021,
            19020,
        ));
        let redis: Arc<dyn Redis> = Arc::new(ProvidedRedis::new(
            "cache.example".to_string(),
            16379,
            "test-cache:".to_string(),
        ));
        let registry: Arc<dyn RegistryService> =
            Arc::new(futures::executor::block_on(ProvidedRegistryService::new(
                "localhost".to_string(),
                18081,
                19091,
                AccountId(Uuid::nil()),
                AccountEmail::new("test@example.com"),
                TokenSecret::trusted("test-token".to_string()),
                AccountId(Uuid::nil()),
                PlanId(Uuid::nil()),
                None,
                None,
                None,
                None,
            )));
        databases.into_iter().for_each(|(rdb, expected_root)| {
            jail.clear_env();
            jail.set_env("GOLEM_TEST_GATEWAY_SESSION_STORAGE", "sqlite");
            jail.set_env("GOLEM__CACHE__ENABLED", "true");
            jail.set_env("GOLEM__DATA_DIR", "/inherited-state");
            let env = futures::executor::block_on(env_vars(
                18082,
                19092,
                19093,
                &shard_manager,
                &rdb,
                &redis,
                Level::INFO,
                false,
                &registry,
                false,
                false,
            ));
            assert!(
                env.keys().all(|key| !key.is_empty()),
                "a service environment must have nonempty variable names"
            );
            assert!(env.keys().all(|key| {
                !key.starts_with("GOLEM_ADVANCED__WORKER_SERVICE__GATEWAY_SESSION_STORAGE__")
            }));
            env.into_iter()
                .for_each(|(key, value)| jail.set_env(key, value));
            let loaded = server_settings::project("worker_service", &env_config_provider());
            match loaded
                .extract_inner::<DbConfig>("gateway_session_storage")
                .unwrap()
            {
                DbConfig::Sqlite(config) => {
                    assert_eq!(
                        std::path::Path::new(&config.database),
                        expected_root.join("gateway-sessions.db")
                    );
                    assert_eq!(config.max_connections, 4);
                    assert!(!config.foreign_keys);
                }
                other => panic!("expected supported SQLite session storage, got {other:?}"),
            }
        });
        Ok(())
    });
}
