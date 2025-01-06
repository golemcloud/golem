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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::redis::REDIS_PORT;
use tokio::sync::Mutex;
use tracing::info;

use crate::components::docker::KillContainer;
use crate::components::redis::Redis;
use crate::components::NETWORK;

pub struct DockerRedis {
    container: Arc<Mutex<Option<ContainerAsync<testcontainers_modules::redis::Redis>>>>,
    keep_container: bool,
    prefix: String,
    valid: AtomicBool,
    public_port: u16,
}

impl DockerRedis {
    const NAME: &'static str = "golem_redis";

    pub async fn new(prefix: String, keep_container: bool) -> Self {
        info!("Starting Redis container");

        let container = testcontainers_modules::redis::Redis::default()
            .with_tag("7.2")
            .with_container_name(Self::NAME)
            .with_network(NETWORK)
            .start()
            .await
            .expect("Failed to start Redis container");

        let public_port = container
            .get_host_port_ipv4(REDIS_PORT)
            .await
            .expect("Failed to get host port");

        super::wait_for_startup("localhost", public_port, Duration::from_secs(10));

        Self {
            container: Arc::new(Mutex::new(Some(container))),
            keep_container,
            prefix,
            valid: AtomicBool::new(true),
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
        Self::NAME.to_string()
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
        info!("Stopping Redis container");
        self.container.kill(self.keep_container).await;
    }
}
