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

//! Management grants (7.5 Rules 5 and 6), over the CLI's command table.
//!
//! `CASES` writes out, for every mode of every `COMMAND_TABLE` row, the
//! requests that the mode makes, from the GOL-1403 plan and not from the
//! table. The coverage test compares them with the table. Each request
//! carries the grants that allow it and the look of its denial.
//!
//! An adapter makes the requests of one variant of a mode in order, as the
//! caller, through the service clients, the invocation session and the
//! agent connection. It never spawns the CLI. It goes on after a denied
//! metadata read of the named agent, as the CLI does, and stops at any other
//! denial. Every conditional request has an adapter variant that makes it,
//! or a pending entry in `PENDING_REQUESTS`.
//!
//! The acceptance tests need the 2.0 card model: the decision types
//! (GOL-1270), the decision wiring (GOL-1271), the denial renderers
//! (GOL-1272), the class catalog (GOL-1276), the 2.0 authorizer (GOL-1288)
//! and the switch to it (GOL-1289), all owned by Aditya Salunkhe. A grant is
//! installed in the 2.0 grant text (GOL-1275) through a 2.0 permission share
//! (GOL-1297); until then `give` refuses. The acceptance tests are ignored
//! with what they wait for. Pending cases with no test here yet:
//!
//! | Case | Waits for |
//! | --- | --- |
//! | Interrupting or resuming a standalone tool invocation, or reading its oplog, without the grant looks like an unknown invocation | GOL-1079 and GOL-1086 (the entry points); GOL-1417 adds the command rows |
//! | Closing a tool-owned webhook without `close-webhook` looks like an unknown webhook | GOL-1390 (the webhook commands); the later of GOL-1390 and GOL-1403 adds the rows |
//! | `agent : replay` admits the restricted manual snapshot-migration rehearsal on a never-live fork, which only validates the load (D12, `MANUAL_UPDATE_OF_A_MARKED_AGENT`) | The executor review and tests of the rehearsal: host-call safety, exact idle, restart behavior and the supported SDK and SQLite cases. GOL-2143 (Simon Schenk) builds the rehearsal; whether it lands before the replay PR is not agreed yet. GOL-1420 for the mark |
//!
//! Fixtures that this draft does not have yet are named where they are
//! used: `Fixture::BashTool` and `Fixture::StaticFileConfigTool`, and the
//! entries of `PENDING_REQUESTS`.

use futures::future::BoxFuture;
use golem_cli::command_table::{
    COMMAND_TABLE, CommandClass, Extra, Marked, Part, PartVerb, Request, Resource, Selector, Stage,
    Target, When,
};
use golem_client::api::{
    AgentClient, AgentClientLive, RegistryServiceClient, WorkerClient, WorkerClientLive,
};
use golem_client::invocation_session::{
    InvocationSession, InvocationSessionRequestProvider, InvocationSessionStateSnapshot,
    ReconnectPolicy, ServerFrame, SessionTransportError,
};
use golem_client::model::{
    AgentInvocationMode, AgentInvocationRequest, AgentUpdateMode, ComponentDto,
    NativeToolDescribeRequest, NativeToolInvocationMode, NativeToolInvocationRequest,
    UpdateWorkerRequest, WorkersMetadataRequest,
};
use golem_client::{Context, Error, Security};
use golem_common::base_model::worker::{
    AgentCreationRequest, RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget,
};
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::base64::Base64;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::invocation_session_public::PublicErrorCode;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_VERSION, InvocationSelector, PublicClientMessage, PublicInvocationOutcome,
    PublicInvocationResult, PublicNativeToolTarget, PublicResumeOperation, PublicServerMessage,
    PublicTypedValue,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::schema::TypedSchemaValue;
use golem_common::schema::external::ExternalTypedSchemaValue;
use golem_common::schema::public_json::{PublicSchemaValueError, encode_public_schema_value};
use golem_common::{agent_id, data_value};
use golem_schema::tool::argv::ParsedToolArguments;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_test_framework::model::IFSEntry;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

inherit_test_dep!(EnvBasedTestDependencies);

type User = TestUserContext<EnvBasedTestDependencies>;

// ── The expected classification ─────────────────────────────────────────

/// What a grant of a case is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    /// The target agent.
    Target,
    /// Every agent of the target's type.
    TargetType,
    /// Every invocation of the target agent.
    TargetInvocations,
    /// The scheduled invocation of the target, by its ID.
    ScheduledInvocation,
    /// The tool that the exec and tool modes address.
    Tool,
}

/// One grant that a case gives the caller: the class and verb of the 2.0
/// class catalog (GOL-1276), its owner and its resource, in grant text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Grant {
    class: &'static str,
    verb: &'static str,
    on: On,
    resource: &'static str,
}

const fn grant(class: &'static str, verb: &'static str, on: On, resource: &'static str) -> Grant {
    Grant {
        class,
        verb,
        on,
        resource,
    }
}

/// How a request denied for a missing grant must look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LooksLike {
    /// The same response as for an agent that does not exist.
    UnknownAgent,
    /// The same response as for an invocation of the same agent that does
    /// not exist.
    UnknownInvocation,
    /// A `403`, for a part on a tool by name.
    Forbidden,
    /// Not decided yet, with what decides it. The case asserts no shape.
    Pending(&'static str),
}

/// One request of a mode, as the plan classifies it, with the grants that
/// allow it and the look of each one's denial.
///
/// `spec` spells the request: `stage[ condition]: part, part`, then
/// ` + if condition: parts` for parts that the target adds, then
/// ` | marked: parts` for what the never-live mark changes. A part is `class verb @target [resource]`.
struct ExpectedRequest {
    spec: &'static str,
    grants: &'static [(Grant, LooksLike)],
    /// The request reads an agent that does not exist yet, so it finds
    /// nothing also with its grants.
    finds_nothing: bool,
}

/// Where a case seeds its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fixture {
    /// A `Repository` agent of `it:agent-counters`, with an oplog processor
    /// at priority 0, a seeded item, two scheduled invocations and an
    /// accepted session.
    Repository,
    /// A `FileReadWrite` agent with `/foo.txt`.
    Files,
    /// A `SnapshotCounter` agent of `it:agent-counters`, which saves and
    /// loads snapshots, so that a manual update reaches the next revision.
    Snapshot,
    /// An agent that hosts the built-in `bash` tool, which runs in the
    /// agent's file system, where the owner reads `STARTED`. Remaining
    /// GOL-1403 work: the suite has no fixture that grants the built-in
    /// `bash` release to an environment and binds it to an agent type yet.
    BashTool,
    /// A tool that reads static `File` config fields. GOL-1086 supplies the
    /// tool admission that needs the tool-wide `invoke : *` for it.
    StaticFileConfigTool,
}

/// A state that the owner puts the target in before the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Precondition {
    None,
    Interrupted,
    ProcessorInactive,
}

/// What the mode does with all its grants, which the owner observes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// The payload of the operation contains this text.
    Reads(&'static str),
    Created,
    Deleted,
    /// The target runs the next revision of its component.
    Updated,
    Interrupted,
    Resumed,
    /// The target's oplog has the caller's invocation.
    Invoked,
    /// The target's oplog has a new deleted region: the seeded item, its
    /// last invocation, is reverted.
    Reverted,
    /// The target has one scheduled invocation less.
    Cancelled,
    ProcessorActive(bool),
    /// No stable observation yet, with what supplies it.
    Pending(&'static str),
}

/// A mode of a command row as the plan classifies it.
struct Case {
    path: &'static [&'static str],
    mode: &'static str,
    /// The arguments that select the mode, spelled `arg` or
    /// `arg=value|value` and joined by `, `; `default` for none.
    selected_by: &'static str,
    class: CommandClass,
    fixture: Fixture,
    precondition: Precondition,
    requests: &'static [ExpectedRequest],
    effect: Effect,
}

const VIEW: Grant = grant("agent", "view", On::Target, "");
const VIEW_DENIED: (Grant, LooksLike) = (VIEW, LooksLike::UnknownAgent);
const OPLOG_READ: (Grant, LooksLike) = (
    grant("oplog", "read", On::Target, "*"),
    LooksLike::UnknownAgent,
);
const TOOL_VIEW: (Grant, LooksLike) = (grant("tool", "view", On::Tool, ""), LooksLike::Forbidden);
const TOOL_INVOKE_RUN: (Grant, LooksLike) = (
    grant("tool", "invoke", On::Tool, "run"),
    LooksLike::Forbidden,
);
const INVOCATION_READ: (Grant, LooksLike) = (
    grant("invocation", "read", On::TargetInvocations, ""),
    LooksLike::UnknownInvocation,
);
const INVOCATION_SEND: (Grant, LooksLike) = (
    grant("invocation", "send", On::TargetInvocations, ""),
    LooksLike::UnknownInvocation,
);
const INVOCATION_CANCEL: (Grant, LooksLike) = (
    grant("invocation", "cancel", On::TargetInvocations, ""),
    LooksLike::UnknownInvocation,
);
/// The output of an agent's invocations on a connection that names no
/// invocation. The plan requires `invocation : read` on each exposed
/// invocation; it does not say whether the connection without it is refused
/// or carries no output of that invocation.
const EXPOSED_OUTPUT: (Grant, LooksLike) = (
    grant("invocation", "read", On::TargetInvocations, ""),
    LooksLike::Pending(
        "the plan: whether a connection without `invocation : read` is refused or withholds the output",
    ),
);
/// A saved session names its invocation by a server-signed token. The
/// counterpart of an unknown invocation is a signed token for an invocation
/// that the server does not know, which the generic invocation API supplies.
const SESSION_READ: (Grant, LooksLike) = (
    grant("invocation", "read", On::TargetInvocations, ""),
    LooksLike::Pending("GOL-1051: a signed session token for an unknown invocation"),
);
const SESSION_SEND: (Grant, LooksLike) = (
    grant("invocation", "send", On::TargetInvocations, ""),
    LooksLike::Pending("GOL-1051: a signed session token for an unknown invocation"),
);

const fn control(verb: &'static str, resource: &'static str) -> (Grant, LooksLike) {
    (
        grant("agent", verb, On::Target, resource),
        LooksLike::UnknownAgent,
    )
}

const fn expected(spec: &'static str, grants: &'static [(Grant, LooksLike)]) -> ExpectedRequest {
    ExpectedRequest {
        spec,
        grants,
        finds_nothing: false,
    }
}

const PREPARE: ExpectedRequest = expected("prepare: agent view @agent", &[VIEW_DENIED]);
/// `agent new` reads the agent that it creates, which does not exist yet.
const PREPARE_NEW: ExpectedRequest = ExpectedRequest {
    finds_nothing: true,
    ..PREPARE
};
const OUTPUT_CONNECTION: ExpectedRequest = expected(
    "follow without no_stream,stdout_format=raw: invocation read @invocation:each-exposed",
    &[EXPOSED_OUTPUT],
);
const INVOKE: ExpectedRequest = expected(
    "operation: agent invoke @agent [method]",
    &[control("invoke", "add")],
);
const SEND_STDIN: ExpectedRequest = expected(
    "input if stdin-marker: invocation send @invocation:accepted",
    &[INVOCATION_SEND],
);
const RESUME_SESSION: &[ExpectedRequest] = &[
    PREPARE,
    OUTPUT_CONNECTION,
    expected(
        "operation: invocation read @invocation:session",
        &[SESSION_READ],
    ),
    expected(
        "input if stdin-marker: invocation send @invocation:session",
        &[SESSION_SEND],
    ),
];
const DESCRIBE_HOSTED: &[(Grant, LooksLike)] = &[VIEW_DENIED, TOOL_VIEW];
const EXEC: &[(Grant, LooksLike)] = &[control("exec", ""), TOOL_INVOKE_RUN];
const SSH_DESCRIBE: ExpectedRequest = expected(
    "prepare: agent view @agent, tool view @tool",
    DESCRIBE_HOSTED,
);
const SSH_START: ExpectedRequest = expected(
    "operation: agent exec @agent, tool invoke @tool [tool-command] + if tool-reads-static-file-config: tool invoke @tool [any-tool-command]",
    EXEC,
);
const SSH_STOP: ExpectedRequest = expected(
    "input if stopped-through-stdin: invocation send @invocation:accepted",
    &[INVOCATION_SEND],
);
const SSH_CANCEL: ExpectedRequest = expected(
    "cancel if stopped: invocation cancel @invocation:accepted",
    &[INVOCATION_CANCEL],
);
const SSH_LOOKUP: ExpectedRequest = expected(
    "follow if stopped-without-result: invocation read @invocation:accepted",
    &[INVOCATION_READ],
);
/// The read of the agent's state that reports a stopped command whose
/// outcome is unknown, after the lookup found no result.
const SSH_UNKNOWN_OUTCOME: ExpectedRequest = expected(
    "follow if outcome-unknown: agent view @agent",
    &[VIEW_DENIED],
);
/// A session reads the agent's state at its prompt, next to each command.
/// Without `view` the description before it already looks unknown and stops
/// the command; `agent get` covers this read on its own.
const SSH_SESSION: &[ExpectedRequest] = &[
    SSH_DESCRIBE,
    expected("follow: agent view @agent", &[VIEW_DENIED]),
    SSH_START,
    SSH_STOP,
    SSH_CANCEL,
    SSH_LOOKUP,
    SSH_UNKNOWN_OUTCOME,
];
/// A script reads the agent's state only for an unknown outcome.
const SSH_SCRIPT: &[ExpectedRequest] = &[
    SSH_DESCRIBE,
    SSH_START,
    SSH_STOP,
    SSH_CANCEL,
    SSH_LOOKUP,
    SSH_UNKNOWN_OUTCOME,
];
const OPLOG_ENTRIES: ExpectedRequest = expected(
    "follow without omit_payloads: oplog read @agent [oplog-entry]",
    &[OPLOG_READ],
);
const AWAIT_UPDATE: ExpectedRequest =
    expected("follow with await: agent view @agent", &[VIEW_DENIED]);
