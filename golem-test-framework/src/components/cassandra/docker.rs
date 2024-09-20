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

use super::Cassandra;
use crate::components::{DOCKER, NETWORK};
use std::sync::atomic::{AtomicBool, Ordering};
use testcontainers::{Container, GenericImage, RunnableImage};
use tracing::info;

pub struct DockerCassandra {
    container: Container<'static, GenericImage>,
    keep_container: bool,
    valid: AtomicBool,
    public_port: u16,
}

impl DockerCassandra {
    const NAME: &'static str = "golem_cassandra";

    pub fn new(keep_container: bool) -> Self {
        let image = GenericImage::new("cassandra", "latest")
            .with_exposed_port(super::DEFAULT_PORT)
            .with_wait_for(testcontainers::core::WaitFor::message_on_stdout(
                "Starting listening for CQL clients on",
            ));
        let cassandra_image: RunnableImage<_> = RunnableImage::from(image)
            .with_container_name(Self::NAME)
            .with_network(NETWORK);

        let container = DOCKER.run(cassandra_image);
        let public_port: u16 = container.get_host_port_ipv4(super::DEFAULT_PORT);

        DockerCassandra {
            container,
            keep_container,
            valid: AtomicBool::new(true),
            public_port,
        }
    }
}

impl Cassandra for DockerCassandra {
    fn assert_valid(&self) {
        if !self.valid.load(Ordering::Acquire) {
            std::panic!("Cassandra has been closed")
        }
    }

    fn private_known_nodes(&self) -> Vec<String> {
        vec![format!("{}:{}", Self::NAME, super::DEFAULT_PORT)]
    }

    fn kill(&self) {
        info!("Stopping Cassandra container");
        if self.keep_container {
            self.container.stop()
        } else {
            self.container.rm()
        }
    }

    fn public_known_nodes(&self) -> Vec<String> {
        vec![format!("localhost:{}", self.public_port)]
    }
}

impl Drop for DockerCassandra {
    fn drop(&mut self) {
        self.kill()
    }
}
