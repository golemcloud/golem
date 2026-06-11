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

use crate::log::current_indent_width;
use crate::model::TemplateDescription;
use crate::model::cli_output::CliOutput;
use crate::model::text::fmt::*;
use itertools::Itertools;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TemplateListView {
    pub templates: Vec<TemplateDescription>,
}

impl CliOutput for TemplateListView {
    const KIND: &'static str = "app.templates.result";
}

impl TextView for TemplateListView {
    fn log(&self) {
        let raw_name_width = self
            .templates
            .iter()
            .map(|tmpl| tmpl.name.len())
            .max()
            .unwrap_or(0)
            .max("Name".len());
        let raw_description_width = self
            .templates
            .iter()
            .map(|tmpl| tmpl.description.len())
            .max()
            .unwrap_or(0)
            .max("Description".len());

        let indent_width = current_indent_width();
        let terminal_width = (terminal_width() as usize).saturating_sub(indent_width);
        let table_overhead = 7;
        let available_content_width = terminal_width.saturating_sub(table_overhead);

        let min_name_width = "Name".len();
        let min_description_width = "Description".len();
        let name_width = raw_name_width.max(min_name_width);
        let description_width = available_content_width
            .saturating_sub(name_width)
            .max(min_description_width);
        let description_max_width =
            description_width.max(raw_description_width.max(min_description_width));

        for (idx, (language, templates)) in self
            .templates
            .iter()
            .chunk_by(|tmpl| tmpl.language)
            .into_iter()
            .enumerate()
        {
            if idx > 0 {
                logln("");
            }

            logln(language.to_string());

            let mut table = new_table_full_condensed(vec![
                Column::new("Name").exact_width(name_width),
                Column::new("Description")
                    .width_range(min_description_width, description_max_width),
            ]);

            for tmpl in templates {
                table.add_row(vec![tmpl.name.clone(), tmpl.description.clone()]);
            }

            log_table(table);
        }
    }
}
