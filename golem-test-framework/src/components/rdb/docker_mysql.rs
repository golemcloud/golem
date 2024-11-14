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
use crate::components::rdb::{mysql_wait_for_startup, DbInfo, MysqlInfo, Rdb, RdbConnectionString};
use crate::components::NETWORK;

pub struct DockerMysqlRdb {
    container: Arc<Mutex<Option<ContainerAsync<testcontainers_modules::mysql::Mysql>>>>,
    keep_container: bool,
    info: MysqlInfo,
}

impl DockerMysqlRdb {
    const DEFAULT_PORT: u16 = 3306;
    const DEFAULT_USERNAME: &'static str = "mysql";
    const DEFAULT_PASSWORD: &'static str = "mysql";
    const DEFAULT_DATABASE: &'static str = "mysql";

    // TODO: can we simplify this and get rid of local_env (and always use localhost and exposed ports)?
    pub async fn new(local_env: bool, keep_container: bool, port: Option<u16>) -> Self {
        let host_port = port.unwrap_or(Self::DEFAULT_PORT);
        info!("Starting Mysql container, host port {}", host_port);

        let database = Self::DEFAULT_DATABASE;
        let password = Self::DEFAULT_PASSWORD;
        let username = Self::DEFAULT_USERNAME;

        let name = "golem_mysql";

        let image = testcontainers_modules::mysql::Mysql::default()
            .with_tag("8")
            .with_env_var("MYSQL_PASSWORD", password)
            .with_env_var("MYSQL_USER", username)
            .with_env_var("MYSQL_DATABASE", database);

        let mut image = if local_env {
            image
        } else {
            image.with_container_name(name).with_network(NETWORK)
        };

        if let Some(port) = port {
            image = image.with_mapped_port(port, Self::DEFAULT_PORT.tcp());
        };

        let container = image
            .start()
            .await
            .unwrap_or_else(|_| panic!("Failed to start Mysql container, host port {}", host_port));

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

        let info = MysqlInfo {
            host: host.to_string(),
            port,
            host_port,
            database_name: database.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        };

        mysql_wait_for_startup(&info, Duration::from_secs(60)).await;

        Self {
            container: Arc::new(Mutex::new(Some(container))),
            keep_container,
            info,
        }
    }
}

#[async_trait]
impl Rdb for DockerMysqlRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Mysql(self.info.clone())
    }

    async fn kill(&self) {
        info!("Stopping Mysql container");
        self.container.kill(self.keep_container).await;
    }
}

impl RdbConnectionString for DockerMysqlRdb {
    fn connection_string(&self) -> String {
        self.info.connection_string()
    }

    fn host_connection_string(&self) -> String {
        self.info.host_connection_string()
    }
}

impl Debug for DockerMysqlRdb {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerMysqlRdb")
    }
}

pub struct DockerMysqlRdbs {
    pub rdbs: Vec<Arc<DockerMysqlRdb>>,
}

impl DockerMysqlRdbs {
    pub async fn make(local_env: bool, keep_container: bool, port: u16) -> Arc<DockerMysqlRdb> {
        Arc::new(DockerMysqlRdb::new(local_env, keep_container, Some(port)).await)
    }

    pub async fn new(size: usize, base_port: u16, local_env: bool, keep_container: bool) -> Self {
        info!("Starting multiple Mysql containers of size {size}");
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

impl Debug for DockerMysqlRdbs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerMysqlRdbs")
    }
}
