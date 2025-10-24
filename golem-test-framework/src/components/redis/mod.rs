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

use async_trait::async_trait;
use redis::RedisResult;
use std::time::{Duration, Instant};
use tracing::info;

pub mod provided;
pub mod spawned;

#[async_trait]
pub trait Redis: Send + Sync {
    fn assert_valid(&self);

    fn private_host(&self) -> String;
    fn private_port(&self) -> u16;

    fn public_host(&self) -> String {
        self.private_host()
    }
    fn public_port(&self) -> u16 {
        self.private_port()
    }

    fn prefix(&self) -> &str;

    async fn kill(&self);

    fn try_get_connection(&self, db: u16) -> RedisResult<redis::Connection> {
        let client = redis::Client::open(format!(
            "redis://{}:{}/{}",
            self.public_host(),
            self.public_port(),
            db
        ))?;
        client.get_connection()
    }

    async fn try_get_async_connection(
        &self,
        db: u16,
    ) -> RedisResult<redis::aio::MultiplexedConnection> {
        let client = redis::Client::open(format!(
            "redis://{}:{}/{}",
            self.public_host(),
            self.public_port(),
            db
        ))?;
        client.get_multiplexed_async_connection().await
    }

    fn get_connection(&self, db: u16) -> redis::Connection {
        self.assert_valid();
        self.try_get_connection(db).unwrap()
    }

    async fn get_async_connection(&self, db: u16) -> redis::aio::MultiplexedConnection {
        self.assert_valid();
        self.try_get_async_connection(db).await.unwrap()
    }

    fn flush_db(&self, db: u16) {
        let mut connection = self.get_connection(db);
        redis::cmd("FLUSHDB").exec(&mut connection).unwrap()
    }
}

const DEFAULT_PORT: u16 = 6379;

pub fn check_if_running(host: &str, port: u16) -> bool {
    let mut client = redis::Client::open(format!("redis://{host}:{port}")).unwrap();
    let result: RedisResult<Vec<String>> = redis::cmd("INFO").arg("server").query(&mut client);
    result.is_ok()
}

fn wait_for_startup(host: &str, port: u16, timeout: Duration) {
    info!(
        "Waiting for Redis start on host {host}:{port}, timeout: {}s",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let is_running = check_if_running(host, port);
        if is_running {
            break;
        }

        if start.elapsed() > timeout {
            std::panic!("Failed to verify that Redis is running");
        }
    }
}