const REVERT: &[ExpectedRequest] = &[
    PREPARE,
    expected(
        "operation: agent revert @agent [revert-cutoff]",
        &[control("revert", "*")],
    ),
];

const fn case(
    path: &'static [&'static str],
    mode: &'static str,
    class: CommandClass,
    requests: &'static [ExpectedRequest],
    effect: Effect,
) -> Case {
    Case {
        path,
        mode,
        selected_by: "default",
        class,
        fixture: Fixture::Repository,
        precondition: Precondition::None,
        requests,
        effect,
    }
}

const fn selected(selected_by: &'static str, case: Case) -> Case {
    Case {
        selected_by,
        ..case
    }
}

const fn on(fixture: Fixture, case: Case) -> Case {
    Case { fixture, ..case }
}

const fn after(precondition: Precondition, case: Case) -> Case {
    Case {
        precondition,
        ..case
    }
}

use CommandClass::{Exec as ExecClass, InvocationGrant, ManagementControl, ManagementRead};

const CASES: &[Case] = &[
    case(
        &["agent", "new"],
        "create",
        ManagementControl,
        &[
            PREPARE_NEW,
            expected(
                "operation: agent create @new-agent",
                &[(
                    grant("agent", "create", On::TargetType, ""),
                    LooksLike::Pending("GOL-1296: the admission of a creation and its denial"),
                )],
            ),
        ],
        Effect::Created,
    ),
    case(
        &["agent", "invoke"],
        "start",
        InvocationGrant,
        &[PREPARE, OUTPUT_CONNECTION, INVOKE, SEND_STDIN],
        Effect::Invoked,
    ),
    selected(
        "trigger",
        case(
            &["agent", "invoke"],
            "trigger",
            InvocationGrant,
            &[PREPARE, OUTPUT_CONNECTION, INVOKE],
            Effect::Invoked,
        ),
    ),
    selected(
        "resume_session",
        case(
            &["agent", "invoke"],
            "resume-session",
            InvocationGrant,
            RESUME_SESSION,
            Effect::Reads("InvocationAccepted"),
        ),
    ),
    selected(
        "takeover_session",
        case(
            &["agent", "invoke"],
            "takeover-session",
            InvocationGrant,
            RESUME_SESSION,
            Effect::Reads("InvocationAccepted"),
        ),
    ),
    case(
        &["agent", "get"],
        "get",
        ManagementRead,
        &[
            PREPARE,
            expected("operation: agent view @agent", &[VIEW_DENIED]),
        ],
        Effect::Reads("management-target"),
    ),
    case(
        &["agent", "delete"],
        "delete",
        ManagementControl,
        &[
            PREPARE,
            expected(
                "operation: agent delete @agent | marked: agent replay @agent",
                &[control("delete", "")],
            ),
        ],
        Effect::Deleted,
    ),
    // The case without the grant is the GOL-1404 visibility test below: a
    // listing filters, it does not refuse.
    case(
        &["agent", "list"],
        "list",
        ManagementRead,
        &[expected(
            "operation: agent view @agent-scope",
            &[(
                grant("agent", "view", On::TargetType, ""),
                LooksLike::Pending(
                    "GOL-1404: a listing returns only the agents that `view` covers",
                ),
            )],
        )],
        Effect::Reads("management-target"),
    ),
    case(
        &["agent", "stream"],
        "follow",
        InvocationGrant,
        &[
            PREPARE,
            expected(
                "operation: invocation read @invocation:each-exposed",
                &[EXPOSED_OUTPUT],
            ),
        ],
        Effect::Reads("connected"),
    ),
    // The stdin line selects the invocation on the client today; the server
    // request that selects it comes with the generic invocation API.
    case(
        &["agent", "repl-stream"],
        "follow",
        InvocationGrant,
        &[expected(
            "operation: invocation read @invocation:stdin",
            &[(
                grant("invocation", "read", On::TargetInvocations, ""),
                LooksLike::Pending("GOL-1051: the server request that selects one invocation"),
            )],
        )],
        Effect::Reads("connected"),
    ),
    case(
        &["agent", "update"],
        "automatic",
        ManagementControl,
        &[
            PREPARE,
            expected(
                "operation: agent update-revision @agent | marked: agent replay @agent",
                &[control("update-revision", "")],
            ),
            AWAIT_UPDATE,
        ],
        Effect::Updated,
    ),
    // On a never-live fork this is the restricted snapshot-migration
    // rehearsal of D12; its admission is pending (see the module docs).
    on(
        Fixture::Snapshot,
        selected(
            "mode=manual",
            case(
                &["agent", "update"],
                "manual",
                ManagementControl,
                &[
                    PREPARE,
                    expected(
                        "operation: agent update-revision @agent | marked: agent replay @agent",
                        &[control("update-revision", "")],
                    ),
                    AWAIT_UPDATE,
                ],
                Effect::Updated,
            ),
        ),
    ),
    case(
        &["agent", "interrupt"],
        "interrupt",
        ManagementControl,
        &[
            PREPARE,
            expected(
                "operation: agent interrupt @agent",
                &[control("interrupt", "")],
            ),
        ],
        Effect::Interrupted,
    ),
    after(
        Precondition::Interrupted,
        case(
            &["agent", "resume"],
            "resume",
            ManagementControl,
            &[
                PREPARE,
                expected("operation: agent resume @agent", &[control("resume", "")]),
            ],
            Effect::Resumed,
        ),
    ),
    case(
        &["agent", "simulate-crash"],
        "simulate-crash",
        ManagementControl,
        &[
            PREPARE,
            expected(
                "operation: agent simulate-crash @agent",
                &[control("simulate-crash", "")],
            ),
        ],
        Effect::Pending("GOL-1403: an observation of a simulated crash in the agent's metadata"),
    ),
    case(
        &["agent", "oplog"],
        "page",
        ManagementRead,
        &[
            PREPARE,
            expected("operation: oplog read @agent [oplog-range]", &[OPLOG_READ]),
            OPLOG_ENTRIES,
        ],
        Effect::Reads("Create"),
    ),
    selected(
        "query",
        case(
            &["agent", "oplog"],
            "search",
            ManagementRead,
            &[
                PREPARE,
                expected("operation: oplog read @agent [oplog-all]", &[OPLOG_READ]),
                OPLOG_ENTRIES,
            ],
            Effect::Reads("Create"),
        ),
    ),
    selected(
        "last_oplog_index",
        case(
            &["agent", "revert"],
            "to-oplog-index",
            ManagementControl,
            REVERT,
            Effect::Reverted,
        ),
    ),
    selected(
        "number_of_invocations",
        case(
            &["agent", "revert"],
            "by-invocation-count",
            ManagementControl,
            REVERT,
            Effect::Reverted,
        ),
    ),
    case(
        &["agent", "cancel-invocation"],
        "cancel",
        InvocationGrant,
        &[
            PREPARE,
            expected(
                "operation: invocation cancel @invocation:idempotency-key",
                &[INVOCATION_CANCEL],
            ),
        ],
        Effect::Cancelled,
    ),
    on(
        Fixture::Files,
        case(
            &["agent", "files"],
            "list",
            ManagementRead,
            &[
                PREPARE,
                expected(
                    "operation: filesystem list @agent [path]",
                    &[(
                        grant("filesystem", "list", On::Target, "/**"),
                        LooksLike::UnknownAgent,
                    )],
                ),
            ],
            Effect::Reads("foo.txt"),
        ),
    ),
    on(
        Fixture::Files,
        case(
            &["agent", "file-contents"],
            "read",
            ManagementRead,
            &[
                PREPARE,
                expected(
                    "operation: filesystem read @agent [path]",
                    &[(
                        grant("filesystem", "read", On::Target, "/**"),
                        LooksLike::UnknownAgent,
                    )],
                ),
            ],
            Effect::Reads("foo"),
        ),
    ),
    after(
        Precondition::ProcessorInactive,
        case(
            &["agent", "activate-plugin"],
            "activate",
            ManagementControl,
            &[
                PREPARE,
                expected(
                    "operation: agent activate-processor @agent [processor]",
                    &[control("activate-processor", "*")],
                ),
            ],
            Effect::ProcessorActive(true),
        ),
    ),
    case(
        &["agent", "deactivate-plugin"],
        "deactivate",
        ManagementControl,
        &[
            PREPARE,
            expected(
                "operation: agent deactivate-processor @agent [processor]",
                &[control("deactivate-processor", "*")],
            ),
        ],
        Effect::ProcessorActive(false),
    ),
    on(
        Fixture::BashTool,
        case(
            &["ssh"],
            "session",
            ExecClass,
            SSH_SESSION,
            Effect::Reads("ToolSuccess"),
        ),
    ),
    on(
        Fixture::BashTool,
        selected(
            "command",
            case(
                &["ssh"],
                "script",
                ExecClass,
                SSH_SCRIPT,
                Effect::Reads("ToolSuccess"),
            ),
        ),
    ),
    on(
        Fixture::BashTool,
        selected(
            "agent",
            case(
                &["tool", "invoke"],
                "hosted-start",
                ExecClass,
                &[
                    expected(
                        "prepare: agent view @agent, tool view @tool",
                        DESCRIBE_HOSTED,
                    ),
                    expected(
                        "operation: agent exec @agent, tool invoke @tool [tool-command] + if tool-reads-static-file-config: tool invoke @tool [any-tool-command]",
                        EXEC,
                    ),
                    expected(
                        "input with stdin: invocation send @invocation:accepted",
                        &[INVOCATION_SEND],
                    ),
                ],
                Effect::Reads("result"),
            ),
        ),
    ),
    on(
        Fixture::BashTool,
        selected(
            "component",
            case(
                &["tool", "invoke"],
                "standalone-start",
                InvocationGrant,
                &[
                    expected("prepare: tool view @tool", &[TOOL_VIEW]),
                    expected(
                        "operation: tool invoke @tool [tool-command] + if tool-reads-static-file-config: tool invoke @tool [any-tool-command]",
                        &[TOOL_INVOKE_RUN],
                    ),
                    expected(
                        "input with stdin: invocation send @invocation:accepted",
                        &[INVOCATION_SEND],
                    ),
                ],
                Effect::Reads("result"),
            ),
        ),
    ),
    on(
        Fixture::BashTool,
        selected(
            "agent, tool_args=tool-help",
            case(
                &["tool", "invoke"],
                "hosted-help",
                ManagementRead,
                &[expected(
                    "operation: agent view @agent, tool view @tool",
                    DESCRIBE_HOSTED,
                )],
                Effect::Reads("run"),
            ),
        ),
    ),
    on(
        Fixture::BashTool,
        selected(
            "component, tool_args=tool-help",
            case(
                &["tool", "invoke"],
                "standalone-help",
                ManagementRead,
                &[expected("operation: tool view @tool", &[TOOL_VIEW])],
                Effect::Reads("run"),
            ),
        ),
    ),
    on(
        Fixture::BashTool,
        selected(
            "lookup",
            case(
                &["tool", "invoke"],
                "lookup",
                InvocationGrant,
                &[expected(
                    "operation: invocation read @invocation:idempotency-key",
                    &[INVOCATION_READ],
                )],
                Effect::Reads("result"),
            ),
        ),
    ),
];

