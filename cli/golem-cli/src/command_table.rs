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
//! This table adds what neither of them says: for each mode of the command,
//! its class and the server requests that it makes, each with the parts of its
//! one operation and their targets; whether the command stays on the CLI,
//! which of its arguments only the CLI may take, which arguments every other
//! surface must pass, which integer arguments every other surface caps, which
//! arguments read stdin when their value is `-`, and whether it is destructive.
//!
//! Every server request is one operation and gets one decision: the server
//! allows it only when it allows every part of it (the card model's D27). A
//! mode lists its requests in the order that the command makes them. A
//! request is not a part of the next one: the `agent : view` that the CLI
//! needs to read an agent's metadata authorizes no control or invocation
//! after it.
//!
//! The parts are the classification of 7.5 Rule 6, which the server answers
//! only on the 2.0 card model (GOL-1289). Until then the server answers each
//! request with its 1.6 check, and a part whose verb the class catalog does
//! not have yet is a [`PendingVerb`].
//!
//! Every `golem agent` command has a row. The bulk controls outside
//! `golem agent` (`update-agents`, `redeploy-agents`, `component update-agents`,
//! `component redeploy-agents`, and `deploy --reset` or `--redeploy-agents`)
//! have no row on purpose, so that only the CLI offers them.

use CommandClass::*;
use Resource as R;
use Stage::*;
use Target::*;
use golem_common::model::card::AgentVerb as A;
use golem_common::model::card::{AgentVerb, FilesystemVerb, OplogVerb, ToolVerb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClass {
    /// Inspects an execution and needs a read verb on the target.
    ManagementRead,
    /// Creates or changes an execution. The parts name the verbs that
    /// authorize it.
    ManagementControl,
    /// Invokes an agent or a tool, or reads, feeds or cancels an invocation,
    /// under invocation grants.
    InvocationGrant,
    /// Runs a command of a tool hosted by an agent, under the hosted-exec
    /// checks: `agent : exec` on the agent and `tool : invoke` on the tool.
    Exec,
}

/// One way to run a command, chosen by its arguments.
#[derive(Debug, Clone, Copy)]
pub struct Mode {
    /// The mode's name, unique in its row. The integration tests key their
    /// adapters by the row's path and this name.
    pub name: &'static str,
    /// The arguments that select the mode; the mode applies when all of them
    /// are given. Empty for the row's default mode. When several modes apply,
    /// the one listed last is the mode.
    pub selected_by: &'static [Selector],
    pub class: CommandClass,
    /// The server requests that the mode makes, in order.
    pub requests: &'static [Request],
}

/// An argument that selects a mode or conditions a request: given at all,
/// or given with one of these values among its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selector {
    /// The clap argument ID.
    pub arg: &'static str,
    /// Empty when the argument only has to be given.
    pub values: &'static [&'static str],
}

/// One server request of a mode, which is one operation and one decision.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub stage: Stage,
    pub when: When,
    /// The parts of the request's operation. The server allows the
    /// operation only when it allows every part. The parts whose targets are
    /// 5.3 Rule 21 kinds come first.
    pub parts: &'static [Part],
    /// Parts that the operation also has when its target needs them, which
    /// the server decides from the target, never from the request.
    pub extra: Option<Extra>,
    /// What the never-live mark of the target agent (GOL-1420) changes.
    pub marked: Marked,
}

/// Parts that an operation also has when its target needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extra {
    pub when: TargetCondition,
    pub parts: &'static [Part],
}

/// A condition on the target that the server reads from its definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetCondition {
    /// The tool reads static `File` config fields, which needs the tool-wide
    /// `invoke : *` of 5.3 beside the grant for the command. The tool
    /// admission (GOL-1086) supplies the check.
    ToolReadsStaticFileConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// A read that the CLI makes to build the operation: the agent's
    /// metadata, for its component revision, or a tool's definition. It
    /// authorizes nothing that follows.
    Prepare,
    /// The command's own operation. Every mode has exactly one.
    Operation,
    /// Each input item that the command sends to the invocation it started.
    Input,
    /// A read of output or state next to the operation: an output connection
    /// that the command opens before it, the target's state, a result, or an
    /// entry that a page left out. The mode lists it where the command makes
    /// it.
    Follow,
    /// The cancellation of the invocation that the command started.
    Cancel,
}

