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

use crate::model::TemplateDescription;
use crate::model::text::fmt::*;

impl TextView for Vec<TemplateDescription> {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Name").fixed(),
            Column::new("Language").fixed(),
            Column::new("Description"),
        ]);
        for tmpl in self {
            table.add_row(vec![
                tmpl.name.to_string(),
                tmpl.language.to_string(),
                tmpl.description.clone(),
            ]);
        }
        log_table(table);
    }
}
