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

use crate::components::docker::{get_docker_container_name, ContainerHandle};
use crate::components::rdb::{postgres_wait_for_startup, DbInfo, PostgresInfo, Rdb};
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use tracing::info;

pub struct DockerPostgresRdb {
    container: ContainerHandle<Postgres>,
    info: PostgresInfo,
}

impl DockerPostgresRdb {
    const DEFAULT_PORT: u16 = 5432;
    const DEFAULT_USERNAME: &'static str = "postgres";
    const DEFAULT_PASSWORD: &'static str = "postgres";
    const DEFAULT_DATABASE: &'static str = "postgres";

    pub async fn new(unique_network_id: &str) -> Self {
        info!("Starting Postgres container");

        let database = Self::DEFAULT_DATABASE;
        let password = Self::DEFAULT_PASSWORD;
        let username = Self::DEFAULT_USERNAME;
        let port = Self::DEFAULT_PORT;

        let container = tryhard::retry_fn(|| {
            Postgres::default()
                .with_tag("14")
                .with_env_var("POSTGRES_DB", database)
                .with_env_var("POSTGRES_PASSWORD", password)
                .with_env_var("POSTGRES_USER", username)
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start Postgres container");

        let private_host = get_docker_container_name(unique_network_id, container.id()).await;

        let public_port = container
            .get_host_port_ipv4(port)
            .await
            .expect("Failed to get host port");

        let info = PostgresInfo {
            public_host: "localhost".to_string(),
            public_port,
            private_host,
            private_port: port,
            database_name: database.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        };

        postgres_wait_for_startup(&info, Duration::from_secs(30)).await;

        Self {
            container: ContainerHandle::new(container),
            info,
        }
    }

    pub fn public_connection_string(&self) -> String {
        self.info.public_connection_string()
    }

    pub fn public_connection_string_to_db(&self, db_name: &str) -> String {
        let db_info = PostgresInfo {
            database_name: db_name.to_string(),
            ..self.info.clone()
        };

        db_info.public_connection_string()
    }

    pub fn private_connection_string(&self) -> String {
        self.info.private_connection_string()
    }
}

#[async_trait]
impl Rdb for DockerPostgresRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Postgres(self.info.clone())
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

impl Debug for DockerPostgresRdb {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerPostgresRdb")
    }
}