/// When the command makes the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    /// Only when one of these arguments is given.
    With(&'static [Selector]),
    /// Only when none of these arguments is given.
    Without(&'static [Selector]),
    /// Only when an argument of the row's `stdin_marker_args` is `-`.
    StdinMarker,
    /// Only when the user stops a command of a tool that can stop one: the
    /// stop goes through the command's stdin.
    StoppedThroughStdin,
    /// Only when the user stops the command, or the CLI stops waiting for
    /// it.
    Stopped,
    /// Only when the command was stopped and its session ended without the
    /// result, or the cancellation found it finished: the CLI then looks the
    /// result up.
    StoppedWithoutResult,
    /// Only when the outcome of the command is unknown.
    OutcomeUnknown,
}

/// What the never-live mark of the target agent changes in a request. The
/// server reads the mark from the agent's metadata, never from the request
/// (D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marked {
    /// The mark changes nothing.
    Same,
    /// On an agent with the mark, the request has these parts instead. This
    /// is a choice of the target, not a second grant that also allows it.
    Instead(&'static [Part]),
}

/// What the never-live mark does to a manual update.
///
/// On an agent with the persisted never-live mark, a manual update is the
/// restricted snapshot-migration rehearsal of D12 (decision 2 of the GOL-1403
/// plan): the source revision of the fork saves its state, the target
/// revision loads it, and the rehearsal stops. It validates the load only:
/// no invocation runs after the load, no fresh external operation runs, and
/// the source does not change. `agent : replay` on the fork authorizes it.
///
/// This is the grant, not the admission. The server admits the rehearsal
/// only after the executor review and tests settle its host-call safety,
/// exact idle and restart behavior, and the supported cases; the issue that
/// builds it, and its gate relative to the replay PR, are not agreed yet.
/// Nothing here builds the rehearsal or refuses a marked manual update.
pub const MANUAL_UPDATE_OF_A_MARKED_AGENT: Marked = Marked::Instead(&[REPLAY]);

/// One part of an operation: a class and verb, with the target and resource
/// that it checks.
///
/// GOL-1270 supplies the shared `OperationPart` that the server builds from a
/// part, with the resolved target in it. A part here names where the command
/// takes the target and resource from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub verb: PartVerb,
    pub target: Target,
    pub resource: Resource,
}

/// The class and verb of a part, from the card model's class catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartVerb {
    Agent(AgentVerb),
    Oplog(OplogVerb),
    Filesystem(FilesystemVerb),
    Tool(ToolVerb),
    Pending(PendingVerb),
}

/// A verb of the 2.0 class catalog that the catalog in this tree does not
/// have yet. GOL-1276 adds each of them; the part then names the catalog's
/// verb, and the table test fails until it does. No check uses these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingVerb {
    AgentCreate,
    AgentExec,
    AgentReplay,
    AgentSimulateCrash,
    AgentActivateProcessor,
    AgentDeactivateProcessor,
    ToolView,
    InvocationRead,
    InvocationCancel,
    InvocationSend,
}

impl PendingVerb {
    pub const ALL: &'static [PendingVerb] = &[
        PendingVerb::AgentCreate,
        PendingVerb::AgentExec,
        PendingVerb::AgentReplay,
        PendingVerb::AgentSimulateCrash,
        PendingVerb::AgentActivateProcessor,
        PendingVerb::AgentDeactivateProcessor,
        PendingVerb::ToolView,
        PendingVerb::InvocationRead,
        PendingVerb::InvocationCancel,
        PendingVerb::InvocationSend,
    ];

    /// The class and the verb, as the grant text spells them.
    pub const fn spelling(self) -> (&'static str, &'static str) {
        match self {
            PendingVerb::AgentCreate => ("agent", "create"),
            PendingVerb::AgentExec => ("agent", "exec"),
            PendingVerb::AgentReplay => ("agent", "replay"),
            PendingVerb::AgentSimulateCrash => ("agent", "simulate-crash"),
            PendingVerb::AgentActivateProcessor => ("agent", "activate-processor"),
            PendingVerb::AgentDeactivateProcessor => ("agent", "deactivate-processor"),
            PendingVerb::ToolView => ("tool", "view"),
            PendingVerb::InvocationRead => ("invocation", "read"),
            PendingVerb::InvocationCancel => ("invocation", "cancel"),
            PendingVerb::InvocationSend => ("invocation", "send"),
        }
    }
}

/// What a part addresses, from the command's arguments. The server fills the
/// target from trusted metadata, never from an owner that the client
/// supplies; GOL-1270 supplies the target type. The kind of the target
/// decides the shape of a denial (D27): a denied part on an agent or an
/// invocation looks like an unknown ID (5.3 Rule 21), and only a part on a
/// tool by name can be a `403`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The existing agent that the command names, with its environment,
    /// type, ID and fingerprint from its metadata (D25, D26).
    Agent,
    /// The agent that the command creates, in its type and environment.
    /// GOL-1296 owns the admission of a creation and its denial.
    NewAgent,
    /// The agents that a listing covers. GOL-1404 owns the scope and the
    /// filter of each row.
    AgentScope,
    /// An invocation, with the owner that its record holds: an agent, or a
    /// tool for a standalone tool invocation.
    Invocation(InvocationSelector),
    /// A tool by name. Not a 5.3 Rule 21 kind.
    Tool,
}