/// Conditional requests that no adapter makes yet, with what they wait for.
const PENDING_REQUESTS: &[(&[&str], &str, usize, &str)] = &[
    (
        &["agent", "invoke"],
        "start",
        3,
        "remaining GOL-1403 work: a fixture method with a stream parameter, fed through the session's input stream binding",
    ),
    (
        &["agent", "invoke"],
        "resume-session",
        3,
        "remaining GOL-1403 work: a fixture method with a stream parameter, fed through the session's input stream binding",
    ),
    (
        &["agent", "invoke"],
        "takeover-session",
        3,
        "remaining GOL-1403 work: a fixture method with a stream parameter, fed through the session's input stream binding",
    ),
    (
        &["ssh"],
        "session",
        3,
        "remaining GOL-1403 work: the stop through the stdin stream binding of a tool invocation, with `Fixture::BashTool`",
    ),
    (
        &["ssh"],
        "script",
        2,
        "remaining GOL-1403 work: the stop through the stdin stream binding of a tool invocation, with `Fixture::BashTool`",
    ),
    (
        &["tool", "invoke"],
        "hosted-start",
        2,
        "remaining GOL-1403 work: the stdin stream binding of a tool invocation, with `Fixture::BashTool`",
    ),
    (
        &["tool", "invoke"],
        "standalone-start",
        2,
        "remaining GOL-1403 work: the stdin stream binding of a tool invocation, with `Fixture::BashTool`",
    ),
];

/// The modes that change an execution or start work, which `view` and
/// `oplog : read` alone must never allow.
const CONTROLS_AND_STARTS: &[(&[&str], &str)] = &[
    (&["agent", "new"], "create"),
    (&["agent", "invoke"], "start"),
    (&["agent", "invoke"], "trigger"),
    (&["agent", "delete"], "delete"),
    (&["agent", "update"], "automatic"),
    (&["agent", "update"], "manual"),
    (&["agent", "interrupt"], "interrupt"),
    (&["agent", "resume"], "resume"),
    (&["agent", "simulate-crash"], "simulate-crash"),
    (&["agent", "revert"], "to-oplog-index"),
    (&["agent", "revert"], "by-invocation-count"),
    (&["agent", "cancel-invocation"], "cancel"),
    (&["agent", "activate-plugin"], "activate"),
    (&["agent", "deactivate-plugin"], "deactivate"),
    (&["ssh"], "session"),
    (&["ssh"], "script"),
    (&["tool", "invoke"], "hosted-start"),
    (&["tool", "invoke"], "standalone-start"),
];

// ── Spelling the table's requests ───────────────────────────────────────

fn kebab(debug: impl std::fmt::Debug) -> String {
    let debug = format!("{debug:?}");
    let mut spelled = String::new();
    for (index, character) in debug.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                spelled.push('-');
            }
            spelled.extend(character.to_lowercase());
        } else {
            spelled.push(character);
        }
    }
    spelled
}

fn spell_part(part: &Part) -> String {
    let verb = match part.verb {
        PartVerb::Agent(verb) => format!("agent {}", kebab(verb)),
        PartVerb::Oplog(verb) => format!("oplog {}", kebab(verb)),
        PartVerb::Filesystem(verb) => format!("filesystem {}", kebab(verb)),
        PartVerb::Tool(verb) => format!("tool {}", kebab(verb)),
        PartVerb::Pending(verb) => {
            let (class, verb) = verb.spelling();
            format!("{class} {verb}")
        }
    };
    let target = match part.target {
        Target::Invocation(selector) => format!("invocation:{}", kebab(selector)),
        target => kebab(target),
    };
    match part.resource {
        Resource::None => format!("{verb} @{target}"),
        resource => format!("{verb} @{target} [{}]", kebab(resource)),
    }
}

fn spell_parts(parts: &[Part]) -> String {
    parts.iter().map(spell_part).collect::<Vec<_>>().join(", ")
}

