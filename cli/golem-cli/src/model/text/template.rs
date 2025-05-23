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

use crate::model::text::fmt::*;
use crate::model::TemplateDescription;
use cli_table::Table;
use golem_templates::model::{GuestLanguage, GuestLanguageTier, TemplateName};

#[derive(Table)]
pub struct TemplateDescriptionTableView {
    #[table(title = "Name")]
    pub name: TemplateName,
    #[table(title = "Language")]
    pub language: GuestLanguage,
    #[table(title = "Tier")]
    pub tier: GuestLanguageTier,
    #[table(title = "Description")]
    pub description: String,
}

impl From<&TemplateDescription> for TemplateDescriptionTableView {
    fn from(value: &TemplateDescription) -> Self {
        Self {
            name: value.name.clone(),
            language: value.language,
            tier: value.tier.clone(),
            description: textwrap::wrap(&value.description, 30).join("\n"),
        }
    }
}

impl TextView for Vec<TemplateDescription> {
    fn log(&self) {
        log_table::<_, TemplateDescriptionTableView>(self);
    }
}