/// Where the command takes the invocation from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationSelector {
    /// The invocation that the command's idempotency key names.
    IdempotencyKey,
    /// The invocation that the server accepted for the command's start.
    Accepted,
    /// The invocation whose ID the command reads from stdin
    /// (`agent repl-stream`). The line selects the output to follow; it is
    /// not input to the guest.
    Stdin,
    /// The accepted invocation of a saved session (`--resume-session`,
    /// `--takeover-session`).
    Session,
    /// Each invocation whose output the stream exposes.
    EachExposed,
}

/// The resource side of a part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    None,
    /// The method that the command invokes.
    Method,
    /// The oplog range or page that the command reads.
    OplogRange,
    /// One oplog entry, by its index.
    OplogEntry,
    /// The whole retained oplog, which a search scans.
    OplogAll,
    /// The oplog index that a revert cuts at, after the server resolves a
    /// count of invocations to it.
    RevertCutoff,
    /// The path argument, in the target agent's filesystem.
    Path,
    /// The oplog processor that the command names.
    Processor,
    /// The command path and arguments of the tool invocation.
    ToolCommand,
    /// Every command of the tool.
    AnyToolCommand,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandRow {
    /// The clap command path, such as `&["agent", "list"]`.
    pub path: &'static [&'static str],
    pub modes: &'static [Mode],
    /// The command stays on the CLI: it needs a terminal, live input or live
    /// following, or no JSON output type carries its result (`ssh -c` has no
    /// JSON output type). The row's comment says which.
    pub interactive: bool,
    /// Clap argument IDs that only the CLI may take: they need a terminal,
    /// live input or a local server, write to stdout outside the JSON output,
    /// touch host files, or change agents beyond the command's target.
    pub cli_only_args: &'static [&'static str],
    /// The command's output types that only its CLI-only arguments select, so that no surface
    /// other than the CLI returns them.
    pub cli_only_output_types: &'static [&'static str],
    /// Long flags that every surface other than the CLI always passes.
    pub fixed_args: &'static [&'static str],
    /// Clap argument IDs of integer arguments, with the largest value that every surface other
    /// than the CLI accepts for them.
    pub capped_args: &'static [(&'static str, u64)],
    /// Clap argument IDs whose value `-` reads the process's stdin. A surface
    /// other than the CLI must refuse `-` as their value.
    pub stdin_marker_args: &'static [&'static str],
    /// The command changes or removes state that cannot be restored, or
    /// asks for a confirmation.
    pub destructive: bool,
}

impl CommandRow {
    /// Every mode of the command is a management read.
    pub fn reads_only(&self) -> bool {
        self.modes
            .iter()
            .all(|mode| mode.class == CommandClass::ManagementRead)
    }
}

const fn part(verb: PartVerb, target: Target, resource: Resource) -> Part {
    Part {
        verb,
        target,
        resource,
    }
}

const fn agent(verb: AgentVerb, resource: Resource) -> Part {
    part(PartVerb::Agent(verb), Agent, resource)
}

const fn pending(verb: PendingVerb, target: Target, resource: Resource) -> Part {
    part(PartVerb::Pending(verb), target, resource)
}

const fn invocation(verb: PendingVerb, selector: InvocationSelector) -> Part {
    pending(verb, Invocation(selector), R::None)
}

const fn oplog(resource: Resource) -> Part {
    part(PartVerb::Oplog(OplogVerb::Read), Agent, resource)
}

const fn filesystem(verb: FilesystemVerb) -> Part {
    part(PartVerb::Filesystem(verb), Agent, R::Path)
}

const fn request(stage: Stage, parts: &'static [Part]) -> Request {
    Request {
        stage,
        when: When::Always,
        parts,
        extra: None,
        marked: Marked::Same,
    }
}

const fn conditional(stage: Stage, when: When, parts: &'static [Part]) -> Request {
    Request {
        stage,
        when,
        parts,
        extra: None,
        marked: Marked::Same,
    }
}

const fn operation(parts: &'static [Part]) -> Request {
    request(Operation, parts)
}

/// A control of the named agent whose verb the never-live mark changes.
const fn control_of_marked(parts: &'static [Part], marked: Marked) -> Request {
    Request {
        stage: Operation,
        when: When::Always,
        parts,
        extra: None,
        marked,
    }
}

/// The modes of a row with one mode, which no argument selects.
macro_rules! one_mode {
    ($name:literal, $class:expr, [$($request:expr),* $(,)?]) => {
        &[Mode {
            name: $name,
            selected_by: &[],
            class: $class,
            requests: &[$($request),*],
        }]
    };
}

const fn arg(arg: &'static str) -> Selector {
    Selector { arg, values: &[] }
}

