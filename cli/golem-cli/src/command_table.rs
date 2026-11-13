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
//! permission parts that the server checks for it and their targets, whether
//! the command stays on the CLI, which of its arguments only the CLI may take,
//! which arguments every other surface must pass, and whether it is destructive.
//!
//! Every `golem agent` command has a row. The bulk controls outside
//! `golem agent` (`update-agents`, `redeploy-agents`, `component update-agents`,
//! `component redeploy-agents`, and `deploy --reset` or `--redeploy-agents`)
//! have no row on purpose, so that only the CLI offers them.

use CommandClass::*;
use Target::*;
use golem_common::model::card::AgentVerb as A;
use golem_common::model::card::{AgentVerb, FilesystemVerb, OplogVerb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClass {
    /// Inspects an execution and needs a read verb on the target.
    ManagementRead,
    /// Creates or changes an execution. The parts name the verbs that
    /// authorize it.
    ManagementControl,
    /// Invokes an agent, or follows or cancels its invocations, under
    /// invocation grants.
    InvocationGrant,
    /// Runs a command of a tool hosted by an agent, under the hosted-exec checks.
    Exec,
}

/// One permission check that the server makes for the command.
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
    /// The agent that the command names: by its agent ID, by its agent type,
    /// parameters and phantom ID (`agent repl-stream`), or, for
    /// `tool invoke --component`, the new owner that the invocation creates.
    Agent,
    /// Every agent of a component: of each component that `agent list` scans,
    /// or of the component that `tool invoke --component` names.
    ComponentAgents,
    /// The path argument, in the filesystem of the agent that the command's
    /// agent ID names.
    AgentPath,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandRow {
    /// The clap command path, such as `&["agent", "list"]`.
    pub path: &'static [&'static str],
    pub class: CommandClass,
    /// Every check that the server makes for the command today. When the
    /// checks depend on the arguments, the parts are their union.
    pub parts: &'static [Part],
    /// The command stays on the CLI: it needs a terminal, live input or live
    /// following, or no JSON output type carries its result (`ssh -c` has no
    /// JSON output type). The row's comment says which.
    pub interactive: bool,
    /// Clap argument IDs that only the CLI may take: they need a terminal,
    /// live input or a local server, write to stdout outside the JSON output,
    /// touch host files, or change agents beyond the command's target.
    pub cli_only_args: &'static [&'static str],
    /// Long flags that every surface other than the CLI always passes.
    pub fixed_args: &'static [&'static str],
    /// The command changes or removes state that cannot be restored, or
    /// asks for a confirmation.
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