fn spell_selectors(selectors: &[Selector], separator: &str) -> String {
    if selectors.is_empty() {
        return "default".to_string();
    }
    selectors
        .iter()
        .map(|selector| match selector {
            Selector::Given(arg) => arg.to_string(),
            Selector::Value(arg, values) => format!("{arg}={}", values.join("|")),
            Selector::ToolHelp(arg) => format!("{arg}=tool-help"),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

fn spell_request(request: &Request) -> String {
    let stage = kebab(request.stage);
    let when = match request.when {
        When::Always => String::new(),
        When::With(selectors) => format!(" with {}", spell_selectors(selectors, ",")),
        When::Without(selectors) => format!(" without {}", spell_selectors(selectors, ",")),
        when => format!(" if {}", kebab(when)),
    };
    let mut spelled = format!("{stage}{when}: {}", spell_parts(request.parts));
    if let Some(Extra { when, parts }) = request.extra {
        spelled.push_str(&format!(" + if {}: {}", kebab(when), spell_parts(parts)));
    }
    match request.marked {
        Marked::Same => {}
        Marked::Instead(parts) => spelled.push_str(&format!(" | marked: {}", spell_parts(parts))),
    }
    spelled
}

// ── Requests and their outcomes ─────────────────────────────────────────

/// What one request ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// Allowed, with what the caller received.
    Allowed(String),
    Denied(Denial),
}

/// A denial, as the caller sees it. Two denials are the same response when
/// they are equal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Denial {
    /// The status, the stable headers and the whole body of an HTTP
    /// response.
    Http(RawResponse),
    /// The rejection message of an invocation session, without the attempt
    /// ID that it echoes from the request.
    Session(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawResponse {
    status: u16,
    /// Every header value, in the order of the response.
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
}

/// Headers that differ between two responses to the same request.
const UNSTABLE_HEADERS: &[&str] = &["date"];

impl RawResponse {
    fn new(status: u16, headers: &http::HeaderMap, body: &[u8]) -> Self {
        Self {
            status,
            headers: headers
                .iter()
                .filter(|(name, _)| !UNSTABLE_HEADERS.contains(&name.as_str()))
                .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
                .collect(),
            body: body.to_vec(),
        }
    }
}

/// Keeps the last response of the caller's HTTP clients, which the
/// generated clients reduce to a parsed body.
#[derive(Clone, Default)]
struct LastResponse(Arc<Mutex<Option<RawResponse>>>);

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for LastResponse {
    async fn handle(
        &self,
        request: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let response = next.run(request, extensions).await?;
        let status = response.status();
        let version = response.version();
        let headers = response.headers().clone();
        let body = response.bytes().await?;
        *self.0.lock().unwrap() = Some(RawResponse::new(status.as_u16(), &headers, &body));
        let mut rebuilt = http::Response::new(body);
        *rebuilt.status_mut() = status;
        *rebuilt.version_mut() = version;
        *rebuilt.headers_mut() = headers;
        Ok(reqwest::Response::from(rebuilt))
    }
}

/// One request of an adapter: the CLI's metadata read of the named agent,
/// after which it goes on when the agent looks unknown, or any other request.
enum Step {
    Prepare(Outcome),
    Request(Outcome),
}

/// The outcomes of the requests that an adapter made, in order.
#[derive(Debug, Default)]
struct Trace {
    outcomes: Vec<Outcome>,
    stopped: bool,
}

impl Trace {
    fn push(&mut self, step: Step) {
        let (outcome, goes_on) = match step {
            Step::Prepare(outcome) => {
                let goes_on = matches!(
                    &outcome,
                    Outcome::Allowed(_)
                        | Outcome::Denied(Denial::Http(RawResponse { status: 404, .. }))
                );
                (outcome, goes_on)
            }
            Step::Request(outcome) => {
                let goes_on = matches!(outcome, Outcome::Allowed(_));
                (outcome, goes_on)
            }
        };
        self.outcomes.push(outcome);
        self.stopped = !goes_on;
    }
}

/// Makes the requests in order, as long as the earlier ones let the command
/// go on. Each step sees `$seed` as the seed of its trace position.
macro_rules! in_order {
    ($seed:ident; $($step:expr),+ $(,)?) => {{
        let mut trace = Trace::default();
        $(
            if !trace.stopped {
                let at = $seed.at(trace.outcomes.len());
                let $seed: &Seed = &at;
                trace.push($step);
            }
        )+
        Ok(trace)
    }};
}

// ── The seeded target ───────────────────────────────────────────────────

const AGENT_TYPE: &str = "Repository";
const METHOD: &str = "add";
const SNAPSHOT_AGENT_TYPE: &str = "SnapshotCounter";
/// The item that the owner adds after the target starts.
const SEED_ITEM: &str = "seed-item";
/// The item that a caller's invocation adds.
const CALL_ITEM: &str = "call-item";
/// The tool that the exec and tool modes address.
const TOOL: &str = "bash";

/// The owner's agent, component and invocations that the caller addresses.
#[derive(Clone)]
struct Seed {
    owner: User,
    caller: User,
    caller_response: LastResponse,
    application: String,
    environment: String,
    component: ComponentDto,
    /// The revision that an update moves the target to.
    next_revision: u64,
    /// The agent that the commands address.
    target: ParsedAgentKey,
    target_id: AgentId,
    /// The agent that `agent new` creates.
    new_agent: ParsedAgentKey,
    /// The oplog index before the seeded item.
    before_seed_item: OplogIndex,
    /// An invocation of the target that waits in its schedule, so that it
    /// stays cancellable.
    scheduled_invocation: String,
    /// Another one, which a grant on the first one does not cover.
    other_scheduled_invocation: String,
    /// The session token of an invocation of the target that the owner
    /// started.
    session_token: String,
    /// When set, the request at this trace position addresses this missing
    /// invocation of the target instead of the one that the command started.
    missing_invocation: Option<(usize, String)>,
    /// When set, the request at this trace position addresses an agent of
    /// the target's type that does not exist.
    unknown_agent_at: Option<usize>,
}

impl Seed {
    async fn new(deps: &EnvBasedTestDependencies, fixture: Fixture) -> anyhow::Result<Self> {
        let owner = deps.user().await?;
        let caller = deps.user().await?;
        let (_, env) = owner.app_and_env().await?;
        let (component, agent_type) = match fixture {
            Fixture::Repository => {
                let processor = owner
                    .component(&env.id, "oplog_processor_release")
                    .unique()
                    .store()
                    .await?;
                let registry = owner.registry_service_client().await;
                let plugin = registry
                    .create_plugin(
                        &owner.account_id.0,
                        &PluginRegistrationCreation {
                            name: format!("management-processor-{}", uuid::Uuid::new_v4()),
                            version: "v1".to_string(),
                            description: "The processor that the plugin modes switch".to_string(),
                            icon: Base64(Vec::new()),
                            homepage: "none".to_string(),
                            spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                                component_id: processor.id,
                                component_revision: processor.revision,
                            }),
                        },
                    )
                    .await?;
                let plugin_grant = registry
                    .create_environment_plugin_grant(
                        &env.id.0,
                        &EnvironmentPluginGrantCreation {
                            plugin_registration_id: plugin.id,
                        },
                    )
                    .await?;
                let component = owner
                    .component(&env.id, "it_agent_counters_release")
                    .name("it:agent-counters")
                    .unique()
                    .with_parametrized_plugin(
                        AGENT_TYPE,
                        &plugin_grant.id,
                        0,
                        BTreeMap::from([(
                            "callback-url".to_string(),
                            "http://127.0.0.1:9/".to_string(),
                        )]),
                    )
                    .store()
                    .await?;
                (component, AGENT_TYPE)
            }
            Fixture::Files => {
                let component = owner
                    .component(&env.id, "it_initial_file_system_release")
                    .name("golem-it:initial-file-system")
                    .unique()
                    .with_files(
                        "FileReadWrite",
                        &[IFSEntry {
                            source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                            target_path: CanonicalFilePath::from_abs_str("/foo.txt")
                                .map_err(anyhow::Error::msg)?,
                            permissions: AgentFilePermissions::ReadOnly,
                        }],
                    )
                    .store()
                    .await?;
                (component, "FileReadWrite")
            }
            Fixture::Snapshot => {
                let component = owner
                    .component(&env.id, "it_agent_counters_release")
                    .name("it:agent-counters")
                    .unique()
                    .store()
                    .await?;
                (component, SNAPSHOT_AGENT_TYPE)
            }
            Fixture::BashTool => anyhow::bail!(
                "pending, remaining GOL-1403 work: a fixture that grants the built-in `bash` release to the environment and binds it to an agent type"
            ),
            Fixture::StaticFileConfigTool => anyhow::bail!(
                "pending on GOL-1086: a tool that reads static `File` config fields, and its admission"
            ),
        };
        // The target starts on the component's first revision, before the
        // next one is published, so that an update moves it.
        let target = agent_id!(agent_type, "management-target");
        let target_id = owner.start_agent(&component.id, target.clone()).await?;
        let mut seed = Self {
            owner,
            caller,
            caller_response: LastResponse::default(),
            application: env.application_name.0.clone(),
            environment: env.name.0.clone(),
            component,
            next_revision: 0,
            target,
            target_id,
            new_agent: agent_id!(agent_type, "management-new"),
            before_seed_item: OplogIndex::from_u64(0),
            scheduled_invocation: String::new(),
            other_scheduled_invocation: String::new(),
            session_token: String::new(),
            missing_invocation: None,
            unknown_agent_at: None,
        };
        if fixture == Fixture::Snapshot {
            // A manual update carries this state over through a snapshot.
            seed.owner
                .invoke_and_await_agent(&seed.component, &seed.target, "increment", data_value!())
                .await?;
        }
        if fixture == Fixture::Repository {
            seed.session_token =
                match start_session(&seed, &seed.owner, "session-item", false).await? {
                    (Outcome::Allowed(_), Some(accepted)) => accepted.session_token,
                    other => anyhow::bail!("the owner could not start a session: {other:?}"),
                };
            seed.scheduled_invocation = seed.schedule_invocation().await?;
            seed.other_scheduled_invocation = seed.schedule_invocation().await?;
            seed.wait_until_only_the_scheduled_invocations_are_pending()
                .await?;
            // The seeded item is the target's last invocation, so that
            // reverting one invocation or to the index before it removes it.
            seed.before_seed_item = worker_client(&seed.owner)
                .await
                .get_worker_metadata(&seed.component.id.0, seed.agent_name())
                .await?
                .last_oplog_index;
            seed.owner
                .invoke_and_await_agent(
                    &seed.component,
                    &seed.target,
                    METHOD,
                    data_value!(SEED_ITEM, "added by the owner"),
                )
                .await?;
        }
        seed.next_revision = seed
            .owner
            .update_component(&seed.component.id, component_file(fixture))
            .await?
            .revision
            .into();
        Ok(seed)
    }

    /// The same seed, addressing an agent of the same type that does not
    /// exist.
    fn with_unknown_agent(&self) -> Self {
        let target = agent_id!(self.target.agent_type.0, "management-unknown");
        Self {
            target_id: AgentId {
                component_id: self.component.id,
                agent_id: target.to_string(),
            },
            target,
            ..self.clone()
        }
    }

    /// The same seed, and the request at trace `position` addresses an
    /// agent of the target's type that does not exist. The requests before
    /// it stay on the target.
    fn with_unknown_agent_at(&self, position: usize) -> Self {
        Self {
            unknown_agent_at: Some(position),
            ..self.clone()
        }
    }

    /// The seed of the request at trace `position`.
    fn at(&self, position: usize) -> Cow<'_, Seed> {
        if self.unknown_agent_at == Some(position) {
            Cow::Owned(self.with_unknown_agent())
        } else {
            Cow::Borrowed(self)
        }
    }

    /// The same seed and agent, addressing scheduled invocations of it that
    /// do not exist.
    fn with_unknown_invocation(&self) -> Self {
        Self {
            scheduled_invocation: uuid::Uuid::new_v4().to_string(),
            other_scheduled_invocation: uuid::Uuid::new_v4().to_string(),
            ..self.clone()
        }
    }

    /// As `with_unknown_invocation`, and the request at trace `position`
    /// addresses a missing invocation of the agent instead of the one that
    /// the command started. The requests before it stay valid.
    fn with_unknown_invocation_at(&self, position: usize) -> Self {
        Self {
            missing_invocation: Some((position, uuid::Uuid::new_v4().to_string())),
            ..self.with_unknown_invocation()
        }
    }

    /// The invocation that the request at trace `position` addresses.
    fn addressed(&self, position: usize, accepted: &str) -> String {
        match &self.missing_invocation {
            Some((missing_at, missing)) if *missing_at == position => missing.clone(),
            _ => accepted.to_string(),
        }
    }

    /// Waits until the owner reads `marker` in the target's files.
    async fn wait_until_the_script_runs(&self, marker: &str) -> anyhow::Result<()> {
        let worker = worker_client(&self.owner).await;
        for _ in 0..30 {
            if worker
                .get_file_content(&self.component.id.0, self.agent_name(), marker)
                .await
                .is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        anyhow::bail!("the script did not start")
    }

    /// Waits until the owner's lookup of the tool invocation under `key`
    /// answers: with its result, or with its end by the cancellation.
    async fn wait_until_the_command_ends(&self, key: &str) -> anyhow::Result<()> {
        let request = tool_request(self, true, NativeToolInvocationMode::Lookup, key)?;
        let lookup = async {
            agent_client(&self.owner)
                .await
                .invoke_tool(Some(key), &request)
                .await
        };
        match tokio::time::timeout(Duration::from_secs(60), lookup).await {
            Ok(_) => Ok(()),
            Err(_) => anyhow::bail!("the stopped command did not end"),
        }
    }

    async fn wait_until_only_the_scheduled_invocations_are_pending(&self) -> anyhow::Result<()> {
        let worker = worker_client(&self.owner).await;
        for _ in 0..30 {
            let metadata = worker
                .get_worker_metadata(&self.component.id.0, self.agent_name())
                .await?;
            if metadata.pending_invocation_count == 2 && metadata.status == AgentStatus::Idle {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        anyhow::bail!("the owner's session invocation did not finish")
    }

    async fn schedule_invocation(&self) -> anyhow::Result<String> {
        let mut request =
            invocation_request(self, AgentInvocationMode::Schedule, "scheduled-item")?;
        request.schedule_at = Some(chrono::Utc::now() + chrono::Duration::days(1));
        let result = agent_client(&self.owner)
            .await
            .invoke_agent(None, &request)
            .await?;
        Ok(result.idempotency_key.to_string())
    }

    fn agent_name(&self) -> &str {
        &self.target_id.agent_id
    }

    /// The caller's clients, which keep their last response.
    fn caller_context(&self) -> Context {
        *self.caller_response.0.lock().unwrap() = None;
        let service = self.caller.deps.worker_service();
        Context {
            client: reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
                .with(self.caller_response.clone())
                .build(),
            base_url: url::Url::parse(&format!(
                "http://{}:{}",
                service.http_host(),
                service.http_port()
            ))
            .expect("the worker service URL"),
            security_token: Security::Bearer(self.caller.token.secret().to_string()),
        }
    }

    fn worker(&self) -> WorkerClientLive {
        WorkerClientLive {
            context: self.caller_context(),
        }
    }

    fn agent(&self) -> AgentClientLive {
        AgentClientLive {
            context: self.caller_context(),
        }
    }

    /// The outcome of a request of the caller's clients.
    fn rest<T: std::fmt::Debug, E: std::fmt::Debug>(
        &self,
        result: Result<T, Error<E>>,
    ) -> anyhow::Result<Outcome> {
        match result {
            Ok(value) => Ok(Outcome::Allowed(format!("{value:?}"))),
            Err(error) => match self.caller_response.0.lock().unwrap().take() {
                Some(response) if response.status >= 400 => {
                    Ok(Outcome::Denied(Denial::Http(response)))
                }
                _ => Err(anyhow::anyhow!("{error:?}")),
            },
        }
    }

    async fn put_in(&self, precondition: Precondition) -> anyhow::Result<()> {
        let worker = worker_client(&self.owner).await;
        match precondition {
            Precondition::None => {}
            Precondition::Interrupted => {
                worker
                    .interrupt_worker(&self.component.id.0, self.agent_name(), Some(false))
                    .await?;
            }
            Precondition::ProcessorInactive => {
                worker
                    .deactivate_plugin(&self.component.id.0, self.agent_name(), 0)
                    .await?;
            }
        }
        Ok(())
    }

    /// What the owner sees of the target and of the agent that `agent new`
    /// creates, through reads that neither wake nor invoke the target.
    async fn observe(&self) -> anyhow::Result<Observation> {
        let worker = worker_client(&self.owner).await;
        let metadata = worker
            .get_worker_metadata(&self.component.id.0, self.agent_name())
            .await
            .ok();
        let created = worker
            .get_worker_metadata(&self.component.id.0, &self.new_agent.to_string())
            .await
            .is_ok();
        let caller_invocations = match &metadata {
            Some(_) => worker
                .get_oplog(
                    &self.component.id.0,
                    self.agent_name(),
                    None,
                    100,
                    None,
                    Some(CALL_ITEM),
                )
                .await?
                .entries
                .len(),
            None => 0,
        };
        Ok(Observation {
            target: metadata.map(|metadata| TargetState {
                status: metadata.status,
                revision: metadata.component_revision.into(),
                pending_invocations: metadata.pending_invocation_count,
                active_processors: metadata.active_plugins.len(),
                deleted_regions: metadata.deleted_regions.len(),
            }),
            created,
            caller_invocations,
        })
    }
}

fn component_file(fixture: Fixture) -> &'static str {
    match fixture {
        Fixture::Files => "it_initial_file_system_release",
        _ => "it_agent_counters_release",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observation {
    target: Option<TargetState>,
    created: bool,
    /// The oplog entries of the target that name the caller's item.
    caller_invocations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetState {
    status: AgentStatus,
    revision: u64,
    pending_invocations: u64,
    active_processors: usize,
    deleted_regions: usize,
}

async fn worker_client(user: &User) -> WorkerClientLive {
    user.deps
        .worker_service()
        .worker_http_client(&user.token)
        .await
}

async fn agent_client(user: &User) -> AgentClientLive {
    user.deps
        .worker_service()
        .agent_http_client(&user.token)
        .await
}

fn public_json(value: &TypedSchemaValue) -> anyhow::Result<serde_json::Value> {
    Ok(encode_public_schema_value(
        value.graph(),
        value.root_type(),
        value.value(),
        |_, _| {
            Err(PublicSchemaValueError::new(
                PublicErrorCode::UnsupportedValue,
                "no streams",
            ))
        },
    )?)
}

fn invocation_request(
    seed: &Seed,
    mode: AgentInvocationMode,
    item: &str,
) -> anyhow::Result<AgentInvocationRequest> {
    Ok(AgentInvocationRequest {
        app_name: seed.application.clone(),
        env_name: seed.environment.clone(),
        agent_type_name: AGENT_TYPE.to_string(),
        parameters: seed
            .target
            .parameters
            .value()
            .clone()
            .try_into()
            .map_err(anyhow::Error::msg)?,
        phantom_id: seed.target.phantom_id,
        config: None,
        method_name: METHOD.to_string(),
        method_parameters: data_value!(item, "added by an invocation")
            .value()
            .clone()
            .try_into()
            .map_err(anyhow::Error::msg)?,
        mode,
        schedule_at: None,
        idempotency_key: None,
        deployment_revision: None,
        owner_account_email: None,
    })
}

// ── The invocation session and the agent connection ────────────────────

struct SessionRequests {
    url: String,
    token: String,
}

#[async_trait::async_trait]
impl InvocationSessionRequestProvider for SessionRequests {
    async fn request(&self) -> Result<tungstenite::http::Request<()>, SessionTransportError> {
        let fail = |error: String| SessionTransportError::RequestProvider(error);
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|error| fail(error.to_string()))?;
        request.headers_mut().insert(
            tungstenite::http::header::AUTHORIZATION,
            format!("Bearer {}", self.token)
                .parse()
                .map_err(|_| fail("invalid token".to_string()))?,
        );
        Ok(request)
    }
}

/// An invocation that a session accepted.
#[derive(Debug, Clone)]
struct Accepted {
    session_token: String,
    idempotency_key: String,
}

/// Opens an invocation session with one start or resume, and returns whether
/// the server accepted it. With `until_result`, an accepted outcome carries
/// the invocation's successful result instead of the acceptance.
async fn session(
    user: &User,
    operation: PublicClientMessage,
    until_result: bool,
) -> anyhow::Result<(Outcome, Option<Accepted>)> {
    let mut session = open_session(user, operation).await?;
    receive(&mut session, until_result).await
}

async fn open_session(
    user: &User,
    operation: PublicClientMessage,
) -> anyhow::Result<InvocationSession> {
    let service = user.deps.worker_service();
    let requests = SessionRequests {
        url: format!(
            "ws://{}:{}/v1/agents/invoke-agent-session",
            service.http_host(),
            service.http_port()
        ),
        token: user.token.secret().to_string(),
    };
    Ok(InvocationSession::open(
        Arc::new(requests),
        None,
        ReconnectPolicy::Within(Duration::from_secs(30)),
        InvocationSessionStateSnapshot {
            delivered_output_cursors: Default::default(),
            stable_stream_bindings: Default::default(),
            pending_operation: Some(operation),
            session_token: None,
            accepted_invocation: None,
        },
        false,
        Arc::new(()),
    )
    .await?)
}

/// Receives the session's frames until its operation is rejected, accepted
/// or, with `until_result`, finished. A failed invocation is an error of the
/// test: a positive case needs the invocation's success, and a denial is a
/// rejection.
async fn receive(
    session: &mut InvocationSession,
    until_result: bool,
) -> anyhow::Result<(Outcome, Option<Accepted>)> {
    let mut accepted = None;
    let mut result = None;
    loop {
        let mut frame = session.receive().await?;
        let message = frame.frame().clone();
        frame.mark_delivered()?;
        match message {
            ServerFrame::Message(PublicServerMessage::InvocationAccepted {
                session_token,
                idempotency_key,
                agent_id,
                ..
            }) => {
                let invocation = Accepted {
                    session_token,
                    idempotency_key: idempotency_key.to_string(),
                };
                if !until_result {
                    return Ok((
                        Outcome::Allowed(format!(
                            "InvocationAccepted {agent_id:?} {idempotency_key}"
                        )),
                        Some(invocation),
                    ));
                }
                accepted = Some(invocation);
            }
            ServerFrame::Message(PublicServerMessage::InvocationResult {
                result: invocation_result,
                ..
            }) if accepted.is_some() => {
                if let PublicInvocationResult::ToolFailure { .. } = *invocation_result {
                    anyhow::bail!("the invocation failed: {invocation_result:?}");
                }
                result = Some(format!("{invocation_result:?}"));
            }
            ServerFrame::Message(PublicServerMessage::InvocationFinished { outcome, .. })
                if accepted.is_some() =>
            {
                return match outcome {
                    PublicInvocationOutcome::Success => Ok((
                        Outcome::Allowed(result.unwrap_or_else(|| "Success".to_string())),
                        accepted,
                    )),
                    failure => anyhow::bail!("the invocation failed: {failure:?}"),
                };
            }
            ServerFrame::Message(PublicServerMessage::InvocationRejected {
                code,
                message,
                retryable,
                stderr,
                version,
                ..
            }) => {
                return Ok((
                    Outcome::Denied(Denial::Session(format!(
                        "{code:?} {message:?} retryable={retryable} stderr={stderr:?} version={version}"
                    ))),
                    None,
                ));
            }
            _ => continue,
        }
    }
}

async fn start_session(
    seed: &Seed,
    user: &User,
    item: &str,
    until_result: bool,
) -> anyhow::Result<(Outcome, Option<Accepted>)> {
    let selector = InvocationSelector {
        agent_type: AGENT_TYPE.to_string(),
        application: seed.application.clone(),
        constructor_parameters: public_json(&seed.target.parameters)?,
        environment: seed.environment.clone(),
        method: METHOD.to_string(),
        phantom_id: seed.target.phantom_id,
    };
    session(
        user,
        PublicClientMessage::InvocationStart {
            attempt_id: uuid::Uuid::new_v4(),
            config: Vec::new(),
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            method_parameters: public_json(&data_value!(item, "added by an invocation"))?,
            selector: Box::new(selector),
            version: INVOCATION_SESSION_VERSION,
        },
        until_result,
    )
    .await
}

async fn resume_session(seed: &Seed, operation: PublicResumeOperation) -> anyhow::Result<Outcome> {
    Ok(session(
        &seed.caller,
        PublicClientMessage::ResumeAttach {
            attempt_id: uuid::Uuid::new_v4(),
            operation,
            output_cursors: Vec::new(),
            session_token: seed.session_token.clone(),
            version: INVOCATION_SESSION_VERSION,
        },
        false,
    )
    .await?
    .0)
}

/// Opens the agent's live output connection, as `agent stream` and
/// `agent invoke` without `--no-stream` do, and closes it.
async fn connect(seed: &Seed) -> anyhow::Result<Outcome> {
    let service = seed.caller.deps.worker_service();
    let mut request = format!(
        "ws://{}:{}/v1/components/{}/workers/{}/connect",
        service.http_host(),
        service.http_port(),
        seed.target_id.component_id.0,
        seed.target_id.agent_name_encoded(),
    )
    .into_client_request()?;
    request.headers_mut().insert(
        tungstenite::http::header::AUTHORIZATION,
        format!("Bearer {}", seed.caller.token.secret()).parse()?,
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok((mut stream, _)) => {
            let _ = stream.close(None).await;
            Ok(Outcome::Allowed("connected".to_string()))
        }
        Err(tungstenite::Error::Http(response)) => {
            Ok(Outcome::Denied(Denial::Http(RawResponse::new(
                response.status().as_u16(),
                response.headers(),
                response.body().as_deref().unwrap_or_default(),
            ))))
        }
        Err(error) => Err(error.into()),
    }
}

/// The script that a command runs to its result.
const SCRIPT: &str = "true";

/// The typed input of the built-in `bash` tool's `run` for `script`, as
/// `golem ssh` builds it: `run -- <script>`, parsed against the pinned
/// definition that the CLI checks the hosted tool against. The parser fills
/// in `cwd` and `timeout` with their defaults and leaves `stdin` unbound.
fn run_input(script: &str) -> anyhow::Result<TypedSchemaValue> {
    let tool: golem_common::schema::tool::Tool = serde_json::from_str(include_str!(
        "../../../cli/golem-cli/src/command_handler/ssh/bash-0.3.0.json"
    ))?;
    let argv = ["run", "--", script].map(str::to_string);
    match golem_schema::tool::argv::parse(&tool, &argv).map_err(anyhow::Error::msg)? {
        ParsedToolArguments::Invoke { input, .. } => Ok(*input),
        ParsedToolArguments::Help(_) => anyhow::bail!("`run` answered with its help"),
    }
}

fn tool_start(
    seed: &Seed,
    target: PublicNativeToolTarget,
    idempotency_key: &str,
    script: &str,
) -> anyhow::Result<PublicClientMessage> {
    Ok(PublicClientMessage::ToolStart {
        attempt_id: uuid::Uuid::new_v4(),
        application: seed.application.clone(),
        environment: seed.environment.clone(),
        idempotency_key: idempotency_key.to_string(),
        tool_name: TOOL.to_string(),
        command_path: vec!["run".to_string()],
        target,
        input: Box::new(
            PublicTypedValue::try_from(run_input(script)?).map_err(anyhow::Error::msg)?,
        ),
        stdin: false,
        stdout: false,
        stderr: false,
        version: INVOCATION_SESSION_VERSION,
    })
}

fn hosted_target(seed: &Seed) -> PublicNativeToolTarget {
    PublicNativeToolTarget::Agent {
        component_id: seed.component.id.0,
        agent_id: seed.agent_name().to_string(),
    }
}

/// A lookup names only the invocation's key; every other mode carries the
/// input of `run`.
fn tool_request(
    seed: &Seed,
    hosted: bool,
    mode: NativeToolInvocationMode,
    idempotency_key: &str,
) -> anyhow::Result<NativeToolInvocationRequest> {
    let input = match mode {
        NativeToolInvocationMode::Lookup => None,
        _ => Some(
            ExternalTypedSchemaValue::try_from(run_input(SCRIPT)?).map_err(anyhow::Error::msg)?,
        ),
    };
    Ok(NativeToolInvocationRequest {
        app_name: seed.application.clone(),
        env_name: seed.environment.clone(),
        agent_id: hosted.then(|| seed.target_id.clone()),
        component_id: (!hosted).then_some(seed.component.id.0),
        tool_name: TOOL.to_string(),
        command_path: vec!["run".to_string()],
        input,
        mode,
        schedule_at: None,
        idempotency_key: Some(idempotency_key.to_string()),
    })
}

// ── The requests ────────────────────────────────────────────────────────

/// The CLI's metadata read of an agent, which it makes first.
async fn prepare(seed: &Seed, agent_name: &str) -> anyhow::Result<Step> {
    Ok(Step::Prepare(
        seed.rest(
            seed.worker()
                .get_worker_metadata(&seed.component.id.0, agent_name)
                .await,
        )?,
    ))
}

async fn prepare_target(seed: &Seed) -> anyhow::Result<Step> {
    prepare(seed, seed.agent_name()).await
}

async fn metadata(seed: &Seed) -> anyhow::Result<Step> {
    Ok(Step::Request(
        seed.rest(
            seed.worker()
                .get_worker_metadata(&seed.component.id.0, seed.agent_name())
                .await,
        )?,
    ))
}

async fn request(
    outcome: impl std::future::Future<Output = anyhow::Result<Outcome>>,
) -> anyhow::Result<Step> {
    Ok(Step::Request(outcome.await?))
}

async fn invoke(seed: &Seed, mode: AgentInvocationMode) -> anyhow::Result<Outcome> {
    let request = invocation_request(seed, mode, CALL_ITEM)?;
    seed.rest(seed.agent().invoke_agent(None, &request).await)
}

async fn update(seed: &Seed, mode: AgentUpdateMode) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.worker()
            .update_worker(
                &seed.component.id.0,
                seed.agent_name(),
                &UpdateWorkerRequest {
                    mode,
                    target_revision: seed.next_revision,
                    disable_wakeup: Some(true),
                },
            )
            .await,
    )
}

/// The CLI's wait for an update: it reads the agent's metadata until the
/// update is done.
async fn await_update(seed: &Seed) -> anyhow::Result<Outcome> {
    for _ in 0..30 {
        let outcome = seed.rest(
            seed.worker()
                .get_worker_metadata(&seed.component.id.0, seed.agent_name())
                .await,
        )?;
        match &outcome {
            Outcome::Allowed(metadata)
                if metadata.contains(&format!("component_revision: {}", seed.next_revision)) =>
            {
                return Ok(outcome);
            }
            Outcome::Allowed(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            Outcome::Denied(_) => return Ok(outcome),
        }
    }
    anyhow::bail!("the update did not finish")
}

async fn revert(seed: &Seed, target: RevertWorkerTarget) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.worker()
            .revert_worker(&seed.component.id.0, seed.agent_name(), &target)
            .await,
    )
}

async fn oplog_page(seed: &Seed, query: Option<&str>) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.worker()
            .get_oplog(
                &seed.component.id.0,
                seed.agent_name(),
                query.is_none().then_some(0),
                10,
                None,
                query,
            )
            .await,
    )
}

