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

pub mod registry;

use golem_common::SafeDisplay;
use golem_common::config::{ConfigExample, HasConfigExamples};
use golem_common::model::RetryConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::time::Duration;
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryServiceConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
    pub max_message_size: usize,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
}

impl RegistryServiceConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build service URI")
    }
}

impl SafeDisplay for RegistryServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "access_token: ****");
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        let _ = writeln!(&mut result, "max_message_size: {}", self.max_message_size);
        let _ = writeln!(&mut result, "connect_timeout: {:?}", self.connect_timeout);
        result
    }
}

impl Default for RegistryServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            retries: RetryConfig::default(),
            max_message_size: 50 * 1024 * 1024,
            connect_timeout: Duration::from_secs(10),
        }
    }
}

impl HasConfigExamples<RegistryServiceConfig> for RegistryServiceConfig {
    fn examples() -> Vec<ConfigExample<RegistryServiceConfig>> {
        vec![]
    }
}
