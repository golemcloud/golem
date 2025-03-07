// Copyright 2024-2025 Golem Cloud
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

use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::Mutex;
use tracing::info;

use crate::components::docker::KillContainer;
use crate::components::rdb::{postgres_wait_for_startup, DbInfo, PostgresInfo, Rdb};
use crate::components::NETWORK;

pub struct DockerPostgresRdb {
    container: Arc<Mutex<Option<ContainerAsync<testcontainers_modules::postgres::Postgres>>>>,
    keep_container: bool,
    info: PostgresInfo,
}

impl DockerPostgresRdb {
    const DEFAULT_NAME: &'static str = "golem_postgres";
    const DEFAULT_PORT: u16 = 5432;
    const DEFAULT_USERNAME: &'static str = "postgres";
    const DEFAULT_PASSWORD: &'static str = "postgres";
    const DEFAULT_DATABASE: &'static str = "postgres";

    pub async fn new(keep_container: bool) -> Self {
        info!("Starting Postgres container");

        let database = Self::DEFAULT_DATABASE;
        let password = Self::DEFAULT_PASSWORD;
        let username = Self::DEFAULT_USERNAME;
        let port = Self::DEFAULT_PORT;
        let name = Self::DEFAULT_NAME;

        let container = testcontainers_modules::postgres::Postgres::default()
            .with_tag("12")
            .with_env_var("POSTGRES_DB", database)
            .with_env_var("POSTGRES_PASSWORD", password)
            .with_env_var("POSTGRES_USER", username)
            .with_container_name(name)
            .with_network(NETWORK)
            .start()
            .await
            .expect("Failed to start Postgres container");

        let public_port = container
            .get_host_port_ipv4(port)
            .await
            .expect("Failed to get host port");

        let info = PostgresInfo {
            public_host: "localhost".to_string(),
            public_port,
            private_host: name.to_string(),
            private_port: port,
            database_name: database.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        };

        postgres_wait_for_startup(&info, Duration::from_secs(30)).await;

        Self {
            container: Arc::new(Mutex::new(Some(container))),
            keep_container,
            info,
        }
    }

    pub fn public_connection_string(&self) -> String {
        self.info.public_connection_string()
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
        info!("Stopping Postgres container");
        self.container.kill(self.keep_container).await;
    }
}

impl Debug for DockerPostgresRdb {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerPostgresRdb")
    }
}
