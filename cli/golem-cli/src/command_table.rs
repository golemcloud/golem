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

//! The management commands, keyed by their clap command path.
//!
//! The clap tree defines each command's arguments, and the command output
//! schema defines its JSON output (`crate::model::cli_output::command_output_types`).
//! This table adds what neither of them says: the command's class, the
//! permission parts that authorize it and their targets, and whether it is
//! interactive or destructive.
//!
//! `golem mcp serve` offers one MCP tool per non-interactive row and leaves out
//! the row's `interactive_args`. The tool of a destructive row takes `confirm`,
//! which becomes `--yes`.

use CommandClass::*;
use Target::*;
use golem_common::model::card::AgentVerb as A;
use golem_common::model::card::{AgentVerb, FilesystemVerb, OplogVerb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClass {
    /// Inspects an execution and needs a read verb on the target.
    ManagementRead,
    /// Creates or changes an execution and needs a control verb on the target.
    ManagementControl,
    /// Invokes, or reads or cancels an invocation, under invocation grants.
    InvocationGrant,
    /// Runs a command of a tool hosted by an agent, under the hosted-exec checks.
    Exec,
}

/// One permission check that authorizes the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub verb: PartVerb,
    pub target: Target,
}

/// The verb of a part, from the card model's class catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartVerb {
    Agent(AgentVerb),
    Oplog(OplogVerb),
    Filesystem(FilesystemVerb),
}

/// What a part addresses, given by the command's arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The agent that the command's agent ID names.
    Agent,
    /// Each agent that the command lists: in the environment, or in the
    /// component or agent type that the command selects.
    ListedAgents,
    /// The path argument, in the filesystem of the agent that the command's
    /// agent ID names.
    AgentPath,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandRow {
    /// The clap command path, such as `&["agent", "list"]`.
    pub path: &'static [&'static str],
    pub class: CommandClass,
    pub parts: &'static [Part],
    /// Every use of the command needs a terminal, live input or live following.
    pub interactive: bool,
    /// Clap argument IDs that need a terminal, live input, live following or a
    /// local server, on a command that is otherwise non-interactive.
    pub interactive_args: &'static [&'static str],
    /// The command changes or removes state that cannot be restored.
    pub destructive: bool,
}

const fn agent(verb: AgentVerb, target: Target) -> Part {
    Part {
        verb: PartVerb::Agent(verb),
        target,
    }
}

const fn oplog(verb: OplogVerb) -> Part {
    Part {
        verb: PartVerb::Oplog(verb),
        target: Target::Agent,
    }
}

const fn filesystem(verb: FilesystemVerb) -> Part {
    Part {
        verb: PartVerb::Filesystem(verb),
        target: Target::AgentPath,
    }
}