/// The entry that a page left out, read by its index, as the CLI does
/// without `--omit-payloads`.
async fn oplog_entry(seed: &Seed) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.worker()
            .get_oplog_entry(&seed.component.id.0, seed.agent_name(), 1, None)
            .await,
    )
}

async fn describe(seed: &Seed, hosted: bool) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.agent()
            .describe_tool(&NativeToolDescribeRequest {
                app_name: seed.application.clone(),
                env_name: seed.environment.clone(),
                agent_id: hosted.then(|| seed.target_id.clone()),
                component_id: (!hosted).then_some(seed.component.id.0),
                tool_name: TOOL.to_string(),
            })
            .await,
    )
}

async fn invoke_tool(
    seed: &Seed,
    hosted: bool,
    mode: NativeToolInvocationMode,
    idempotency_key: &str,
) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.agent()
            .invoke_tool(
                Some(idempotency_key),
                &tool_request(seed, hosted, mode, idempotency_key)?,
            )
            .await,
    )
}

async fn cancel(seed: &Seed, idempotency_key: &str) -> anyhow::Result<Outcome> {
    seed.rest(
        seed.worker()
            .cancel_invocation(&seed.component.id.0, seed.agent_name(), idempotency_key)
            .await,
    )
}

/// What the user, or the CLI, does to a running `ssh` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Nothing: the command runs `SCRIPT` to its result.
    None,
    /// The user stops a script that runs longer than the CLI's wait while it
    /// runs, and the CLI cancels the accepted invocation while the session
    /// is open.
    Cancel,
    /// As `Cancel`, and the session ends without the result, so the CLI
    /// looks it up: the script finishes within the CLI's wait.
    CancelAndLookUp,
    /// As `CancelAndLookUp`, but the script has no result within the wait,
    /// so the outcome is unknown and the CLI reads the agent's state to
    /// report it.
    OutcomeUnknown,
}

/// The script of a stopped command: it writes `marker` when it runs, and
/// then runs for `seconds`. The owner reads the marker through the agent's
/// files, because `bash` runs in the agent's file system. Each command has
/// its own marker, so that a command of an earlier run cannot pass for it.
fn stopped_script(marker: &str, seconds: u32) -> String {
    format!("touch {marker} && sleep {seconds}")
}
/// How long the CLI waits for the result of a stopped command
/// (`CANCEL_WAIT` of `golem ssh`).
const RESULT_WAIT: Duration = Duration::from_secs(5);
/// A lookup that is still waiting when the CLI gives up on it. A denial
/// answers at once.
const NO_RESULT: &str = "no result within the wait";

