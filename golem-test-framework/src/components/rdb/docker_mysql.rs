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

use crate::components::docker::{get_docker_container_name, ContainerHandle, NETWORK};
use crate::components::rdb::{mysql_wait_for_startup, DbInfo, MysqlInfo, Rdb};
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use tracing::info;

pub struct DockerMysqlRdb {
    container: ContainerHandle<testcontainers_modules::mysql::Mysql>,
    info: MysqlInfo,
}

impl DockerMysqlRdb {
    const DEFAULT_PORT: u16 = 3306;
    const DEFAULT_USERNAME: &'static str = "root";
    const DEFAULT_PASSWORD: &'static str = "mysql";
    const DEFAULT_DATABASE: &'static str = "mysql";

    pub async fn new() -> Self {
        info!("Starting Mysql container");

        let database = Self::DEFAULT_DATABASE;
        let password = Self::DEFAULT_PASSWORD;
        let username = Self::DEFAULT_USERNAME;
        let port = Self::DEFAULT_PORT;

        let container = testcontainers_modules::mysql::Mysql::default()
            .with_tag("8")
            .with_env_var("MYSQL_ROOT_PASSWORD", password)
            .with_env_var("MYSQL_DATABASE", database)
            .with_network(NETWORK)
            .start()
            .await
            .expect("Failed to start Mysql container");

        let private_host = get_docker_container_name(container.id()).await;

        let public_port = container
            .get_host_port_ipv4(port)
            .await
            .expect("Failed to get host port");

        let info = MysqlInfo {
            public_host: "localhost".to_string(),
            public_port,
            private_host,
            private_port: port,
            database_name: database.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        };

        mysql_wait_for_startup(&info, Duration::from_secs(60)).await;

        Self {
            container: ContainerHandle::new(container),
            info,
        }
    }

    pub fn public_connection_string(&self) -> String {
        self.info.public_connection_string()
    }

    pub fn public_connection_string_to_db(&self, db_name: &str) -> String {
        let db_info = MysqlInfo {
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
impl Rdb for DockerMysqlRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Mysql(self.info.clone())
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

impl Debug for DockerMysqlRdb {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerMysqlRdb")
    }
}
