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

use serde_derive::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

// NOTE: using aliases for lower-case support in manifest, as global configs are using the
//       PascalCase versions historically, should be cleared up (migrated), if we touch the global
//       CLI config
#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize, Default)]
pub enum Format {
    #[serde(alias = "json")]
    Json,
    #[serde(alias = "pretty-json", alias = "pretty")]
    PrettyJson,
    #[serde(alias = "yaml")]
    Yaml,
    #[serde(alias = "pretty-yaml")]
    PrettyYaml,
    #[default]
    #[serde(alias = "text")]
    Text,
}

impl Display for Format {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::PrettyJson => "pretty-json",
            Self::Yaml => "yaml",
            Self::PrettyYaml => "pretty-yaml",
            Self::Text => "text",
        };
        Display::fmt(&s, f)
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Format::Json),
            "pretty" | "pretty-json" => Ok(Format::PrettyJson),
            "yaml" => Ok(Format::Yaml),
            "pretty-yaml" => Ok(Format::PrettyYaml),
            "text" => Ok(Format::Text),
            _ => {
                let all = Format::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown format: {s}. Expected one of {all}"))
            }
        }
    }
}

pub trait HasFormatConfig {
    fn format(&self) -> Option<Format>;
}
