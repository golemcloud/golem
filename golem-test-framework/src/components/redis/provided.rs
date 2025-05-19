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

use crate::components::redis::Redis;
use async_trait::async_trait;
use tracing::info;

pub struct ProvidedRedis {
    host: String,
    port: u16,
    prefix: String,
}

impl ProvidedRedis {
    pub fn new_default() -> Self {
        Self::new("localhost".to_string(), super::DEFAULT_PORT, "".to_string())
    }

    pub fn new(host: String, port: u16, prefix: String) -> Self {
        info!("Using already running Redis on {}:{}", host, port);
        Self { host, port, prefix }
    }
}

#[async_trait]
impl Redis for ProvidedRedis {
    fn assert_valid(&self) {}

    fn private_host(&self) -> String {
        self.host.to_string()
    }

    fn private_port(&self) -> u16 {
        self.port
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    async fn kill(&self) {}
}