#[rustfmt::skip]
pub const COMMAND_TABLE: &[CommandRow] = &[
    // ── golem agent ──────────────────────────────────────────────────────
    CommandRow { path: &["agent", "new"],               class: ManagementControl, parts: &[agent(A::Invoke, Agent)],            interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "invoke"],            class: InvocationGrant,   parts: &[agent(A::Invoke, Agent)],            interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "get"],               class: ManagementRead,    parts: &[agent(A::View, Agent)],              interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "delete"],            class: ManagementControl, parts: &[agent(A::Delete, Agent)],            interactive: false, interactive_args: &[], destructive: true },
    CommandRow { path: &["agent", "list"],              class: ManagementRead,    parts: &[agent(A::View, ListedAgents)],       interactive: false, interactive_args: &["refresh"], destructive: false },
    CommandRow { path: &["agent", "stream"],            class: InvocationGrant,   parts: &[agent(A::View, Agent)],              interactive: true,  interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "repl-stream"],       class: InvocationGrant,   parts: &[agent(A::View, Agent)],              interactive: true,  interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "update"],            class: ManagementControl, parts: &[agent(A::UpdateRevision, Agent)],    interactive: false, interactive_args: &[], destructive: true },
    CommandRow { path: &["agent", "interrupt"],         class: ManagementControl, parts: &[agent(A::Interrupt, Agent)],         interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "resume"],            class: ManagementControl, parts: &[agent(A::Resume, Agent)],            interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "simulate-crash"],    class: ManagementControl, parts: &[agent(A::Interrupt, Agent)],         interactive: false, interactive_args: &[], destructive: true },
    CommandRow { path: &["agent", "oplog"],             class: ManagementRead,    parts: &[oplog(OplogVerb::Read)],             interactive: false, interactive_args: &["serve", "port", "open", "no_open"], destructive: false },
    CommandRow { path: &["agent", "revert"],            class: ManagementControl, parts: &[agent(A::Revert, Agent)],            interactive: false, interactive_args: &[], destructive: true },
    CommandRow { path: &["agent", "cancel-invocation"], class: ManagementControl, parts: &[agent(A::CancelInvocation, Agent)],  interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "files"],             class: ManagementRead,    parts: &[filesystem(FilesystemVerb::Stat), filesystem(FilesystemVerb::List)], interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "file-contents"],     class: ManagementRead,    parts: &[filesystem(FilesystemVerb::Read)],   interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "activate-plugin"],   class: ManagementControl, parts: &[agent(A::ActivatePlugin, Agent)],    interactive: false, interactive_args: &[], destructive: false },
    CommandRow { path: &["agent", "deactivate-plugin"], class: ManagementControl, parts: &[agent(A::DeactivatePlugin, Agent)],  interactive: false, interactive_args: &[], destructive: false },

    // ── exec ─────────────────────────────────────────────────────────────
    CommandRow { path: &["ssh"],                        class: Exec,              parts: &[agent(A::Invoke, Agent)],            interactive: true,  interactive_args: &[], destructive: false },
    CommandRow { path: &["tool", "invoke"],             class: Exec,              parts: &[agent(A::Invoke, Agent)],            interactive: false, interactive_args: &["stdin"], destructive: false },
];

#[cfg(test)]
mod test {
    use super::*;
    use crate::command::GolemCliCommand;
    use crate::model::cli_output::command_output_types;
    use clap::{Command, CommandFactory};
    use std::collections::BTreeMap;
    use test_r::test;

    fn find<'a>(root: &'a Command, path: &[&str]) -> Option<&'a Command> {
        path.iter()
            .try_fold(root, |command, name| command.find_subcommand(name))
    }

    #[test]
    fn every_row_matches_the_clap_tree_and_the_output_schema() {
        let root = GolemCliCommand::command();
        let mut problems = Vec::<String>::new();

        let mut rows_per_path = BTreeMap::<String, usize>::new();
        for row in COMMAND_TABLE {
            *rows_per_path.entry(row.path.join(" ")).or_default() += 1;
        }
        for (path, count) in &rows_per_path {
            if *count > 1 {
                problems.push(format!("`{path}` has {count} rows"));
            }
        }

        let agent = find(&root, &["agent"]).expect("golem agent exists");
        for subcommand in agent.get_subcommands() {
            let path = format!("agent {}", subcommand.get_name());
            if !rows_per_path.contains_key(&path) {
                problems.push(format!("`{path}` has no row"));
            }
        }

        for row in COMMAND_TABLE {
            let path = row.path.join(" ");
            let Some(command) = find(&root, row.path) else {
                problems.push(format!("`{path}` is not a command"));
                continue;
            };
            for arg in row.interactive_args {
                if !command.get_arguments().any(|a| a.get_id() == *arg) {
                    problems.push(format!("`{path}` has no argument `{arg}`"));
                }
            }
            if !row.interactive && command_output_types(&path).unwrap().is_empty() {
                problems.push(format!(
                    "`{path}` is non-interactive but has no JSON output type"
                ));
            }
        }

        assert!(
            problems.is_empty(),
            "COMMAND_TABLE does not match the CLI:\n  {}",
            problems.join("\n  ")
        );
    }
}