pub const COMMAND_TABLE: &[CommandRow] = &[
    // ── golem agent ──────────────────────────────────────────────────────
    // The server checks `agent : invoke` when it creates an agent.
    CommandRow {
        path: &["agent", "new"],
        class: ManagementControl,
        parts: &[agent(A::Invoke, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // `agent : view` is for the live stream, which `--no-stream` turns off.
    // An argument value of `-` binds stdin to a stream parameter; the table
    // cannot express it, so a surface other than the CLI must refuse `-` values.
    // The post-deploy flags can update, redeploy or delete every agent of the
    // deployment without a prompt.
    CommandRow {
        path: &["agent", "invoke"],
        class: InvocationGrant,
        parts: &[agent(A::Invoke, Agent), agent(A::View, Agent)],
        interactive: false,
        cli_only_args: &[
            "update_agents",
            "redeploy_agents",
            "reset",
            "stdin_format",
            "stdout_format",
            "save_session",
            "resume_session",
            "takeover_session",
            "stream_no_log_level",
            "stream_no_timestamp",
            "logs_only",
        ],
        fixed_args: &["--no-stream"],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "get"],
        class: ManagementRead,
        parts: &[agent(A::View, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "delete"],
        class: ManagementControl,
        parts: &[agent(A::Delete, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: true,
    },
    CommandRow {
        path: &["agent", "list"],
        class: ManagementRead,
        parts: &[agent(A::View, ComponentAgents)],
        interactive: false,
        cli_only_args: &["refresh"],
        fixed_args: &[],
        destructive: false,
    },
    // Live following.
    CommandRow {
        path: &["agent", "stream"],
        class: InvocationGrant,
        parts: &[agent(A::View, Agent)],
        interactive: true,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // Live following, for Bridge SDK-based REPLs.
    CommandRow {
        path: &["agent", "repl-stream"],
        class: InvocationGrant,
        parts: &[agent(A::View, Agent)],
        interactive: true,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // `agent : view` reads the agent's metadata before and after the update.
    CommandRow {
        path: &["agent", "update"],
        class: ManagementControl,
        parts: &[agent(A::UpdateRevision, Agent), agent(A::View, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: true,
    },
    // Destructive although D9 does not list it: interrupting an ephemeral
    // agent asks for a confirmation, which `--yes` gives.
    CommandRow {
        path: &["agent", "interrupt"],
        class: ManagementControl,
        parts: &[agent(A::Interrupt, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: true,
    },
    CommandRow {
        path: &["agent", "resume"],
        class: ManagementControl,
        parts: &[agent(A::Resume, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // The catalog has no simulate-crash verb; the server checks interrupt.
    CommandRow {
        path: &["agent", "simulate-crash"],
        class: ManagementControl,
        parts: &[agent(A::Interrupt, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: true,
    },
    // `--html` writes a host file; the viewer flags serve or open it locally.
    CommandRow {
        path: &["agent", "oplog"],
        class: ManagementRead,
        parts: &[oplog(OplogVerb::Read)],
        interactive: false,
        cli_only_args: &["html", "serve", "port", "open", "no_open"],
        fixed_args: &[],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "revert"],
        class: ManagementControl,
        parts: &[agent(A::Revert, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: true,
    },
    CommandRow {
        path: &["agent", "cancel-invocation"],
        class: InvocationGrant,
        parts: &[agent(A::CancelInvocation, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "files"],
        class: ManagementRead,
        parts: &[
            filesystem(FilesystemVerb::Stat),
            filesystem(FilesystemVerb::List),
        ],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // The contents are not in the JSON output: the command writes them to a
    // host file, asking before it overwrites one, or as raw bytes to stdout
    // with `--output -`. It stays on the CLI until a command returns them.
    CommandRow {
        path: &["agent", "file-contents"],
        class: ManagementRead,
        parts: &[filesystem(FilesystemVerb::Read)],
        interactive: true,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "activate-plugin"],
        class: ManagementControl,
        parts: &[agent(A::ActivatePlugin, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "deactivate-plugin"],
        class: ManagementControl,
        parts: &[agent(A::DeactivatePlugin, Agent)],
        interactive: false,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // ── exec ─────────────────────────────────────────────────────────────
    // Without `-c`, a session; with `-c`, the script's raw stdout and stderr,
    // and no JSON output type. `agent : view` describes the tool and reads the
    // agent's metadata; looking up a running command also checks view.
    CommandRow {
        path: &["ssh"],
        class: Exec,
        parts: &[agent(A::View, Agent), agent(A::Invoke, Agent)],
        interactive: true,
        cli_only_args: &[],
        fixed_args: &[],
        destructive: false,
    },
    // `agent : view` describes the tool: on the agent with `--agent`, on the
    // component's agents with `--component`. `--lookup` checks only view; an
    // invocation checks invoke on the agent or on the new owner. The raw
    // output flags write to stdout or to host files.
    CommandRow {
        path: &["tool", "invoke"],
        class: Exec,
        parts: &[
            agent(A::View, Agent),
            agent(A::View, ComponentAgents),
            agent(A::Invoke, Agent),
        ],
        interactive: false,
        cli_only_args: &["stdin", "stdout", "output", "stderr", "stderr_output"],
        fixed_args: &[],
        destructive: false,
    },
];

#[cfg(test)]
mod test {
    use super::*;
    use crate::command::GolemCliCommand;
    use crate::model::cli_output::{command_output_schema_value, command_output_types};
    use clap::{Command, CommandFactory};
    use std::collections::BTreeMap;
    use test_r::test;

    /// Rows outside `golem agent` that the table must keep.
    const REQUIRED_PATHS: &[&[&str]] = &[&["ssh"], &["tool", "invoke"]];

    /// Non-interactive commands whose only JSON output is a finite stream of
    /// documents. `agent oplog` emits one `agent.oplog` document per entry and
    /// ends at the end of the oplog.
    const FINITE_STREAMS: &[&str] = &["agent oplog"];

    fn find<'a>(root: &'a Command, path: &[&str]) -> Option<&'a Command> {
        path.iter()
            .try_fold(root, |command, name| command.find_subcommand(name))
    }

    #[test]
    fn every_row_matches_the_clap_tree_and_the_output_schema() {
        let root = GolemCliCommand::command();
        let schema = command_output_schema_value().unwrap();
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
        let required = agent
            .get_subcommands()
            .map(|subcommand| format!("agent {}", subcommand.get_name()))
            .chain(REQUIRED_PATHS.iter().map(|path| path.join(" ")));
        for path in required {
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
            for arg in row.cli_only_args {
                if !command.get_arguments().any(|a| a.get_id() == *arg) {
                    problems.push(format!("`{path}` has no argument `{arg}`"));
                }
            }
            for flag in row.fixed_args {
                let known = flag.strip_prefix("--").is_some_and(|long| {
                    command.get_arguments().any(|a| {
                        a.get_long() == Some(long)
                            && !row.cli_only_args.contains(&a.get_id().as_str())
                    })
                });
                if !known {
                    problems.push(format!("`{path}` has no allowed flag `{flag}`"));
                }
            }
            if row.interactive {
                continue;
            }
            let output_types = command_output_types(&path).unwrap();
            if output_types.is_empty() {
                problems.push(format!(
                    "`{path}` is non-interactive but has no JSON output type"
                ));
            }
            let only_streams = output_types.iter().all(|output_type| {
                schema["definitions"][output_type]["x-golem-output-mode"] == "stream"
            });
            if !output_types.is_empty() && only_streams && !FINITE_STREAMS.contains(&path.as_str())
            {
                problems.push(format!(
                    "`{path}` is non-interactive but has only stream output"
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
