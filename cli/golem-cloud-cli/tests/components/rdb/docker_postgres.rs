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

use crate::components::rdb::{wait_for_startup, DbInfo, PostgresInfo, Rdb};
use crate::components::{DOCKER, NETWORK};
use std::time::Duration;
use testcontainers::{Container, RunnableImage};
use tracing::info;

pub struct DockerPostgresRdb {
    container: Container<'static, testcontainers_modules::postgres::Postgres>,
    host: String,
    port: u16,
    host_port: u16,
}

impl DockerPostgresRdb {
    const DEFAULT_PORT: u16 = 5432;

    // TODO: can we simplify this and get rid of local_env (and always use localhost and exposed ports)?
    pub async fn new(local_env: bool) -> Self {
        info!("Starting Postgres container");

        let name = "golem_postgres";
        let image = RunnableImage::from(testcontainers_modules::postgres::Postgres::default())
            .with_tag("12");

        let image = if local_env {
            image
        } else {
            image.with_container_name(name).with_network(NETWORK)
        };

        let container = DOCKER.run(image);

        let host = if local_env { "localhost" } else { name };
        let port = if local_env {
            container.get_host_port_ipv4(Self::DEFAULT_PORT)
        } else {
            Self::DEFAULT_PORT
        };

        let host_port = container.get_host_port_ipv4(Self::DEFAULT_PORT);

        wait_for_startup("localhost", host_port, Duration::from_secs(30)).await;

        Self {
            container,
            host: host.to_string(),
            port,
            host_port,
        }
    }
}

impl Rdb for DockerPostgresRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Postgres(PostgresInfo {
            host: self.host.clone(),
            port: self.port,
            host_port: self.host_port,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
        })
    }

    fn kill(&self) {
        info!("Stopping Postgres container");
        self.container.stop();
    }
}

impl Drop for DockerPostgresRdb {
    fn drop(&mut self) {
        self.kill();
    }
}
