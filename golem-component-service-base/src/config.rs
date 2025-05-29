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

use golem_common::model::{Empty, RetryConfig};
use http::Uri;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentCompilationConfig {
    Enabled(ComponentCompilationEnabledConfig),
    Disabled(Empty),
}

impl Default for ComponentCompilationConfig {
    fn default() -> Self {
        Self::Enabled(ComponentCompilationEnabledConfig {
            host: "localhost".to_string(),
            port: 9091,
            retries: RetryConfig::default(),
            connect_timeout: Duration::from_secs(10),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
}

impl ComponentCompilationEnabledConfig {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentCompilationService URI")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PluginTransformationsConfig {
    pub(crate) retries: RetryConfig,
}