const fn arg_is(arg: &'static str, values: &'static [&'static str]) -> Selector {
    Selector { arg, values }
}

const VIEW: Part = agent(A::View, R::None);
/// The CLI's metadata read of the named agent, for its component revision
/// (`component_by_agent_id_match`). The CLI goes on with the component's
/// current revision when the agent looks unknown, so a denial of this read
/// does not stop the command.
const PREPARE_AGENT: Request = request(Prepare, &[VIEW]);
const REPLAY: Part = pending(PendingVerb::AgentReplay, Agent, R::None);
const TOOL_VIEW: Part = pending(PendingVerb::ToolView, Tool, R::None);
/// Describing a tool bound to an agent. The agent part comes first, so that
/// an agent the caller may not see looks unknown before the tool is named.
const DESCRIBE_HOSTED: &[Part] = &[VIEW, TOOL_VIEW];
const DESCRIBE_STANDALONE: &[Part] = &[TOOL_VIEW];
/// The flags for which the tool's argument parser answers with the tool's
/// help (`golem_schema::schema::tool::argv`).
const HELP: &[&str] = &["--help", "-h"];
/// A tool that reads static `File` config fields also needs the tool-wide
/// `invoke : *` (5.3).
const STATIC_FILE_CONFIG: Option<Extra> = Some(Extra {
    when: TargetCondition::ToolReadsStaticFileConfig,
    parts: &[part(
        PartVerb::Tool(ToolVerb::Invoke),
        Tool,
        R::AnyToolCommand,
    )],
});
const fn tool_start(parts: &'static [Part]) -> Request {
    Request {
        stage: Operation,
        when: When::Always,
        parts,
        extra: STATIC_FILE_CONFIG,
        marked: Marked::Same,
    }
}
const EXEC: &[Part] = &[
    pending(PendingVerb::AgentExec, Agent, R::None),
    part(PartVerb::Tool(ToolVerb::Invoke), Tool, R::ToolCommand),
];
const STANDALONE_INVOKE: &[Part] = &[part(PartVerb::Tool(ToolVerb::Invoke), Tool, R::ToolCommand)];
const SEND_TO_ACCEPTED: Part =
    invocation(PendingVerb::InvocationSend, InvocationSelector::Accepted);
const READ_ACCEPTED: Part = invocation(PendingVerb::InvocationRead, InvocationSelector::Accepted);
const CANCEL_ACCEPTED: Part =
    invocation(PendingVerb::InvocationCancel, InvocationSelector::Accepted);
/// The live output connection that `agent invoke` opens to the agent before
/// it dispatches, so that it names no invocation yet and can expose the
/// output of other invocations of the agent. Raw stdout opens none.
const OUTPUT_CONNECTION: Request = conditional(
    Follow,
    When::Without(&[arg("no_stream"), arg_is("stdout_format", &["raw"])]),
    &[invocation(
        PendingVerb::InvocationRead,
        InvocationSelector::EachExposed,
    )],
);
const FOLLOW_ENTRIES: Request = conditional(
    Follow,
    When::Without(&[arg("omit_payloads")]),
    &[oplog(R::OplogEntry)],
);
const AWAIT_UPDATE: Request = conditional(Follow, When::With(&[arg("await")]), &[VIEW]);
/// The requests of an `ssh` command: the description of its tool, the start,
/// the stop through stdin, the read of the agent's state, and, when the user
/// or the CLI stops waiting, the cancellation and the lookup of a result
/// that finished meanwhile. `state_read` says when the command reads the
/// agent's state.
const fn ssh_command(state_read: When) -> [Request; 6] {
    [
        request(Prepare, DESCRIBE_HOSTED),
        tool_start(EXEC),
        conditional(Input, When::StoppedThroughStdin, &[SEND_TO_ACCEPTED]),
        conditional(Follow, state_read, &[VIEW]),
        conditional(Cancel, When::Stopped, &[CANCEL_ACCEPTED]),
        conditional(Follow, When::StoppedWithoutResult, &[READ_ACCEPTED]),
    ]
}
/// A session reads the agent's state next to each command.
const SSH_SESSION_COMMAND: &[Request] = &ssh_command(When::Always);
/// A script reads it only to report a command whose outcome is unknown.
const SSH_SCRIPT_COMMAND: &[Request] = &ssh_command(When::OutcomeUnknown);
const RESUME_SESSION: &[Request] = &[
    PREPARE_AGENT,
    OUTPUT_CONNECTION,
    operation(&[invocation(
        PendingVerb::InvocationRead,
        InvocationSelector::Session,
    )]),
    conditional(
        Input,
        When::StdinMarker,
        &[invocation(
            PendingVerb::InvocationSend,
            InvocationSelector::Session,
        )],
    ),
];

pub const COMMAND_TABLE: &[CommandRow] = &[
    // ── golem agent ──────────────────────────────────────────────────────
    // A fresh creation is `agent : create`, not `invoke`; GOL-1296 owns its
    // admission and the creation card. The CLI reads the metadata of the
    // agent it creates and goes on when it does not exist.
    CommandRow {
        path: &["agent", "new"],
        modes: one_mode!(
            "create",
            ManagementControl,
            [
                PREPARE_AGENT,
                operation(&[pending(PendingVerb::AgentCreate, NewAgent, R::None)]),
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // An `arguments` value of `-` binds stdin to a direct stream parameter.
    // The post-deploy flags can update, redeploy or delete every agent of the
    // deployment without a prompt. The upsert, schedule and request admission
    // checks of the invocation stay. The output connection is not opened for
    // `--stdout-format raw`.
    CommandRow {
        path: &["agent", "invoke"],
        modes: &[
            // Through the invocation session.
            Mode {
                name: "start",
                selected_by: &[],
                class: InvocationGrant,
                requests: &[
                    PREPARE_AGENT,
                    OUTPUT_CONNECTION,
                    operation(&[agent(A::Invoke, R::Method)]),
                    conditional(Input, When::StdinMarker, &[SEND_TO_ACCEPTED]),
                ],
            },
            // Through REST, which schedules the invocation.
            Mode {
                name: "trigger",
                selected_by: &[arg("trigger")],
                class: InvocationGrant,
                requests: &[
                    PREPARE_AGENT,
                    OUTPUT_CONNECTION,
                    operation(&[agent(A::Invoke, R::Method)]),
                ],
            },
            Mode {
                name: "resume-session",
                selected_by: &[arg("resume_session")],
                class: InvocationGrant,
                requests: RESUME_SESSION,
            },
            Mode {
                name: "takeover-session",
                selected_by: &[arg("takeover_session")],
                class: InvocationGrant,
                requests: RESUME_SESSION,
            },
        ],
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
        cli_only_output_types: &[],
        fixed_args: &["--no-stream"],
        capped_args: &[],
        stdin_marker_args: &["arguments"],
        destructive: false,
    },
    CommandRow {
        path: &["agent", "get"],
        modes: one_mode!("get", ManagementRead, [PREPARE_AGENT, operation(&[VIEW])]),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // Deleting a sandbox fork is `agent : replay` (D12).
    CommandRow {
        path: &["agent", "delete"],
        modes: one_mode!(
            "delete",
            ManagementControl,
            [
                PREPARE_AGENT,
                control_of_marked(&[agent(A::Delete, R::None)], Marked::Instead(&[REPLAY])),
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: true,
    },
    CommandRow {
        path: &["agent", "list"],
        modes: one_mode!(
            "list",
            ManagementRead,
            [operation(&[part(
                PartVerb::Agent(A::View),
                AgentScope,
                R::None
            )])]
        ),
        interactive: false,
        cli_only_args: &["refresh"],
        cli_only_output_types: &[],
        fixed_args: &["--page"],
        capped_args: &[("max_count", 50)],
        stdin_marker_args: &[],
        destructive: false,
    },
    // Live following. `agent : view` reads no invocation output.
    CommandRow {
        path: &["agent", "stream"],
        modes: one_mode!(
            "follow",
            InvocationGrant,
            [
                PREPARE_AGENT,
                operation(&[invocation(
                    PendingVerb::InvocationRead,
                    InvocationSelector::EachExposed
                )]),
            ]
        ),
        interactive: true,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // Live following, for Bridge SDK-based REPLs. It starts no work.
    CommandRow {
        path: &["agent", "repl-stream"],
        modes: one_mode!(
            "follow",
            InvocationGrant,
            [operation(&[invocation(
                PendingVerb::InvocationRead,
                InvocationSelector::Stdin
            )])]
        ),
        interactive: true,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // The automatic update of a sandbox fork is `agent : replay` (D12), and so
    // is its manual update, as the restricted snapshot-migration rehearsal:
    // see `MANUAL_UPDATE_OF_A_MARKED_AGENT` for what of it is still open.
    CommandRow {
        path: &["agent", "update"],
        modes: &[
            Mode {
                name: "automatic",
                selected_by: &[],
                class: ManagementControl,
                requests: &[
                    PREPARE_AGENT,
                    control_of_marked(
                        &[agent(A::UpdateRevision, R::None)],
                        Marked::Instead(&[REPLAY]),
                    ),
                    AWAIT_UPDATE,
                ],
            },
            Mode {
                name: "manual",
                selected_by: &[arg_is("mode", &["manual"])],
                class: ManagementControl,
                requests: &[
                    PREPARE_AGENT,
                    control_of_marked(
                        &[agent(A::UpdateRevision, R::None)],
                        MANUAL_UPDATE_OF_A_MARKED_AGENT,
                    ),
                    AWAIT_UPDATE,
                ],
            },
        ],
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: true,
    },
    // Destructive although D9 does not list it: interrupting an ephemeral
    // agent asks for a confirmation, which `--yes` gives. Interruption is not
    // the cancellation of an invocation.
    CommandRow {
        path: &["agent", "interrupt"],
        modes: one_mode!(
            "interrupt",
            ManagementControl,
            [PREPARE_AGENT, operation(&[agent(A::Interrupt, R::None)])]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: true,
    },
    CommandRow {
        path: &["agent", "resume"],
        modes: one_mode!(
            "resume",
            ManagementControl,
            [PREPARE_AGENT, operation(&[agent(A::Resume, R::None)])]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // The server selects the verb from the request's `recover_immediately`.
    CommandRow {
        path: &["agent", "simulate-crash"],
        modes: one_mode!(
            "simulate-crash",
            ManagementControl,
            [
                PREPARE_AGENT,
                operation(&[pending(PendingVerb::AgentSimulateCrash, Agent, R::None)]),
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: true,
    },
    // `--html` writes a host file; the viewer flags serve or open it locally. The obfuscated
    // export reads the whole oplog, and `--page` cannot be combined with `--obfuscate`.
    // A page leaves large payloads out, and the CLI reads each of them as one
    // entry unless `--omit-payloads` is given.
    CommandRow {
        path: &["agent", "oplog"],
        modes: &[
            Mode {
                name: "page",
                selected_by: &[],
                class: ManagementRead,
                requests: &[
                    PREPARE_AGENT,
                    operation(&[oplog(R::OplogRange)]),
                    FOLLOW_ENTRIES,
                ],
            },
            Mode {
                name: "search",
                selected_by: &[arg("query")],
                class: ManagementRead,
                requests: &[
                    PREPARE_AGENT,
                    operation(&[oplog(R::OplogAll)]),
                    FOLLOW_ENTRIES,
                ],
            },
        ],
        interactive: false,
        cli_only_args: &[
            "html",
            "serve",
            "port",
            "open",
            "no_open",
            "obfuscate",
            "keep_scalars",
            "keep_shape",
            "keep_error_text",
            "max_entry_bytes",
        ],
        cli_only_output_types: &[
            "agent.oplog-export-header",
            "agent.oplog-export-entry",
            "agent.oplog-export-coverage",
        ],
        fixed_args: &["--page"],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // The server resolves a count of invocations to its cutoff before the
    // decision, and reveals neither on a denial.
    CommandRow {
        path: &["agent", "revert"],
        modes: &[
            Mode {
                name: "to-oplog-index",
                selected_by: &[arg("last_oplog_index")],
                class: ManagementControl,
                requests: &[
                    PREPARE_AGENT,
                    operation(&[agent(A::Revert, R::RevertCutoff)]),
                ],
            },
            Mode {
                name: "by-invocation-count",
                selected_by: &[arg("number_of_invocations")],
                class: ManagementControl,
                requests: &[
                    PREPARE_AGENT,
                    operation(&[agent(A::Revert, R::RevertCutoff)]),
                ],
            },
        ],
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: true,
    },
    // `invocation : cancel` replaces `agent : cancel-invocation`, with no
    // alias.
    CommandRow {
        path: &["agent", "cancel-invocation"],
        modes: one_mode!(
            "cancel",
            InvocationGrant,
            [
                PREPARE_AGENT,
                operation(&[invocation(
                    PendingVerb::InvocationCancel,
                    InvocationSelector::IdempotencyKey
                )]),
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // The reader is the recipient of the filesystem grant. The `stat` that
    // the server makes today to list a path is not a part of the command.
    CommandRow {
        path: &["agent", "files"],
        modes: one_mode!(
            "list",
            ManagementRead,
            [
                PREPARE_AGENT,
                operation(&[filesystem(FilesystemVerb::List)])
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // The contents are not in the JSON output: the command writes them to a
    // host file, asking before it overwrites one, or as raw bytes to stdout
    // when `--output` is `-`. It stays on the CLI until a command returns them.
    CommandRow {
        path: &["agent", "file-contents"],
        modes: one_mode!(
            "read",
            ManagementRead,
            [
                PREPARE_AGENT,
                operation(&[filesystem(FilesystemVerb::Read)])
            ]
        ),
        interactive: true,
        cli_only_args: &["output"],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // 6.1 renames the command to `activate-processor`.
    CommandRow {
        path: &["agent", "activate-plugin"],
        modes: one_mode!(
            "activate",
            ManagementControl,
            [
                PREPARE_AGENT,
                operation(&[pending(
                    PendingVerb::AgentActivateProcessor,
                    Agent,
                    R::Processor
                )]),
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // 6.1 renames the command to `deactivate-processor`.
    CommandRow {
        path: &["agent", "deactivate-plugin"],
        modes: one_mode!(
            "deactivate",
            ManagementControl,
            [
                PREPARE_AGENT,
                operation(&[pending(
                    PendingVerb::AgentDeactivateProcessor,
                    Agent,
                    R::Processor
                )]),
            ]
        ),
        interactive: false,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // ── exec ─────────────────────────────────────────────────────────────
    // Without `-c`, a session; with `-c`, the script's raw stdout and stderr,
    // and no JSON output type. The CLI resolves the agent's component without
    // reading the agent's metadata. Each command of a session, and its
    // helper scripts, make the requests of `SSH_SESSION_COMMAND`; a script
    // makes those of `SSH_SCRIPT_COMMAND`.
    CommandRow {
        path: &["ssh"],
        modes: &[
            Mode {
                name: "session",
                selected_by: &[],
                class: Exec,
                requests: SSH_SESSION_COMMAND,
            },
            Mode {
                name: "script",
                selected_by: &[arg("command")],
                class: Exec,
                requests: SSH_SCRIPT_COMMAND,
            },
        ],
        interactive: true,
        cli_only_args: &[],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &[],
        destructive: false,
    },
    // `--stdin` reads a host file, or the process's stdin when its value is
    // `-`; the raw output flags write to stdout or to host files. The CLI
    // resolves an `--agent` and its component without reading the agent's
    // metadata. The help modes apply when the tool's argument parser answers
    // with the tool's help, for either of its help flags (`HELP`).
    CommandRow {
        path: &["tool", "invoke"],
        modes: &[
            Mode {
                name: "hosted-start",
                selected_by: &[arg("agent")],
                class: Exec,
                requests: &[
                    request(Prepare, DESCRIBE_HOSTED),
                    tool_start(EXEC),
                    conditional(Input, When::With(&[arg("stdin")]), &[SEND_TO_ACCEPTED]),
                ],
            },
            // The server runs this invocation in a fresh owner agent today;
            // GOL-1079 makes it a standalone tool invocation.
            Mode {
                name: "standalone-start",
                selected_by: &[arg("component")],
                class: InvocationGrant,
                requests: &[
                    request(Prepare, DESCRIBE_STANDALONE),
                    tool_start(STANDALONE_INVOKE),
                    conditional(Input, When::With(&[arg("stdin")]), &[SEND_TO_ACCEPTED]),
                ],
            },
            // The tool's help is its description alone.
            Mode {
                name: "hosted-help",
                selected_by: &[arg("agent"), arg_is("tool_args", HELP)],
                class: ManagementRead,
                requests: &[operation(DESCRIBE_HOSTED)],
            },
            Mode {
                name: "standalone-help",
                selected_by: &[arg("component"), arg_is("tool_args", HELP)],
                class: ManagementRead,
                requests: &[operation(DESCRIBE_STANDALONE)],
            },
            Mode {
                name: "lookup",
                selected_by: &[arg("lookup")],
                class: InvocationGrant,
                requests: &[operation(&[invocation(
                    PendingVerb::InvocationRead,
                    InvocationSelector::IdempotencyKey,
                )])],
            },
        ],
        interactive: false,
        cli_only_args: &["stdin", "stdout", "output", "stderr", "stderr_output"],
        cli_only_output_types: &[],
        fixed_args: &[],
        capped_args: &[],
        stdin_marker_args: &["stdin"],
        destructive: false,
    },
];

#[cfg(test)]
mod test {
    use super::*;
    use crate::command::GolemCliCommand;
    use crate::model::cli_output::{command_output_schema_value, command_output_types};
    use clap::{Command, CommandFactory};
    use golem_common::model::card::permission_class_metadata;
    use std::collections::{BTreeMap, BTreeSet};
    use test_r::test;

    /// Rows outside `golem agent` that the table must keep.
    const REQUIRED_PATHS: &[&[&str]] = &[&["ssh"], &["tool", "invoke"]];

    fn find<'a>(root: &'a Command, path: &[&str]) -> Option<&'a Command> {
        path.iter()
            .try_fold(root, |command, name| command.find_subcommand(name))
    }

    fn has_arg(command: &Command, id: &str) -> bool {
        command.get_arguments().any(|a| a.get_id() == id)
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
                if !has_arg(command, arg) {
                    problems.push(format!("`{path}` has no argument `{arg}`"));
                }
            }
            let output_types = command_output_types(&path).unwrap();
            for output_type in row.cli_only_output_types {
                if !output_types.iter().any(|known| known == output_type) {
                    problems.push(format!("`{path}` has no output type `{output_type}`"));
                }
            }
            for (arg, _) in row.capped_args {
                if !command.get_arguments().any(|a| a.get_id() == *arg) {
                    problems.push(format!("`{path}` has no capped argument `{arg}`"));
                }
            }
            for arg in row.stdin_marker_args {
                if !has_arg(command, arg) {
                    problems.push(format!("`{path}` has no stdin argument `{arg}`"));
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
            check_modes(row, command, &mut problems);
            if row.interactive {
                continue;
            }
            if output_types.is_empty() {
                problems.push(format!(
                    "`{path}` is non-interactive but has no JSON output type"
                ));
            }
            let only_streams = output_types.iter().all(|output_type| {
                schema["definitions"][output_type]["x-golem-output-mode"] == "stream"
            });
            if !output_types.is_empty() && only_streams {
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

    fn check_modes(row: &CommandRow, command: &Command, problems: &mut Vec<String>) {
        let path = row.path.join(" ");
        if row.modes.is_empty() {
            problems.push(format!("`{path}` has no mode"));
        }
        let mut names = BTreeSet::new();
        let mut defaults = 0;
        let names_an_agent = has_arg(command, "agent_id");
        for mode in row.modes {
            let at = format!("`{path}` mode `{}`", mode.name);
            if !names.insert(mode.name) {
                problems.push(format!("{at} is not the only mode of that name"));
            }
            if mode.selected_by.is_empty() {
                defaults += 1;
            }
            check_selectors(&at, "selects on", mode.selected_by, command, problems);
            let operations = mode
                .requests
                .iter()
                .filter(|request| request.stage == Operation)
                .count();
            if operations != 1 {
                problems.push(format!("{at} has {operations} operation requests"));
            }
            // A command that names an agent addresses it under its own
            // `agent : view` before or with anything else that it does.
            if names_an_agent
                && !mode
                    .requests
                    .first()
                    .is_some_and(|first| first.parts.contains(&VIEW))
            {
                problems.push(format!(
                    "{at} names an agent but does not first address it under `agent : view`"
                ));
            }
            for request in mode.requests {
                if request.parts.is_empty() {
                    problems.push(format!("{at} has a request without parts"));
                }
                match request.when {
                    When::With(selectors) | When::Without(selectors) => {
                        check_selectors(&at, "depends on", selectors, command, problems)
                    }
                    When::StdinMarker if row.stdin_marker_args.is_empty() => {
                        problems.push(format!("{at} reads stdin but has no stdin argument"))
                    }
                    _ => {}
                }
                // D27: the parts on 5.3 Rule 21 targets come first, so that a
                // denial of one of them looks unknown before a tool by name
                // is named in a `403`.
                let parts = match request.marked {
                    Marked::Instead(instead) => [request.parts, instead],
                    Marked::Same => [request.parts, &[]],
                };
                for parts in parts {
                    let first_tool = parts.iter().position(|part| part.target == Tool);
                    if let Some(first_tool) = first_tool
                        && parts[first_tool..].iter().any(|part| part.target != Tool)
                    {
                        problems.push(format!(
                            "{at} checks a tool by name before a Rule 21 target"
                        ));
                    }
                }
                if let Some(extra) = request.extra
                    && extra.parts.iter().any(|part| part.target != Tool)
                {
                    problems.push(format!(
                        "{at} adds a part on a Rule 21 target after the request's parts"
                    ));
                }
                if request.marked != Marked::Same
                    && (request.stage != Operation
                        || request.parts.iter().any(|part| part.target != Agent))
                {
                    problems.push(format!(
                        "{at} depends on the never-live mark outside a control of an agent"
                    ));
                }
            }
        }
        if defaults > 1 {
            problems.push(format!("`{path}` has {defaults} default modes"));
        }
    }

    fn check_selectors(
        at: &str,
        what: &str,
        selectors: &[Selector],
        command: &Command,
        problems: &mut Vec<String>,
    ) {
        for selector in selectors {
            let Some(arg) = command.get_arguments().find(|a| a.get_id() == selector.arg) else {
                problems.push(format!("{at} {what} no argument `{}`", selector.arg));
                continue;
            };
            let possible = arg.get_possible_values();
            for value in selector.values {
                if !possible.is_empty() && !possible.iter().any(|v| v.matches(value, false)) {
                    problems.push(format!(
                        "{at} {what} `{}` with a value it does not take: `{value}`",
                        selector.arg
                    ));
                }
            }
        }
    }

    /// A pending verb names a verb that the class catalog does not have. When
    /// GOL-1276 adds one, its parts must name the catalog's verb instead.
    #[test]
    fn pending_verbs_are_not_in_the_class_catalog() {
        let catalog = permission_class_metadata();
        let added = PendingVerb::ALL
            .iter()
            .filter(|verb| {
                let (class, name) = verb.spelling();
                catalog
                    .iter()
                    .any(|entry| entry.class_name == class && entry.verbs.contains(&name))
            })
            .collect::<Vec<_>>();
        assert!(
            added.is_empty(),
            "the class catalog has these verbs; name them in the table instead: {added:?}"
        );
    }
}