/// One command of an `ssh` session (`session`) or script: the description,
/// the read of the agent's state at a session's prompt, the start, and then
/// what `stop` does on the accepted invocation. A stop comes only after the
/// owner sees that the script runs, so that the invocation is not still
/// queued. The request at the trace position that `Seed::missing_invocation`
/// names addresses that missing invocation of the agent instead.
async fn ssh_command(seed: &Seed, session: bool, stop: Stop) -> anyhow::Result<Trace> {
    let mut trace = Trace::default();
    trace.push(Step::Request(
        describe(&seed.at(trace.outcomes.len()), true).await?,
    ));
    if trace.stopped {
        return Ok(trace);
    }
    if session {
        // The state read is best effort: a denial does not stop the command.
        trace
            .outcomes
            .push(match metadata(&seed.at(trace.outcomes.len())).await? {
                Step::Request(outcome) | Step::Prepare(outcome) => outcome,
            });
    }
    let key = uuid::Uuid::new_v4().to_string();
    let marker = format!("/management-started-{key}");
    // A stopped command that finishes within the wait finishes well inside
    // it; one that has no result outlasts it.
    let script = match stop {
        Stop::None => SCRIPT.to_string(),
        Stop::CancelAndLookUp => stopped_script(&marker, 2),
        Stop::Cancel | Stop::OutcomeUnknown => stopped_script(&marker, 15),
    };
    let starting = seed.at(trace.outcomes.len());
    let mut live = open_session(
        &starting.caller,
        tool_start(&starting, hosted_target(&starting), &key, &script)?,
    )
    .await?;
    let (started, accepted) = receive(&mut live, stop == Stop::None).await?;
    trace.push(Step::Request(started));
    if trace.stopped || stop == Stop::None {
        return Ok(trace);
    }
    let accepted = accepted
        .expect("an accepted start has its key")
        .idempotency_key;
    seed.wait_until_the_script_runs(&marker).await?;
    'stop: {
        let cancelling = seed.at(trace.outcomes.len());
        let cancelled = cancel(
            &cancelling,
            &cancelling.addressed(trace.outcomes.len(), &accepted),
        )
        .await?;
        trace.push(Step::Request(cancelled));
        if trace.stopped || stop == Stop::Cancel {
            break 'stop;
        }
        let looking_up = seed.at(trace.outcomes.len());
        let addressed = looking_up.addressed(trace.outcomes.len(), &accepted);
        let looked_up = match tokio::time::timeout(
            RESULT_WAIT,
            invoke_tool(
                &looking_up,
                true,
                NativeToolInvocationMode::Lookup,
                &addressed,
            ),
        )
        .await
        {
            Ok(outcome) => outcome?,
            Err(_) => Outcome::Allowed(NO_RESULT.to_string()),
        };
        let found = matches!(&looked_up, Outcome::Allowed(result) if result != NO_RESULT);
        // Without a result, also after a denied lookup, the CLI reports an
        // unknown outcome with the agent's state.
        trace.outcomes.push(looked_up);
        if !found {
            trace.push(metadata(&seed.at(trace.outcomes.len())).await?);
        }
    }
    drop(live);
    // The next run on the same agent, such as the unknown-target
    // comparison, starts only after this command has ended.
    seed.wait_until_the_command_ends(&accepted).await?;
    Ok(trace)
}

type Run = for<'a> fn(&'a Seed) -> BoxFuture<'a, anyhow::Result<Trace>>;

/// The requests of one variant of one mode of a command row, as the caller.
struct Adapter {
    path: &'static [&'static str],
    mode: &'static str,
    variant: &'static str,
    /// The indexes of the mode's requests that the adapter makes, in order.
    makes: &'static [usize],
    /// The effect of this variant, when it differs from the case's.
    effect: Option<Effect>,
    run: Run,
}

macro_rules! adapter {
    ([$($segment:literal),+], $mode:literal, $variant:literal, [$($index:literal),+], $(effect: $effect:expr,)? |$seed:ident| $body:expr) => {
        Adapter {
            path: &[$($segment),+],
            mode: $mode,
            variant: $variant,
            makes: &[$($index),+],
            effect: adapter!(@effect $($effect)?),
            run: |$seed| Box::pin(async move { $body }),
        }
    };
    (@effect) => {
        None
    };
    (@effect $effect:expr) => {
        Some($effect)
    };
}

/// A stopped `ssh` command started its invocation and has no result to
/// read. What the stop does to the running `bash` process is the tool's,
/// not a decision of a grant.
const STOPPED: Effect = Effect::Reads("InvocationAccepted");

const ADAPTERS: &[Adapter] = &[
    // The CLI reads the metadata of the agent that it creates and goes on
    // when it looks unknown.
    adapter!(
        ["agent", "new"],
        "create",
        "create",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare(seed, &seed.new_agent.to_string()).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .launch_new_worker(
                            &seed.component.id.0,
                            &AgentCreationRequest {
                                name: seed.new_agent.to_string(),
                                env: Default::default(),
                                config: Vec::new(),
                            },
                        )
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "start",
        "no-stream",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async { Ok(start_session(seed, &seed.caller, CALL_ITEM, true).await?.0) })
                .await?,
        )
    ),
    // `--stdout-format raw` opens no output connection either; the CLI
    // allows it for a method with a stream result.
    adapter!(
        ["agent", "invoke"],
        "start",
        "raw-output",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async { Ok(start_session(seed, &seed.caller, CALL_ITEM, true).await?.0) })
                .await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "start",
        "stream",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(connect(seed)).await?,
            request(async { Ok(start_session(seed, &seed.caller, CALL_ITEM, true).await?.0) })
                .await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "trigger",
        "no-stream",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(invoke(seed, AgentInvocationMode::Schedule)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "trigger",
        "stream",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(connect(seed)).await?,
            request(invoke(seed, AgentInvocationMode::Schedule)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "resume-session",
        "no-stream",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(resume_session(seed, PublicResumeOperation::Resume)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "resume-session",
        "raw-output",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(resume_session(seed, PublicResumeOperation::Resume)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "resume-session",
        "stream",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(connect(seed)).await?,
            request(resume_session(seed, PublicResumeOperation::Resume)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "takeover-session",
        "no-stream",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(resume_session(seed, PublicResumeOperation::Takeover)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "takeover-session",
        "raw-output",
        [0, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(resume_session(seed, PublicResumeOperation::Takeover)).await?,
        )
    ),
    adapter!(
        ["agent", "invoke"],
        "takeover-session",
        "stream",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(connect(seed)).await?,
            request(resume_session(seed, PublicResumeOperation::Takeover)).await?,
        )
    ),
    adapter!(["agent", "get"], "get", "get", [0, 1], |seed| in_order!(
        seed;
        prepare_target(seed).await?,
        metadata(seed).await?,
    )),
    adapter!(
        ["agent", "delete"],
        "delete",
        "delete",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .delete_worker(&seed.component.id.0, seed.agent_name())
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(["agent", "list"], "list", "list", [0], |seed| in_order!(
        seed;
        request(async {
            seed.rest(
                seed.worker()
                    .find_workers_metadata(
                        &seed.component.id.0,
                        &WorkersMetadataRequest {
                            filter: None,
                            cursor: None,
                            count: Some(10),
                            precise: None,
                        },
                    )
                    .await,
            )
        })
        .await?
    )),
    adapter!(
        ["agent", "stream"],
        "follow",
        "follow",
        [0, 1],
        |seed| in_order!(seed; prepare_target(seed).await?, request(connect(seed)).await?,)
    ),
    // The connection names the agent of the REPL's invocation; the
    // invocation that the stdin line selects stays on the client today.
    adapter!(
        ["agent", "repl-stream"],
        "follow",
        "follow",
        [0],
        |seed| in_order!(seed; request(connect(seed)).await?)
    ),
    adapter!(
        ["agent", "update"],
        "automatic",
        "no-await",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(update(seed, AgentUpdateMode::Automatic)).await?,
        )
    ),
    adapter!(
        ["agent", "update"],
        "automatic",
        "await",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(update(seed, AgentUpdateMode::Automatic)).await?,
            request(await_update(seed)).await?,
        )
    ),
    adapter!(
        ["agent", "update"],
        "manual",
        "no-await",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(update(seed, AgentUpdateMode::Manual)).await?,
        )
    ),
    adapter!(
        ["agent", "update"],
        "manual",
        "await",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(update(seed, AgentUpdateMode::Manual)).await?,
            request(await_update(seed)).await?,
        )
    ),
    adapter!(
        ["agent", "interrupt"],
        "interrupt",
        "interrupt",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .interrupt_worker(&seed.component.id.0, seed.agent_name(), Some(false))
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "resume"],
        "resume",
        "resume",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .resume_worker(&seed.component.id.0, seed.agent_name())
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "simulate-crash"],
        "simulate-crash",
        "simulate-crash",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .interrupt_worker(&seed.component.id.0, seed.agent_name(), Some(true))
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "oplog"],
        "page",
        "omit-payloads",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(oplog_page(seed, None)).await?,
        )
    ),
    adapter!(
        ["agent", "oplog"],
        "page",
        "payloads",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(oplog_page(seed, None)).await?,
            request(oplog_entry(seed)).await?,
        )
    ),
    adapter!(
        ["agent", "oplog"],
        "search",
        "omit-payloads",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(oplog_page(seed, Some("create"))).await?,
        )
    ),
    adapter!(
        ["agent", "oplog"],
        "search",
        "payloads",
        [0, 1, 2],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(oplog_page(seed, Some("create"))).await?,
            request(oplog_entry(seed)).await?,
        )
    ),
    adapter!(
        ["agent", "revert"],
        "to-oplog-index",
        "to-oplog-index",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(revert(
                seed,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: seed.before_seed_item,
                })
            ))
            .await?,
        )
    ),
    adapter!(
        ["agent", "revert"],
        "by-invocation-count",
        "by-invocation-count",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(revert(
                seed,
                RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                    number_of_invocations: 1,
                })
            ))
            .await?,
        )
    ),
    adapter!(
        ["agent", "cancel-invocation"],
        "cancel",
        "cancel",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(cancel(seed, &seed.scheduled_invocation)).await?,
        )
    ),
    adapter!(
        ["agent", "files"],
        "list",
        "list",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .get_files(&seed.component.id.0, seed.agent_name(), "/")
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "file-contents"],
        "read",
        "read",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .get_file_content(&seed.component.id.0, seed.agent_name(), "/foo.txt")
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "activate-plugin"],
        "activate",
        "activate",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .activate_plugin(&seed.component.id.0, seed.agent_name(), 0)
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(
        ["agent", "deactivate-plugin"],
        "deactivate",
        "deactivate",
        [0, 1],
        |seed| in_order!(
            seed;
            prepare_target(seed).await?,
            request(async {
                seed.rest(
                    seed.worker()
                        .deactivate_plugin(&seed.component.id.0, seed.agent_name(), 0)
                        .await,
                )
            })
            .await?,
        )
    ),
    adapter!(["ssh"], "session", "runs", [0, 1, 2], |seed| ssh_command(
        seed,
        true,
        Stop::None
    )
    .await),
    adapter!(
        ["ssh"],
        "session",
        "stopped",
        [0, 1, 2, 4],
        effect: STOPPED,
        |seed| ssh_command(seed, true, Stop::Cancel).await
    ),
    adapter!(
        ["ssh"],
        "session",
        "stopped-without-result",
        [0, 1, 2, 4, 5],
        effect: STOPPED,
        |seed| ssh_command(seed, true, Stop::CancelAndLookUp).await
    ),
    adapter!(
        ["ssh"],
        "session",
        "outcome-unknown",
        [0, 1, 2, 4, 5, 6],
        effect: STOPPED,
        |seed| ssh_command(seed, true, Stop::OutcomeUnknown).await
    ),
    adapter!(["ssh"], "script", "runs", [0, 1], |seed| ssh_command(
        seed,
        false,
        Stop::None
    )
    .await),
    adapter!(
        ["ssh"],
        "script",
        "stopped",
        [0, 1, 3],
        effect: STOPPED,
        |seed| ssh_command(seed, false, Stop::Cancel).await
    ),
    adapter!(
        ["ssh"],
        "script",
        "stopped-without-result",
        [0, 1, 3, 4],
        effect: STOPPED,
        |seed| ssh_command(seed, false, Stop::CancelAndLookUp).await
    ),
    adapter!(
        ["ssh"],
        "script",
        "outcome-unknown",
        [0, 1, 3, 4, 5],
        effect: STOPPED,
        |seed| ssh_command(seed, false, Stop::OutcomeUnknown).await
    ),
    adapter!(
        ["tool", "invoke"],
        "hosted-start",
        "await",
        [0, 1],
        |seed| in_order!(
            seed;
            request(describe(seed, true)).await?,
            request(invoke_tool(
                seed,
                true,
                NativeToolInvocationMode::Await,
                &uuid::Uuid::new_v4().to_string()
            ))
            .await?,
        )
    ),
    adapter!(
        ["tool", "invoke"],
        "standalone-start",
        "await",
        [0, 1],
        |seed| in_order!(
            seed;
            request(describe(seed, false)).await?,
            request(invoke_tool(
                seed,
                false,
                NativeToolInvocationMode::Await,
                &uuid::Uuid::new_v4().to_string()
            ))
            .await?,
        )
    ),
    adapter!(
        ["tool", "invoke"],
        "hosted-help",
        "help",
        [0],
        |seed| in_order!(seed; request(describe(seed, true)).await?)
    ),
    adapter!(
        ["tool", "invoke"],
        "standalone-help",
        "help",
        [0],
        |seed| in_order!(seed; request(describe(seed, false)).await?)
    ),
    // The owner's earlier tool invocation is looked up under its key.
    adapter!(
        ["tool", "invoke"],
        "lookup",
        "lookup",
        [0],
        |seed| in_order!(
            seed;
            request(invoke_tool(
                seed,
                true,
                NativeToolInvocationMode::Lookup,
                &seed.scheduled_invocation
            ))
            .await?
        )
    ),
];

