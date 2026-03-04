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

use crate::app::template::repo::TEMPLATES_DIR;

use anyhow::Context;
use serde_derive::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AppTemplateMetadata {
    #[serde(rename_all = "camelCase")]
    Common {
        description: Option<String>,
        skip_if_exists: Option<PathBuf>,
        exclude: Option<Vec<String>>,
        dev_only: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    CommonOnDemand {
        description: Option<String>,
        exclude: Option<Vec<String>>,
        dev_only: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    Component {
        description: String,
        exclude: Option<Vec<String>>,
        dev_only: Option<bool>,
    },
}

impl AppTemplateMetadata {
    pub fn load(template_path: &Path) -> anyhow::Result<AppTemplateMetadata> {
        let raw_metadata = TEMPLATES_DIR
            .get_file(template_path.join("metadata.json"))
            .expect("Failed to read metadata JSON")
            .contents();
        serde_json::from_slice(raw_metadata).with_context(|| {
            format!(
                "Failed to parse metadata JSON for template at {}",
                template_path.display()
            )
        })
    }

    pub fn is_common(&self) -> bool {
        matches!(self, AppTemplateMetadata::Common { .. })
    }

    pub fn is_common_on_demand(&self) -> bool {
        matches!(self, AppTemplateMetadata::CommonOnDemand { .. })
    }

    pub fn is_component(&self) -> bool {
        matches!(self, AppTemplateMetadata::Component { .. })
    }
}
