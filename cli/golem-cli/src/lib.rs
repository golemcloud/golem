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

use clap_verbosity_flag::Verbosity;
use golem_common::tracing::directive;
use golem_common::tracing::directive::warn;
use shadow_rs::shadow;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

pub mod app;
pub mod auth;
pub mod command;
pub mod command_handler;
pub mod config;
pub mod context;
pub mod diagnose;
pub mod error;
pub mod fs;
pub mod fuzzy;
pub mod log;
pub mod model;
pub mod validation;
pub mod wasm_rpc_stubgen;

#[allow(unused)]
mod wasm_metadata;

#[cfg(test)]
test_r::enable!();

shadow!(build);

pub fn command_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or("golem-cli".to_string())
}

pub fn version() -> &'static str {
    if build::PKG_VERSION != "0.0.0" {
        build::PKG_VERSION
    } else {
        build::GIT_DESCRIBE_TAGS
    }
}

pub fn init_tracing(verbosity: Verbosity, pretty_mode: bool) {
    if let Some(level) = verbosity.tracing_level() {
        let subscriber = FmtSubscriber::builder();

        let mut filter = EnvFilter::builder().parse_lossy(level.as_str());
        for directive in directive::default_deps() {
            filter = filter.add_directive(directive);
        }
        filter = filter.add_directive(warn("opentelemetry_sdk"));
        filter = filter.add_directive(warn("opentelemetry"));
        filter = filter.add_directive(warn("poem"));

        if pretty_mode {
            let subscriber = subscriber
                .pretty()
                .with_max_level(level)
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .finish();

            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
        } else {
            let subscriber = subscriber
                .with_max_level(level)
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .finish();

            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
        };
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::command::GolemCliCommand;
    use clap::{ArgAction, Command, CommandFactory};

    #[test]
    fn dump_commands_v_1_2() {
        let command = GolemCliCommand::command();
        dump_command(0, &command);
    }

    fn dump_command(level: usize, command: &Command) {
        print!("{}{}", "\t".repeat(level), command.get_name());

        let aliases = command.get_aliases().collect::<Vec<_>>();
        if !aliases.is_empty() {
            print!(" ({})", aliases.join(", "));
        }

        let (positional, flag_args): (Vec<_>, Vec<_>) =
            command.get_arguments().partition(|arg| arg.is_positional());

        if !positional.is_empty() {
            for arg in positional {
                let id = arg.get_id().to_string().to_uppercase();
                if arg.is_required_set() && arg.get_default_values().is_empty() {
                    print!(" <{id}>");
                } else {
                    print!(" [{id}]");
                }
                if let ArgAction::Append = arg.get_action() {
                    print!("...")
                }
            }
        }

        println!();

        if !flag_args.is_empty() {
            print!("{}", "\t".repeat(level + 2));
            for arg in flag_args.clone() {
                print!(" --{}", arg.get_long().unwrap(),);
                arg.get_short().iter().for_each(|short| print!("({short})"));
            }
            println!()
        }

        let subcommand_level = level + 1;
        for subcommand in command.get_subcommands() {
            dump_command(subcommand_level, subcommand);
        }
    }
}