fn adapters(path: &[&str], mode: &str) -> Vec<&'static Adapter> {
    ADAPTERS
        .iter()
        .filter(|adapter| adapter.path == path && adapter.mode == mode)
        .collect()
}

fn case_of(path: &[&str], mode: &str) -> &'static Case {
    CASES
        .iter()
        .find(|case| case.path == path && case.mode == mode)
        .unwrap_or_else(|| panic!("`{}` mode `{mode}` has no case", path.join(" ")))
}

// ── Grants ──────────────────────────────────────────────────────────────

/// Installs a card with exactly these grants into the caller's wallet.
///
/// The grants are 2.0 grant text: the 2.0 grammar (GOL-1275) with the 2.0
/// class catalog (GOL-1276), installed through a 2.0 permission share
/// (GOL-1297). Until those land there is no way to install them, and this
/// refuses rather than install 1.6 grants under 2.0 verbs.
async fn give(seed: &Seed, grants: &[Grant]) -> anyhow::Result<()> {
    let grants = grants
        .iter()
        .map(|grant| {
            format!(
                "{} : {} on {:?} of {} : {}",
                grant.class,
                grant.verb,
                grant.on,
                seed.agent_name(),
                grant.resource
            )
        })
        .collect::<Vec<_>>();
    anyhow::bail!(
        "pending: installing 2.0 grants needs GOL-1275, GOL-1276 and GOL-1297 (Aditya Salunkhe); the grants: {grants:?}"
    )
}

// ── Coverage ────────────────────────────────────────────────────────────

fn key(path: &[&str], mode: &str) -> (String, String) {
    (path.join(" "), mode.to_string())
}

