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

use clap::builder::PossibleValue;
use clap::{Arg, Command};
use serde_derive::{Deserialize, Serialize};

pub struct CliMetadataFilter {
    pub command_path_prefix_exclude: Vec<Vec<&'static str>>,
    pub arg_id_exclude: Vec<&'static str>,
    pub exclude_hidden: bool,
}

impl CliMetadataFilter {
    fn exclude_path(&self, path: &[String]) -> bool {
        self.command_path_prefix_exclude.iter().any(|prefix| {
            path.len() >= prefix.len()
                && path
                    .iter()
                    .zip(prefix.iter())
                    .all(|(segment, expected)| segment == expected)
        })
    }

    fn exclude_arg(&self, arg: &Arg) -> bool {
        self.arg_id_exclude.iter().any(|id| arg.get_id() == id)
            || self.exclude_hidden && arg.is_hide_set()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliCommandMetadata {
    pub path: Vec<String>,
    pub name: String,
    pub display_name: Option<String>,
    pub about: Option<String>,
    pub long_about: Option<String>,
    pub hidden: bool,
    pub visible_aliases: Vec<String>,
    pub args: Vec<CliArgMetadata>,
    pub subcommands: Vec<CliCommandMetadata>,
}

impl CliCommandMetadata {
    pub fn new(command: &Command) -> CliCommandMetadata {
        let mut path = Vec::new();
        Self::collect_command_metadata(&mut path, command, None)
    }

    pub fn new_filtered(command: &Command, filter: &CliMetadataFilter) -> CliCommandMetadata {
        let mut path = Vec::new();
        Self::collect_command_metadata(&mut path, command, Some(filter))
    }

    fn collect_command_metadata(
        path: &mut Vec<String>,
        command: &Command,
        filter: Option<&CliMetadataFilter>,
    ) -> CliCommandMetadata {
        let args = command
            .get_arguments()
            .filter_map(|arg| {
                (!filter.as_ref().map(|f| f.exclude_arg(arg)).unwrap_or(false))
                    .then(|| Self::collect_arg_metadata(arg))
            })
            .collect::<Vec<_>>();

        let subcommands = command
            .get_subcommands()
            .filter_map(|subcommand| {
                path.push(subcommand.get_name().to_string());
                let subcommand_metadata = (!filter
                    .as_ref()
                    .map(|f| f.exclude_path(path))
                    .unwrap_or(false))
                .then(|| Self::collect_command_metadata(path, subcommand, filter));
                path.pop();
                subcommand_metadata
            })
            .collect::<Vec<_>>();

        let metadata = CliCommandMetadata {
            path: path.clone(),
            name: command.get_name().to_string(),
            display_name: command.get_display_name().map(|s| s.to_string()),
            about: command.get_about().map(|s| s.to_string()),
            long_about: command.get_long_about().map(|s| s.to_string()),
            hidden: command.is_hide_set(),
            visible_aliases: command
                .get_visible_aliases()
                .map(|alias| alias.to_string())
                .collect(),
            args,
            subcommands,
        };

        metadata
    }

    fn collect_arg_metadata(arg: &Arg) -> CliArgMetadata {
        let long = arg
            .get_long_and_visible_aliases()
            .into_iter()
            .flatten()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        let short = arg
            .get_short_and_visible_aliases()
            .into_iter()
            .flatten()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();

        CliArgMetadata {
            id: arg.get_id().to_string(),
            help: arg.get_help().map(|s| s.to_string()),
            long_help: arg.get_long_help().map(|s| s.to_string()),
            value_names: arg
                .get_value_names()
                .map(|names| names.iter().map(|name| name.to_string()).collect())
                .unwrap_or_default(),
            value_hint: format!("{:?}", arg.get_value_hint()),
            possible_values: arg
                .get_possible_values()
                .iter()
                .map(Self::collect_possible_value_metadata)
                .collect(),
            action: format!("{:?}", arg.get_action()),
            num_args: arg.get_num_args().map(|range| format!("{range:?}")),
            is_positional: arg.is_positional(),
            is_required: arg.is_required_set(),
            is_global: arg.is_global_set(),
            is_hidden: arg.is_hide_set(),
            index: arg.get_index(),
            long,
            short,
            default_values: arg
                .get_default_values()
                .iter()
                .map(|value| value.to_string_lossy().to_string())
                .collect(),
            takes_value: arg.get_action().takes_values(),
        }
    }

    fn collect_possible_value_metadata(value: &PossibleValue) -> CliPossibleValueMetadata {
        CliPossibleValueMetadata {
            name: value.get_name().to_string(),
            help: value.get_help().map(|s| s.to_string()),
            hidden: value.is_hide_set(),
            aliases: value
                .get_name_and_aliases()
                .filter(|alias| *alias != value.get_name())
                .map(|alias| alias.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliArgMetadata {
    pub id: String,
    pub help: Option<String>,
    pub long_help: Option<String>,
    pub value_names: Vec<String>,
    pub value_hint: String,
    pub possible_values: Vec<CliPossibleValueMetadata>,
    pub action: String,
    pub num_args: Option<String>,
    pub is_positional: bool,
    pub is_required: bool,
    pub is_global: bool,
    pub is_hidden: bool,
    pub index: Option<usize>,
    pub long: Vec<String>,
    pub short: Vec<String>,
    pub default_values: Vec<String>,
    pub takes_value: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliPossibleValueMetadata {
    pub name: String,
    pub help: Option<String>,
    pub hidden: bool,
    pub aliases: Vec<String>,
}
