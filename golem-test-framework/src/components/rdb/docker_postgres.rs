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
use std::sync::Arc;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::Mutex;
use tracing::info;

use crate::components::docker::KillContainer;
use crate::components::rdb::{wait_for_startup, DbInfo, PostgresInfo, Rdb};
use crate::components::NETWORK;

pub struct DockerPostgresRdb {
    container: Arc<Mutex<Option<ContainerAsync<testcontainers_modules::postgres::Postgres>>>>,
    keep_container: bool,
    host: String,
    port: u16,
    host_port: u16,
}

impl DockerPostgresRdb {
    const DEFAULT_PORT: u16 = 5432;

    // TODO: can we simplify this and get rid of local_env (and always use localhost and exposed ports)?
    pub async fn new(local_env: bool, keep_container: bool) -> Self {
        info!("Starting Postgres container");

        let name = "golem_postgres";
        let image = testcontainers_modules::postgres::Postgres::default().with_tag("12");

        let image = if local_env {
            image
        } else {
            image.with_container_name(name).with_network(NETWORK)
        };

        let container = image
            .start()
            .await
            .expect("Failed to start Postgres container");

        let host = if local_env { "localhost" } else { name };
        let port = if local_env {
            container
                .get_host_port_ipv4(Self::DEFAULT_PORT)
                .await
                .expect("Failed to get host port")
        } else {
            Self::DEFAULT_PORT
        };

        let host_port = container
            .get_host_port_ipv4(Self::DEFAULT_PORT)
            .await
            .expect("Failed to get host port");

        wait_for_startup("localhost", host_port, Duration::from_secs(30)).await;

        Self {
            container: Arc::new(Mutex::new(Some(container))),
            keep_container,
            host: host.to_string(),
            port,
            host_port,
        }
    }
}

#[async_trait]
impl Rdb for DockerPostgresRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Postgres(PostgresInfo {
            host: self.host.clone(),
            port: self.port,
            host_port: self.host_port,
            database_name: "postgres".to_string(),
            username: "postgres".to_string(),
            password: "postgres".to_string(),
        })
    }

    async fn kill(&self) {
        info!("Stopping Postgres container");
        self.container.kill(self.keep_container).await;
    }
}