/// Every mode of the command table has one case that classifies it as the
/// plan does, and adapters that make each of its requests, except the named
/// pending ones. The read-only case covers every control and exec mode and
/// no read mode.
#[test]
fn management_every_command_mode_has_an_adapter_and_a_case() {
    let mut problems = Vec::new();
    let modes = COMMAND_TABLE
        .iter()
        .flat_map(|row| row.modes.iter().map(move |mode| (row, mode)))
        .collect::<Vec<_>>();
    let mode_keys = modes
        .iter()
        .map(|(row, mode)| key(row.path, mode.name))
        .collect::<BTreeSet<_>>();

    let mut case_keys = BTreeSet::new();
    for case in CASES {
        if !case_keys.insert(key(case.path, case.mode)) {
            problems.push(format!(
                "`{}` mode `{}` has two cases",
                case.path.join(" "),
                case.mode
            ));
        }
    }
    for (path, mode) in mode_keys.difference(&case_keys) {
        problems.push(format!("`{path}` mode `{mode}` has no case"));
    }
    for (path, mode) in case_keys.difference(&mode_keys) {
        problems.push(format!("the case of `{path}` mode `{mode}` has no row"));
    }

    let mut adapter_keys = BTreeSet::new();
    for adapter in ADAPTERS {
        let (path, mode) = key(adapter.path, adapter.mode);
        if !adapter_keys.insert((path.clone(), mode.clone(), adapter.variant)) {
            problems.push(format!(
                "`{path}` mode `{mode}` has two adapters `{}`",
                adapter.variant
            ));
        }
        if !mode_keys.contains(&(path.clone(), mode.clone())) {
            problems.push(format!("the adapter of `{path}` mode `{mode}` has no row"));
        }
    }

    for (row, mode) in &modes {
        let at = format!("`{}` mode `{}`", row.path.join(" "), mode.name);
        let Some(case) = CASES
            .iter()
            .find(|case| case.path == row.path && case.mode == mode.name)
        else {
            continue;
        };
        let selected_by = spell_selectors(mode.selected_by, ", ");
        if case.selected_by != selected_by {
            problems.push(format!(
                "{at} is selected by `{selected_by}`, the plan says `{}`",
                case.selected_by
            ));
        }
        if case.class != mode.class {
            problems.push(format!(
                "{at} is {:?}, the plan says {:?}",
                mode.class, case.class
            ));
        }
        let actual = mode.requests.iter().map(spell_request).collect::<Vec<_>>();
        let expected = case
            .requests
            .iter()
            .map(|request| request.spec)
            .collect::<Vec<_>>();
        if actual != expected {
            problems.push(format!(
                "{at} makes\n    {}\n  the plan says\n    {}",
                actual.join("\n    "),
                expected.join("\n    ")
            ));
            continue;
        }
        for (index, request) in case.requests.iter().enumerate() {
            if request.grants.is_empty() {
                problems.push(format!("{at} request {index} has no grants"));
            }
        }

        let mode_adapters = adapters(row.path, mode.name);
        if mode_adapters.is_empty() {
            problems.push(format!("{at} has no adapter"));
        }
        let mut made = BTreeSet::new();
        for adapter in &mode_adapters {
            let in_order = adapter.makes.windows(2).all(|pair| pair[0] < pair[1]);
            if !in_order
                || adapter
                    .makes
                    .iter()
                    .any(|index| *index >= mode.requests.len())
            {
                problems.push(format!(
                    "{at} adapter `{}` makes requests out of order or that the mode lacks",
                    adapter.variant
                ));
                continue;
            }
            for (index, request) in mode.requests.iter().enumerate() {
                if request.when == When::Always && !adapter.makes.contains(&index) {
                    problems.push(format!(
                        "{at} adapter `{}` skips request {index}, which the mode always makes",
                        adapter.variant
                    ));
                }
            }
            made.extend(adapter.makes.iter().copied());
        }
        for index in 0..mode.requests.len() {
            let pending = PENDING_REQUESTS.iter().any(|(path, name, pending, _)| {
                *path == row.path && *name == mode.name && *pending == index
            });
            match (made.contains(&index), pending) {
                (false, false) => problems.push(format!("{at} request {index} has no adapter")),
                (true, true) => problems.push(format!(
                    "{at} request {index} has an adapter but is still pending"
                )),
                _ => {}
            }
        }
    }
    for (path, mode, index, _) in PENDING_REQUESTS {
        if !modes.iter().any(|(row, m)| {
            row.path == *path
                && m.name == *mode
                && m.requests
                    .get(*index)
                    .is_some_and(|r| r.when != When::Always)
        }) {
            problems.push(format!(
                "the pending request {index} of `{}` mode `{mode}` is not a conditional request",
                path.join(" ")
            ));
        }
    }

    let read_only = CONTROLS_AND_STARTS
        .iter()
        .map(|(path, mode)| key(path, mode))
        .collect::<BTreeSet<_>>();
    for (row, mode) in &modes {
        let at = key(row.path, mode.name);
        let covered = read_only.contains(&at);
        let starts = mode.class == CommandClass::InvocationGrant
            && mode.requests.iter().any(|request| {
                request.stage == Stage::Operation
                    && request.parts.iter().any(|part| {
                        matches!(part.target, Target::Agent | Target::Tool)
                            && spell_part(part).contains(" invoke ")
                    })
            });
        let must_cover = matches!(
            mode.class,
            CommandClass::ManagementControl | CommandClass::Exec
        ) || starts;
        if must_cover && !covered {
            problems.push(format!(
                "`{}` mode `{}` is missing from the read-only case",
                at.0, at.1
            ));
        }
        if mode.class == CommandClass::ManagementRead && covered {
            problems.push(format!(
                "`{}` mode `{}` is a read in the read-only case",
                at.0, at.1
            ));
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// ── Acceptance ──────────────────────────────────────────────────────────

/// The trace position of the operation of `case` in a trace of `adapter`.
fn operation_position(case: &Case, adapter: &Adapter) -> usize {
    let operation = case
        .requests
        .iter()
        .position(|request| request.spec.starts_with("operation"))
        .expect("every case has an operation");
    adapter
        .makes
        .iter()
        .position(|index| *index == operation)
        .expect("every adapter makes the operation")
}

/// Whether the effect of a mode with all its grants happened.
async fn check_effect(
    seed: &Seed,
    case: &Case,
    adapter: &Adapter,
    before: &Observation,
    trace: &Trace,
) -> anyhow::Result<()> {
    let at = format!(
        "`{}` mode `{}` variant `{}`",
        case.path.join(" "),
        case.mode,
        adapter.variant
    );
    let position = operation_position(case, adapter);
    let effect = adapter.effect.unwrap_or(case.effect);
    let effect_happened = |after: &Observation| {
        let target = |observation: &Observation| observation.target.clone();
        match effect {
            Effect::Reads(_) | Effect::Pending(_) => true,
            Effect::Created => !before.created && after.created,
            Effect::Deleted => target(before).is_some() && target(after).is_none(),
            Effect::Updated => {
                target(before).is_some_and(|state| state.revision != seed.next_revision)
                    && target(after).is_some_and(|state| state.revision == seed.next_revision)
            }
            Effect::Interrupted => {
                target(after).is_some_and(|state| state.status == AgentStatus::Interrupted)
            }
            Effect::Resumed => {
                target(before).is_some_and(|state| state.status == AgentStatus::Interrupted)
                    && target(after).is_some_and(|state| state.status != AgentStatus::Interrupted)
            }
            Effect::Invoked => after.caller_invocations > before.caller_invocations,
            Effect::Reverted => match (target(before), target(after)) {
                (Some(before), Some(after)) => after.deleted_regions > before.deleted_regions,
                _ => false,
            },
            Effect::Cancelled => match (target(before), target(after)) {
                (Some(before), Some(after)) => {
                    after.pending_invocations + 1 == before.pending_invocations
                }
                _ => false,
            },
            Effect::ProcessorActive(active) => {
                target(after).is_some_and(|state| (state.active_processors > 0) == active)
            }
        }
    };
    if let Effect::Reads(text) = effect {
        match trace.outcomes.get(position) {
            Some(Outcome::Allowed(payload)) => assert!(
                payload.contains(text),
                "{at} did not read the seeded `{text}`: {payload}"
            ),
            other => panic!("{at} was not allowed: {other:?}"),
        }
    }
    for _ in 0..10 {
        if effect_happened(&seed.observe().await?) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("{at} did not have its effect {effect:?}")
}

/// Each request of each mode is allowed with its grants, and denied without
/// any one of them, with the same response as an unknown target where the
/// target is a Rule 21 kind. A denied operation changes nothing. A denied
/// metadata read of the named agent does not stop the command, as the CLI
/// goes on with the component's current revision.
#[test]
#[timeout("60m")]
#[ignore = "pending: the 2.0 card model (GOL-1270, GOL-1271, GOL-1272, GOL-1276, GOL-1288, GOL-1289) and 2.0 grant installation (GOL-1275, GOL-1297), all Aditya Salunkhe; the creation contract (GOL-1296); the invocation entry points (GOL-1051); the standalone tool invocation (GOL-1079); the `bash` fixture (remaining GOL-1403 work)"]
async fn management_every_operation_is_allowed_with_its_grants_and_looks_unknown_without_them(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    for case in CASES {
        for adapter in adapters(case.path, case.mode) {
            let made = adapter
                .makes
                .iter()
                .map(|index| &case.requests[*index])
                .collect::<Vec<_>>();
            let mut all = Vec::<Grant>::new();
            for (grant, _) in made.iter().flat_map(|request| request.grants) {
                if !all.contains(grant) {
                    all.push(*grant);
                }
            }
            // Each distinct grant is removed with every request that needs
            // it, so that no equivalent grant stays installed.
            for missing in &all {
                let seed = Seed::new(deps, case.fixture).await?;
                seed.put_in(case.precondition).await?;
                let without = all
                    .iter()
                    .filter(|grant| *grant != missing)
                    .copied()
                    .collect::<Vec<_>>();
                give(&seed, &without).await?;
                let before = seed.observe().await?;
                let trace = (adapter.run)(&seed).await?;
                let at = format!(
                    "`{}` mode `{}` variant `{}` without {missing:?}",
                    case.path.join(" "),
                    case.mode,
                    adapter.variant
                );
                let needing = made
                    .iter()
                    .enumerate()
                    .filter_map(|(position, request)| {
                        request
                            .grants
                            .iter()
                            .find(|(grant, _)| grant == missing)
                            .map(|(_, looks_like)| (position, *request, *looks_like))
                    })
                    .collect::<Vec<_>>();
                if needing
                    .iter()
                    .any(|(_, request, _)| request.spec.starts_with("operation"))
                {
                    assert_eq!(seed.observe().await?, before, "{at} changed the target");
                }
                let stopped_by_it = trace.stopped
                    && needing
                        .iter()
                        .any(|(position, _, _)| *position + 1 == trace.outcomes.len());
                for (position, _, looks_like) in &needing {
                    if let LooksLike::Pending(_) = looks_like {
                        continue;
                    }
                    let Some(outcome) = trace.outcomes.get(*position) else {
                        // An earlier request that needs the same grant was
                        // denied and stopped the command.
                        assert!(stopped_by_it, "{at} did not make request {position}");
                        continue;
                    };
                    let denied = match outcome {
                        Outcome::Denied(denial) => denial.clone(),
                        other => panic!("{at} request {position} was not denied: {other:?}"),
                    };
                    match looks_like {
                        LooksLike::UnknownAgent | LooksLike::UnknownInvocation => {
                            let unknown = if *looks_like == LooksLike::UnknownAgent {
                                seed.with_unknown_agent_at(*position)
                            } else {
                                seed.with_unknown_invocation_at(*position)
                            };
                            let unknown = (adapter.run)(&unknown).await?;
                            assert_eq!(
                                unknown.outcomes.get(*position),
                                Some(&Outcome::Denied(denied)),
                                "{at} request {position} differs from an unknown target"
                            );
                        }
                        LooksLike::Forbidden => assert!(
                            matches!(denied, Denial::Http(RawResponse { status: 403, .. })),
                            "{at} request {position} is not a 403 on a tool by name: {denied:?}"
                        ),
                        LooksLike::Pending(_) => {}
                    }
                }
            }
            let seed = Seed::new(deps, case.fixture).await?;
            seed.put_in(case.precondition).await?;
            give(&seed, &all).await?;
            let before = seed.observe().await?;
            let trace = (adapter.run)(&seed).await?;
            let as_expected = trace.outcomes.len() == made.len()
                && trace
                    .outcomes
                    .iter()
                    .zip(&made)
                    .all(|(outcome, request)| match outcome {
                        Outcome::Allowed(_) => !request.finds_nothing,
                        Outcome::Denied(Denial::Http(RawResponse { status: 404, .. })) => {
                            request.finds_nothing
                        }
                        Outcome::Denied(_) => false,
                    });
            assert!(
                as_expected,
                "`{}` mode `{}` variant `{}` was denied with its grants: {:?}",
                case.path.join(" "),
                case.mode,
                adapter.variant,
                trace.outcomes
            );
            check_effect(&seed, case, adapter, &before, &trace).await?;
        }
    }
    Ok(())
}

/// `view`, `oplog : read` and `tool : view` neither control nor invoke, also
/// through REST calls that no client preflight guards. Each tested variant
/// makes only reads that these grants allow before its operation, so that
/// the operation itself is what is denied. A variant that also needs another
/// grant before its operation is not tested here: the stream variants of
/// `agent invoke` open the output connection first, and whether a connection
/// without `invocation : read` is refused is open (`EXPOSED_OUTPUT`). Each
/// mode has a variant without it that makes the same operation.
#[test]
#[timeout("30m")]
#[ignore = "pending: the 2.0 card model (GOL-1270, GOL-1271, GOL-1272, GOL-1276, GOL-1288, GOL-1289) and 2.0 grant installation (GOL-1275, GOL-1297), all Aditya Salunkhe"]
async fn management_read_grants_cannot_control_or_invoke(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let read_grants = [VIEW, OPLOG_READ.0, TOOL_VIEW.0];
    for (path, mode) in CONTROLS_AND_STARTS {
        let case = case_of(path, mode);
        let mut tested = 0;
        for adapter in adapters(path, mode) {
            let operation = operation_position(case, adapter);
            let needs_another_grant = adapter.makes[..operation].iter().any(|index| {
                case.requests[*index]
                    .grants
                    .iter()
                    .any(|(grant, _)| !read_grants.contains(grant))
            });
            if needs_another_grant {
                continue;
            }
            tested += 1;
            let seed = Seed::new(deps, case.fixture).await?;
            seed.put_in(case.precondition).await?;
            give(&seed, &read_grants).await?;
            let before = seed.observe().await?;
            let trace = (adapter.run)(&seed).await?;
            let operation = operation_position(case, adapter);
            assert!(
                matches!(trace.outcomes.get(operation), Some(Outcome::Denied(_))),
                "`{}` mode `{mode}` variant `{}` did not have its operation denied with read grants: {:?}",
                path.join(" "),
                adapter.variant,
                trace.outcomes
            );
            assert_eq!(
                seed.observe().await?,
                before,
                "`{}` mode `{mode}` variant `{}` changed the target",
                path.join(" "),
                adapter.variant
            );
        }
        assert!(
            tested > 0,
            "`{}` mode `{mode}` has no variant whose operation the read grants reach",
            path.join(" ")
        );
    }
    Ok(())
}

/// Cancelling needs the invocation's `cancel` grant; `read` is not enough. A
/// denied cancel leaves the invocation in place. A `cancel` grant on one
/// invocation does not cancel another invocation of the same agent.
#[test]
#[timeout("10m")]
#[ignore = "pending: the invocation class (GOL-1276), 2.0 grant installation (GOL-1275, GOL-1297) and the 2.0 card model (GOL-1270, GOL-1271, GOL-1272, GOL-1288, GOL-1289), all Aditya Salunkhe; the invocation cancellation entry point (GOL-1051)"]
async fn management_cancel_needs_the_invocation_cancel_grant(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let seed = Seed::new(deps, Fixture::Repository).await?;
    give(
        &seed,
        &[VIEW, grant("invocation", "read", On::TargetInvocations, "")],
    )
    .await?;
    let denied = cancel(&seed, &seed.scheduled_invocation).await?;
    let unknown = seed.with_unknown_invocation();
    assert_eq!(
        denied,
        cancel(&unknown, &unknown.scheduled_invocation).await?,
        "a cancel under a read grant differs from a cancel of an unknown invocation"
    );

    give(
        &seed,
        &[grant("invocation", "cancel", On::ScheduledInvocation, "")],
    )
    .await?;
    let other = cancel(&seed, &seed.other_scheduled_invocation).await?;
    assert_eq!(
        other,
        cancel(&unknown, &unknown.other_scheduled_invocation).await?,
        "a cancel grant on one invocation reached another one"
    );
    let cancelled = cancel(&seed, &seed.scheduled_invocation).await?;
    assert!(
        matches!(&cancelled, Outcome::Allowed(response) if response.contains("canceled: true")),
        "the denied cancel already removed the invocation: {cancelled:?}"
    );
    Ok(())
}

/// A type-wide `replay` grant updates and deletes a sandbox fork of the type,
/// and neither updates, automatically or manually, nor deletes an agent of
/// the type without the mark, nor a fork of another type. The manual update
/// of a marked fork, the restricted snapshot-migration rehearsal, is not a
/// positive case here: its admission waits for the executor review of its
/// host-call safety, exact idle, restart behavior and supported cases, and
/// for the rehearsal itself (GOL-2143, `MANUAL_UPDATE_OF_A_MARKED_AGENT`).
#[test]
#[timeout("10m")]
#[ignore = "pending: the never-live mark and the fork that sets it (GOL-1420); the replay verb (GOL-1276), 2.0 grant installation (GOL-1275, GOL-1297) and the 2.0 card model (GOL-1289), all Aditya Salunkhe"]
async fn management_replay_cannot_change_a_real_agent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let update = adapters(&["agent", "update"], "automatic")[0];
    let manual = adapters(&["agent", "update"], "manual")[0];
    let delete = adapters(&["agent", "delete"], "delete")[0];
    let seed = Seed::new(deps, Fixture::Repository).await?;
    give(
        &seed,
        &[
            grant("agent", "view", On::TargetType, ""),
            grant("agent", "replay", On::TargetType, ""),
        ],
    )
    .await?;
    for real in [update, manual, delete] {
        let before = seed.observe().await?;
        let trace = (real.run)(&seed).await?;
        let unknown = (real.run)(&seed.with_unknown_agent()).await?;
        assert_eq!(trace.outcomes.get(1), unknown.outcomes.get(1));
        assert_eq!(seed.observe().await?, before, "replay changed a real agent");
    }
    anyhow::bail!(
        "pending: the marked half of this case needs a sandbox fork with the never-live mark, which GOL-1420 supplies, and a fork of another type as the negative owner control; its manual rehearsal waits for the executor review and for the rehearsal itself (GOL-2143)"
    )
}

/// A tool that reads static `File` config fields needs the tool-wide
/// `invoke : *` beside the grant for the command: a grant for the command
/// alone is a `403`, and the wildcard allows it.
#[test]
#[timeout("10m")]
#[ignore = "pending: the tool admission and its fixture (GOL-1086, Daniel Vigovszky); 2.0 grant installation (GOL-1275, GOL-1297) and the 2.0 card model (GOL-1289), Aditya Salunkhe"]
async fn management_static_file_config_needs_the_tool_wide_invoke(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let start = adapters(&["tool", "invoke"], "hosted-start")[0];
    let narrow = Seed::new(deps, Fixture::StaticFileConfigTool).await?;
    give(
        &narrow,
        &[
            VIEW,
            TOOL_VIEW.0,
            grant("agent", "exec", On::Target, ""),
            TOOL_INVOKE_RUN.0,
        ],
    )
    .await?;
    let trace = (start.run)(&narrow).await?;
    assert!(
        matches!(
            trace.outcomes.get(1),
            Some(Outcome::Denied(Denial::Http(RawResponse {
                status: 403,
                ..
            })))
        ),
        "a grant for the command alone started a tool that reads static `File` config: {:?}",
        trace.outcomes
    );
    let wildcard = Seed::new(deps, Fixture::StaticFileConfigTool).await?;
    give(
        &wildcard,
        &[
            VIEW,
            TOOL_VIEW.0,
            grant("agent", "exec", On::Target, ""),
            TOOL_INVOKE_RUN.0,
            grant("tool", "invoke", On::Tool, "*"),
        ],
    )
    .await?;
    let trace = (start.run)(&wildcard).await?;
    assert!(
        matches!(trace.outcomes.get(1), Some(Outcome::Allowed(_))),
        "the tool-wide `invoke : *` did not start the tool: {:?}",
        trace.outcomes
    );
    Ok(())
}

/// `agent list` returns the agents that the caller's `view` covers and no
/// other agent, with success.
#[test]
#[timeout("10m")]
#[ignore = "pending: `scope` and `filter` of a listing (GOL-1404); 2.0 grant installation (GOL-1275, GOL-1297) and the 2.0 card model (GOL-1289), Aditya Salunkhe"]
async fn management_list_returns_only_visible_agents(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let list = adapters(&["agent", "list"], "list")[0];
    let seed = Seed::new(deps, Fixture::Repository).await?;
    let hidden = agent_id!(AGENT_TYPE, "management-hidden");
    seed.owner
        .start_agent(&seed.component.id, hidden.clone())
        .await?;
    give(&seed, &[VIEW]).await?;
    let trace = (list.run)(&seed).await?;
    match trace.outcomes.first() {
        Some(Outcome::Allowed(rows)) => {
            assert!(
                rows.contains(seed.agent_name()),
                "the visible agent is missing: {rows}"
            );
            assert!(
                !rows.contains(&hidden.to_string()),
                "the listing shows a hidden agent: {rows}"
            );
        }
        other => panic!("the listing failed: {other:?}"),
    }
    give(&seed, &[]).await?;
    let trace = (list.run)(&seed).await?;
    match trace.outcomes.first() {
        Some(Outcome::Allowed(rows)) => assert!(
            !rows.contains(seed.agent_name()) && !rows.contains(&hidden.to_string()),
            "the listing shows agents without `view`: {rows}"
        ),
        other => panic!("the listing without `view` is not an empty success: {other:?}"),
    }
    Ok(())
}
