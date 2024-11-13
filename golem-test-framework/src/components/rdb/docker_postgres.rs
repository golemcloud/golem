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

use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::core::IntoContainerPort;
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
    const DEFAULT_PORT: u16 = 5432;
    const DEFAULT_USERNAME: &'static str = "postgres";
    const DEFAULT_PASSWORD: &'static str = "postgres";
    const DEFAULT_DATABASE: &'static str = "postgres";

    // TODO: can we simplify this and get rid of local_env (and always use localhost and exposed ports)?
    pub async fn new(local_env: bool, keep_container: bool, port: Option<u16>) -> Self {
        let host_port = port.unwrap_or(Self::DEFAULT_PORT);
        info!("Starting Postgres container, host port {}", host_port);
        let database = Self::DEFAULT_DATABASE;
        let password = Self::DEFAULT_PASSWORD;
        let username = Self::DEFAULT_USERNAME;

        let name = "golem_postgres";
        let image = testcontainers_modules::postgres::Postgres::default()
            .with_tag("12")
            .with_env_var("POSTGRES_DB", database)
            .with_env_var("POSTGRES_PASSWORD", password)
            .with_env_var("POSTGRES_USER", username);

        let mut image = if local_env {
            image
        } else {
            image.with_container_name(name).with_network(NETWORK)
        };

        if let Some(port) = port {
            image = image.with_mapped_port(port, Self::DEFAULT_PORT.tcp());
        };

        let container = image.start().await.unwrap_or_else(|_| {
            panic!(
                "Failed to start Postgres container, host port {}",
                host_port
            )
        });

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

        let info = PostgresInfo {
            host: host.to_string(),
            port,
            host_port,
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

    pub fn postgres_info(&self) -> PostgresInfo {
        self.info.clone()
    }
}

#[async_trait]
impl Rdb for DockerPostgresRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Postgres(self.postgres_info())
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

pub struct DockerPostgresRdbs {
    pub rdbs: Vec<Arc<DockerPostgresRdb>>,
}

impl DockerPostgresRdbs {
    pub async fn make(local_env: bool, keep_container: bool, port: u16) -> Arc<DockerPostgresRdb> {
        Arc::new(DockerPostgresRdb::new(local_env, keep_container, Some(port)).await)
    }

    pub async fn new(size: usize, base_port: u16, local_env: bool, keep_container: bool) -> Self {
        info!("Starting multiple Postgres containers of size {size}");
        let mut rdbs_joins = Vec::new();

        for i in 0..size {
            let port = base_port + i as u16;

            let db = tokio::spawn(Self::make(local_env, keep_container, port));

            rdbs_joins.push(db);
        }

        let mut rdbs = Vec::new();

        for join in rdbs_joins {
            rdbs.push(join.await.expect("Failed to join"));
        }

        Self { rdbs }
    }
}
