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

// pub mod auth;
// pub mod limit;
pub mod registry;
// pub mod plugin;

use golem_common::SafeDisplay;
use golem_common::config::{ConfigExample, HasConfigExamples};
use golem_common::model::RetryConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteServiceConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
}

impl RemoteServiceConfig {
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

impl SafeDisplay for RemoteServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "access_token: ****");
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

impl Default for RemoteServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            retries: RetryConfig::default(),
        }
    }
}

impl HasConfigExamples<RemoteServiceConfig> for RemoteServiceConfig {
    fn examples() -> Vec<ConfigExample<RemoteServiceConfig>> {
        vec![]
    }
}
