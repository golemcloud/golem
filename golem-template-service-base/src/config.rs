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

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
#[derive(Default)]
pub enum TemplateCompilationConfig {
    Enabled(TemplateCompilationEnabledConfig),
    #[default]
    Disabled,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
}

impl TemplateCompilationEnabledConfig {
    pub fn uri(&self) -> http_02::Uri {
        http_02::Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build TemplateCompilationService URI")
    }
}
