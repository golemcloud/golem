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

use golem_common::model::Empty;
use http::Uri;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentStoreConfig {
    S3(ComponentStoreS3Config),
    Local(ComponentStoreLocalConfig),
}

impl Default for ComponentStoreConfig {
    fn default() -> Self {
        ComponentStoreConfig::Local(ComponentStoreLocalConfig {
            root_path: "/tmp".to_string(),
            object_prefix: "".to_string(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentStoreS3Config {
    pub bucket_name: String,
    pub object_prefix: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentStoreLocalConfig {
    pub root_path: String,
    pub object_prefix: String,
}

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
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
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
