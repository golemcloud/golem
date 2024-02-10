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

use std::time::Duration;

use serde::Deserialize;
use url::Url;

#[derive(Clone, Debug, Deserialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub database: usize,
    pub tracing: bool,
    pub pool_size: usize,
    pub retries: RetryConfig,
    pub key_prefix: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RedisConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!(
            "redis://{}:{}/{}",
            self.host, self.port, self.database
        ))
        .expect("Failed to parse Redis URL")
    }
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 6379,
            database: 0,
            tracing: false,
            pool_size: 8,
            retries: RetryConfig::default(),
            key_prefix: "".to_string(),
            username: None,
            password: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2,
        }
    }
}
