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

use crate::components::docker::{get_docker_container_name, network, ContainerHandle};
use crate::components::redis::Redis;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::redis::REDIS_PORT;
use tracing::info;

pub struct DockerRedis {
    container: ContainerHandle<testcontainers_modules::redis::Redis>,
    prefix: String,
    valid: AtomicBool,
    private_host: String,
    public_port: u16,
}

impl DockerRedis {
    pub async fn new(unique_network_id: &str, prefix: String) -> Self {
        info!("Starting Redis container");

        let container = tryhard::retry_fn(|| {
            testcontainers_modules::redis::Redis::default()
                .with_tag("7.2")
                .with_network(network(unique_network_id))
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start Redis container");

        let public_port = container
            .get_host_port_ipv4(REDIS_PORT)
            .await
            .expect("Failed to get host port");

        super::wait_for_startup("localhost", public_port, Duration::from_secs(10));

        let private_host = get_docker_container_name(unique_network_id, container.id()).await;

        Self {
            container: ContainerHandle::new(container),
            prefix,
            valid: AtomicBool::new(true),
            private_host,
            public_port,
        }
    }
}

#[async_trait]
impl Redis for DockerRedis {
    fn assert_valid(&self) {
        if !self.valid.load(Ordering::Acquire) {
            std::panic!("Redis has been closed")
        }
    }

    fn private_host(&self) -> String {
        self.private_host.clone()
    }

    fn private_port(&self) -> u16 {
        REDIS_PORT
    }

    fn public_host(&self) -> String {
        "localhost".to_string()
    }

    fn public_port(&self) -> u16 {
        self.public_port
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}
