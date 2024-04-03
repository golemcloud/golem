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

use crate::components::redis::Redis;
use crate::components::NETWORK;
use std::sync::atomic::{AtomicBool, Ordering};
use testcontainers::{Container, RunnableImage};
use testcontainers_modules::redis::REDIS_PORT;
use tracing::info;

pub struct DockerRedis<'d> {
    container: Container<'d, testcontainers_modules::redis::Redis>,
    prefix: String,
    valid: AtomicBool,
}

impl<'d> DockerRedis<'d> {
    pub fn new(docker: &'d testcontainers::clients::Cli, prefix: String) -> Self {
        info!("Starting Redis container");

        let name = "golem_redis";
        let image = RunnableImage::from(testcontainers_modules::redis::Redis)
            .with_tag("7.2")
            .with_container_name(name)
            .with_network(NETWORK);
        let container = docker.run(image);

        super::wait_for_startup("localhost", container.get_host_port_ipv4(REDIS_PORT));

        Self {
            container,
            prefix,
            valid: AtomicBool::new(true),
        }
    }
}

impl<'d> Redis for DockerRedis<'d> {
    fn assert_valid(&self) {
        if !self.valid.load(Ordering::Acquire) {
            std::panic!("Redis has been closed")
        }
    }

    fn host(&self) -> &str {
        "localhost"
    }

    fn port(&self) -> u16 {
        self.container.get_host_port_ipv4(REDIS_PORT)
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    fn kill(&self) {
        info!("Stopping Redis container");
        self.valid.store(false, Ordering::Release);
        self.container.stop();
    }
}

impl<'d> Drop for DockerRedis<'d> {
    fn drop(&mut self) {
        self.kill()
    }
}
