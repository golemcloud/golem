// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::app::template::AppTemplate;
use crate::model::language::GuestLanguage;
use serde::{Deserialize, Serialize};

/// A summary view of an [`AppTemplate`] for CLI output (the `app templates` listing).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TemplateDescription {
    pub name: String,
    pub language: GuestLanguage,
    pub description: String,
}

impl TemplateDescription {
    pub fn from_template(template: &AppTemplate) -> Self {
        Self {
            name: template.name.as_str().to_string(),
            language: template.language,
            description: template.description().to_string(),
        }
    }
}
