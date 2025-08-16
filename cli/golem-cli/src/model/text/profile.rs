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

use crate::config::ProfileConfig;
use crate::log::{logln, LogColorize};
use crate::model::text::fmt::*;
use crate::model::ProfileView;
use colored::Colorize;

impl TextView for Vec<ProfileView> {
    fn log(&self) {
        logln("Available profiles:".log_color_help_group().to_string());
        for profile in self {
            logln(format!(
                " {} {}{}",
                if profile.is_active { "*" } else { " " },
                format_id(&profile.name),
                if profile.name.is_builtin() {
                    ", builtin"
                } else {
                    ""
                },
            ));
        }
    }
}

impl MessageWithFields for ProfileView {
    fn message(&self) -> String {
        format!("Profile {}", format_message_highlight(&self.name))
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field_optional("Active", &self.is_active, self.is_active, |b| {
                b.to_string().green().to_string()
            })
            .fmt_field_optional(
                "Allow insecure",
                &self.allow_insecure,
                self.allow_insecure,
                |b| b.to_string().red().to_string(),
            )
            .field("Default output format", &self.config.default_format);

        if let Some(url) = &self.url {
            if let Some(worker_url) = &self.worker_url {
                fields
                    .field("Component service URL", url)
                    .field("Worker service URL", worker_url);
            } else {
                fields.field("Service URL", url);
            }
        } else {
            fields.field("Using default URLs", &true);
        }

        fields.build()
    }
}

impl TextView for ProfileConfig {
    fn log(&self) {
        logln(format!(
            "Default output format: {}",
            format_message_highlight(&self.default_format),
        ))
    }
}
