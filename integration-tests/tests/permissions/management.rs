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
//! Every mode of every `COMMAND_TABLE` row has an adapter. An adapter makes
//! the mode's server requests in order, as the caller, through the service
//! clients and the invocation session, and returns the first denial. It
//! never spawns the CLI, and it passes `--no-stream` as every surface other
//! than the CLI does.
//!
//! The acceptance cases need the 2.0 card model: the decision types
//! (GOL-1270), the denial renderers (GOL-1272), the class catalog
//! (GOL-1276), the 2.0 authorizer (GOL-1288) and the switch to it
//! (GOL-1289), all owned by Aditya Salunkhe. Until then they are ignored with
//! what they wait for. Pending cases with no test here yet:
//!
//! | Case | Waits for |
//! | --- | --- |
//! | Interrupting or resuming a standalone tool invocation, or reading its oplog, without the grant looks like an unknown invocation | GOL-1079 and GOL-1086 (the entry points); GOL-1417 adds the command rows |
//! | Closing a tool-owned webhook without `close-webhook` looks like an unknown webhook | GOL-1390 (the webhook commands); the later of GOL-1390 and GOL-1403 adds the rows |
//! | A manual update of an agent with the never-live mark | Open: decision 2 of the GOL-1403 plan, after the snapshot-migration rehearsal design (`MANUAL_UPDATE_OF_A_MARKED_AGENT`) |
//! | `agent list` returns only the agents that the caller's `view` covers | GOL-1404 (`scope` and `filter`) |

use futures::future::BoxFuture;
use golem_cli::command_table::{COMMAND_TABLE, CommandClass};
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
use golem_client::{Error, ErrorInfo};
use golem_common::agent_id;
use golem_common::base_model::worker::{
    AgentCreationRequest, RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget,
};
use golem_common::model::AgentId;
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::invocation_session_public::PublicErrorCode;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_VERSION, InvocationSelector, PublicClientMessage, PublicNativeToolTarget,
    PublicResumeOperation, PublicServerMessage, PublicTypedValue,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName,
};
use golem_common::schema::TypedSchemaValue;
use golem_common::schema::public_json::{PublicSchemaValueError, encode_public_schema_value};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

inherit_test_dep!(EnvBasedTestDependencies);

type User = TestUserContext<EnvBasedTestDependencies>;

/// What the caller's requests ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Allowed,
    Denied(Denial),
}

/// A denial, as the caller sees it. Two denials are the same response when
/// they are equal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Denial {
    Rest {
        status: u16,
        code: Option<String>,
        messages: Vec<String>,
    },
    Session {
        code: String,
        message: String,
    },
    Connect(String),
}

/// The owner's agent, component and invocations that the caller addresses.
struct Seed {
    owner: User,
    caller: User,
    application: String,
    environment: String,
    component: ComponentDto,
    /// The agent that the commands address.
    target: ParsedAgentKey,
    target_id: AgentId,
    /// An invocation of the target that waits in its schedule, so that it
    /// stays cancellable.
    scheduled_invocation: String,
    /// The session token of an invocation of the target that the owner
    /// started.
    session_token: String,
}

const AGENT_TYPE: &str = "RpcCounter";
const METHOD: &str = "get_value";
/// The tool that the exec and tool modes address. The fixture's component
/// must bind it before those cases can run.
const TOOL: &str = "bash";

impl Seed {
    async fn new(deps: &EnvBasedTestDependencies) -> anyhow::Result<Self> {
        let owner = deps.user().await?;
        let caller = deps.user().await?;
        let (_, env) = owner.app_and_env().await?;
        let component = owner
            .component(&env.id, "golem_it_agent_rpc_rust_release")
            .name("golem-it:agent-rpc-rust")
            .unique()
            .store()
            .await?;
        let target = agent_id!(AGENT_TYPE, "management-target");
        let target_id = owner.start_agent(&component.id, target.clone()).await?;
        let mut seed = Self {
            owner,
            caller,
            application: env.application_name.0.clone(),
            environment: env.name.0.clone(),
            component,
            target,
            target_id,
            scheduled_invocation: String::new(),
            session_token: String::new(),
        };
        seed.scheduled_invocation = seed.schedule_invocation().await?;
        seed.session_token = match start_session(&seed, &seed.owner).await? {
            (Outcome::Allowed, Some(token)) => token,
            other => anyhow::bail!("the owner could not start a session: {other:?}"),
        };
        Ok(seed)
    }

    /// The same seed, addressing an agent that does not exist.
    fn with_unknown_target(&self) -> anyhow::Result<Self> {
        let target = agent_id!(AGENT_TYPE, "management-unknown");
        Ok(Self {
            owner: self.owner.clone(),
            caller: self.caller.clone(),
            application: self.application.clone(),
            environment: self.environment.clone(),
            component: self.component.clone(),
            target_id: AgentId {
                component_id: self.component.id,
                agent_id: target.to_string(),
            },
            target,
            scheduled_invocation: self.scheduled_invocation.clone(),
            session_token: self.session_token.clone(),
        })
    }

    async fn schedule_invocation(&self) -> anyhow::Result<String> {
        let mut request = invocation_request(self, AgentInvocationMode::Schedule)?;
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

    /// The owner's view of the target, which a denied control must leave as
    /// it was.
    async fn observe(&self) -> anyhow::Result<String> {
        let metadata = worker_client(&self.owner)
            .await
            .get_worker_metadata(&self.component.id.0, self.agent_name())
            .await?;
        Ok(format!(
            "{:?} {} {} {:?}",
            metadata.status,
            metadata.component_revision,
            metadata.pending_invocation_count,
            metadata.active_plugins,
        ))
    }
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
        method_parameters: golem_common::data_value!()
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

fn rest<T, E: ErrorInfo + std::fmt::Debug>(result: Result<T, Error<E>>) -> anyhow::Result<Outcome> {
    match result {
        Ok(_) => Ok(Outcome::Allowed),
        Err(Error::Item(error)) => Ok(Outcome::Denied(Denial::Rest {
            status: error.status_code(),
            code: error.code().map(str::to_string),
            messages: error.errors().to_vec(),
        })),
        Err(error) => Err(anyhow::anyhow!("{error:?}")),
    }
}

/// Runs the requests in order and returns the first denial.
macro_rules! in_order {
    ($($request:expr),+ $(,)?) => {{
        let mut outcome = Outcome::Allowed;
        $(
            if outcome == Outcome::Allowed {
                outcome = $request;
            }
        )+
        Ok(outcome)
    }};
}

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

/// Opens an invocation session with one start or resume, and returns whether
/// the server accepted it, with the session token of an accepted one.
async fn session(
    user: &User,
    operation: PublicClientMessage,
) -> anyhow::Result<(Outcome, Option<String>)> {
    let service = user.deps.worker_service();
    let requests = SessionRequests {
        url: format!(
            "ws://{}:{}/v1/agents/invoke-agent-session",
            service.http_host(),
            service.http_port()
        ),
        token: user.token.secret().to_string(),
    };
    let mut session = InvocationSession::open(
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
    .await?;
    loop {
        let mut frame = session.receive().await?;
        let message = frame.frame().clone();
        frame.mark_delivered()?;
        match message {
            ServerFrame::Message(PublicServerMessage::InvocationAccepted {
                session_token, ..
            }) => return Ok((Outcome::Allowed, Some(session_token))),
            ServerFrame::Message(PublicServerMessage::InvocationRejected {
                code, message, ..
            }) => {
                return Ok((
                    Outcome::Denied(Denial::Session {
                        code: format!("{code:?}"),
                        message,
                    }),
                    None,
                ));
            }
            _ => continue,
        }
    }
}

async fn start_session(seed: &Seed, user: &User) -> anyhow::Result<(Outcome, Option<String>)> {
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
            method_parameters: public_json(&golem_common::data_value!())?,
            selector: Box::new(selector),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await
}

fn tool_start(seed: &Seed, target: PublicNativeToolTarget) -> anyhow::Result<PublicClientMessage> {
    Ok(PublicClientMessage::ToolStart {
        attempt_id: uuid::Uuid::new_v4(),
        application: seed.application.clone(),
        environment: seed.environment.clone(),
        idempotency_key: uuid::Uuid::new_v4().to_string(),
        tool_name: TOOL.to_string(),
        command_path: vec!["run".to_string()],
        target,
        input: Box::new(
            PublicTypedValue::try_from(golem_common::data_value!()).map_err(anyhow::Error::msg)?,
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

fn describe_request(seed: &Seed, hosted: bool) -> NativeToolDescribeRequest {
    NativeToolDescribeRequest {
        app_name: seed.application.clone(),
        env_name: seed.environment.clone(),
        agent_id: hosted.then(|| seed.target_id.clone()),
        component_id: (!hosted).then_some(seed.component.id.0),
        tool_name: TOOL.to_string(),
    }
}

fn tool_request(
    seed: &Seed,
    hosted: bool,
    mode: NativeToolInvocationMode,
) -> NativeToolInvocationRequest {
    NativeToolInvocationRequest {
        app_name: seed.application.clone(),
        env_name: seed.environment.clone(),
        agent_id: hosted.then(|| seed.target_id.clone()),
        component_id: (!hosted).then_some(seed.component.id.0),
        tool_name: TOOL.to_string(),
        command_path: vec!["run".to_string()],
        input: None,
        mode,
        schedule_at: None,
        idempotency_key: None,
    }
}

/// The CLI's metadata read of the named agent, which it makes first.
async fn prepare(seed: &Seed) -> anyhow::Result<Outcome> {
    rest(
        worker_client(&seed.caller)
            .await
            .get_worker_metadata(&seed.component.id.0, seed.agent_name())
            .await,
    )
}

async fn connect(seed: &Seed, agent_id: &AgentId) -> anyhow::Result<Outcome> {
    Ok(
        match seed.caller.make_worker_log_event_stream(agent_id).await {
            Ok(_) => Outcome::Allowed,
            Err(error) => Outcome::Denied(Denial::Connect(error.to_string())),
        },
    )
}

type Run = for<'a> fn(&'a Seed) -> BoxFuture<'a, anyhow::Result<Outcome>>;

/// The requests of one mode of one command row, as the caller.
struct Adapter {
    path: &'static [&'static str],
    mode: &'static str,
    run: Run,
}

macro_rules! adapter {
    ([$($segment:literal),+], $mode:literal, |$seed:ident| $body:expr) => {
        Adapter {
            path: &[$($segment),+],
            mode: $mode,
            run: |$seed| Box::pin(async move { $body }),
        }
    };
}

const ADAPTERS: &[Adapter] = &[
    adapter!(["agent", "new"], "create", |seed| {
        // The CLI reads the metadata of the agent it creates and goes on when
        // it does not exist.
        prepare(seed).await?;
        rest(
            worker_client(&seed.caller)
                .await
                .launch_new_worker(
                    &seed.component.id.0,
                    &AgentCreationRequest {
                        name: agent_id!(AGENT_TYPE, "management-new").to_string(),
                        env: Default::default(),
                        config: Vec::new(),
                    },
                )
                .await,
        )
    }),
    adapter!(["agent", "invoke"], "start", |seed| in_order!(
        prepare(seed).await?,
        start_session(seed, &seed.caller).await?.0,
    )),
    adapter!(["agent", "invoke"], "resume-session", |seed| in_order!(
        prepare(seed).await?,
        session(
            &seed.caller,
            PublicClientMessage::ResumeAttach {
                attempt_id: uuid::Uuid::new_v4(),
                operation: PublicResumeOperation::Resume,
                output_cursors: Vec::new(),
                session_token: seed.session_token.clone(),
                version: INVOCATION_SESSION_VERSION,
            },
        )
        .await?
        .0,
    )),
    adapter!(["agent", "get"], "get", |seed| in_order!(
        prepare(seed).await?,
        prepare(seed).await?,
    )),
    adapter!(["agent", "delete"], "delete", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .delete_worker(&seed.component.id.0, seed.agent_name())
                .await
        )?,
    )),
    adapter!(["agent", "list"], "list", |seed| rest(
        worker_client(&seed.caller)
            .await
            .find_workers_metadata(
                &seed.component.id.0,
                &WorkersMetadataRequest {
                    filter: None,
                    cursor: None,
                    count: Some(10),
                    precise: None,
                },
            )
            .await
    )),
    adapter!(["agent", "stream"], "follow", |seed| in_order!(
        prepare(seed).await?,
        connect(seed, &seed.target_id).await?,
    )),
    // The connection names the agent of the REPL's invocation; the
    // invocation that the stdin line selects stays on the client today.
    adapter!(["agent", "repl-stream"], "follow", |seed| connect(
        seed,
        &seed.target_id
    )
    .await),
    adapter!(["agent", "update"], "automatic", |seed| in_order!(
        prepare(seed).await?,
        update(seed, AgentUpdateMode::Automatic).await?,
    )),
    adapter!(["agent", "update"], "manual", |seed| in_order!(
        prepare(seed).await?,
        update(seed, AgentUpdateMode::Manual).await?,
    )),
    adapter!(["agent", "interrupt"], "interrupt", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .interrupt_worker(&seed.component.id.0, seed.agent_name(), Some(false))
                .await
        )?,
    )),
    adapter!(["agent", "resume"], "resume", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .resume_worker(&seed.component.id.0, seed.agent_name())
                .await
        )?,
    )),
    adapter!(
        ["agent", "simulate-crash"],
        "simulate-crash",
        |seed| in_order!(
            prepare(seed).await?,
            rest(
                worker_client(&seed.caller)
                    .await
                    .interrupt_worker(&seed.component.id.0, seed.agent_name(), Some(true))
                    .await
            )?,
        )
    ),
    adapter!(["agent", "oplog"], "page", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .get_oplog(
                    &seed.component.id.0,
                    seed.agent_name(),
                    Some(0),
                    10,
                    None,
                    None
                )
                .await
        )?,
        rest(
            worker_client(&seed.caller)
                .await
                .get_oplog_entry(&seed.component.id.0, seed.agent_name(), 1, None)
                .await
        )?,
    )),
    adapter!(["agent", "oplog"], "search", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .get_oplog(
                    &seed.component.id.0,
                    seed.agent_name(),
                    None,
                    10,
                    None,
                    Some("create")
                )
                .await
        )?,
    )),
    adapter!(["agent", "revert"], "to-oplog-index", |seed| in_order!(
        prepare(seed).await?,
        revert(
            seed,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: OplogIndex::from_u64(1),
            })
        )
        .await?,
    )),
    adapter!(
        ["agent", "revert"],
        "by-invocation-count",
        |seed| in_order!(
            prepare(seed).await?,
            revert(
                seed,
                RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                    number_of_invocations: 1,
                })
            )
            .await?,
        )
    ),
    adapter!(["agent", "cancel-invocation"], "cancel", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .cancel_invocation(
                    &seed.component.id.0,
                    seed.agent_name(),
                    &seed.scheduled_invocation
                )
                .await
        )?,
    )),
    adapter!(["agent", "files"], "list", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .get_files(&seed.component.id.0, seed.agent_name(), "/")
                .await
        )?,
    )),
    adapter!(["agent", "file-contents"], "read", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .get_file_content(&seed.component.id.0, seed.agent_name(), "/")
                .await
        )?,
    )),
    adapter!(["agent", "activate-plugin"], "activate", |seed| in_order!(
        prepare(seed).await?,
        rest(
            worker_client(&seed.caller)
                .await
                .activate_plugin(&seed.component.id.0, seed.agent_name(), 0)
                .await
        )?,
    )),
    adapter!(
        ["agent", "deactivate-plugin"],
        "deactivate",
        |seed| in_order!(
            prepare(seed).await?,
            rest(
                worker_client(&seed.caller)
                    .await
                    .deactivate_plugin(&seed.component.id.0, seed.agent_name(), 0)
                    .await
            )?,
        )
    ),
    adapter!(["ssh"], "session", |seed| in_order!(
        prepare(seed).await?,
        describe(seed, true).await?,
        session(&seed.caller, tool_start(seed, hosted_target(seed))?)
            .await?
            .0,
    )),
    adapter!(["ssh"], "script", |seed| in_order!(
        prepare(seed).await?,
        describe(seed, true).await?,
        session(&seed.caller, tool_start(seed, hosted_target(seed))?)
            .await?
            .0,
    )),
    adapter!(["tool", "invoke"], "hosted-start", |seed| in_order!(
        prepare(seed).await?,
        describe(seed, true).await?,
        rest(
            agent_client(&seed.caller)
                .await
                .invoke_tool(
                    None,
                    &tool_request(seed, true, NativeToolInvocationMode::Await)
                )
                .await
        )?,
    )),
    adapter!(["tool", "invoke"], "standalone-start", |seed| in_order!(
        describe(seed, false).await?,
        rest(
            agent_client(&seed.caller)
                .await
                .invoke_tool(
                    None,
                    &tool_request(seed, false, NativeToolInvocationMode::Await)
                )
                .await
        )?,
    )),
    adapter!(["tool", "invoke"], "lookup", |seed| {
        let mut request = tool_request(seed, true, NativeToolInvocationMode::Lookup);
        request.idempotency_key = Some(seed.scheduled_invocation.clone());
        rest(
            agent_client(&seed.caller)
                .await
                .invoke_tool(request.idempotency_key.as_deref(), &request)
                .await,
        )
    }),
];

async fn update(seed: &Seed, mode: AgentUpdateMode) -> anyhow::Result<Outcome> {
    rest(
        worker_client(&seed.caller)
            .await
            .update_worker(
                &seed.component.id.0,
                seed.agent_name(),
                &UpdateWorkerRequest {
                    mode,
                    target_revision: seed.component.revision.into(),
                    disable_wakeup: Some(true),
                },
            )
            .await,
    )
}

async fn revert(seed: &Seed, target: RevertWorkerTarget) -> anyhow::Result<Outcome> {
    rest(
        worker_client(&seed.caller)
            .await
            .revert_worker(&seed.component.id.0, seed.agent_name(), &target)
            .await,
    )
}

async fn describe(seed: &Seed, hosted: bool) -> anyhow::Result<Outcome> {
    rest(
        agent_client(&seed.caller)
            .await
            .describe_tool(&describe_request(seed, hosted))
            .await,
    )
}

fn adapter(path: &[&str], mode: &str) -> &'static Adapter {
    ADAPTERS
        .iter()
        .find(|adapter| adapter.path == path && adapter.mode == mode)
        .unwrap_or_else(|| panic!("`{}` mode `{mode}` has no adapter", path.join(" ")))
}

/// What a grant of a case is on.
#[derive(Debug, Clone, Copy)]
enum On {
    /// The target agent.
    Target,
    /// Every agent of the target's type.
    TargetType,
    /// Every invocation of the target agent.
    TargetInvocations,
    /// The tool that the exec and tool modes address.
    Tool,
}

/// One grant that a case gives the caller, in the grant text of the 2.0
/// class catalog (GOL-1276); `render` spells its owner.
#[derive(Debug, Clone, Copy)]
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

fn render(seed: &Seed, grant: &Grant) -> String {
    let environment = format!(
        "{}/{}/{}",
        seed.owner.account_email.as_str(),
        seed.application,
        seed.environment
    );
    let component = format!("{environment}/{}", seed.component.component_name.0);
    let owner = match grant.on {
        On::Target | On::TargetInvocations => format!("{component}/{}", seed.agent_name()),
        On::TargetType => format!("{component}/{AGENT_TYPE}(*)"),
        On::Tool => format!("{environment}/{TOOL}"),
    };
    format!(
        "{}({owner}) @ {} : {} : {}",
        grant.class,
        seed.caller.account_email.as_str(),
        grant.verb,
        grant.resource
    )
}

/// Installs a card with exactly these grants into the caller's wallet, with
/// the grants that let the caller address the owner's environment and
/// component at all. GOL-1289 decides whether the 2.0 model still needs those.
async fn give(seed: &Seed, name: &str, grants: &[Grant]) -> anyhow::Result<()> {
    let positive = [
        format!(
            "environment({}/{}/{}) @ {} : view :",
            seed.owner.account_email.as_str(),
            seed.application,
            seed.environment,
            seed.caller.account_email.as_str()
        ),
        format!(
            "component({}/{}/{}/*) @ {} : view : *",
            seed.owner.account_email.as_str(),
            seed.application,
            seed.environment,
            seed.caller.account_email.as_str()
        ),
    ]
    .into_iter()
    .chain(grants.iter().map(|grant| render(seed, grant)))
    .collect();
    seed.owner
        .registry_service_client()
        .await
        .create_permission_share(
            &seed.owner.account_id.0,
            &PermissionShareCreation {
                target_account_email: seed.caller.account_email.clone(),
                name: PermissionShareName(name.to_string()),
                data: PermissionShareData {
                    lower_positive: positive,
                    lower_negative: Vec::new(),
                    upper_positive: Vec::new(),
                    upper_negative: Vec::new(),
                },
            },
        )
        .await?;
    Ok(())
}

/// How a request denied for a missing grant must look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LooksLike {
    /// The same response as for an agent ID that does not exist.
    UnknownAgent,
    /// The same response as for an invocation that does not exist.
    UnknownInvocation,
    /// A `403`, for a part on a tool by name.
    Forbidden,
    /// GOL-1296 decides the denial of a creation.
    DecidedByCreation,
}

/// A mode of a command row, the grants that allow it, and how its denial
/// looks when each one of them is missing.
struct Case {
    path: &'static [&'static str],
    mode: &'static str,
    grants: &'static [(Grant, LooksLike)],
}

const VIEW: Grant = grant("agent", "view", On::Target, "");
const VIEW_DENIED: (Grant, LooksLike) = (VIEW, LooksLike::UnknownAgent);
const OPLOG_READ: Grant = grant("oplog", "read", On::Target, "*");
const TOOL_VIEW: (Grant, LooksLike) = (grant("tool", "view", On::Tool, ""), LooksLike::Forbidden);
const EXEC: &[(Grant, LooksLike)] = &[
    VIEW_DENIED,
    TOOL_VIEW,
    (
        grant("agent", "exec", On::Target, ""),
        LooksLike::UnknownAgent,
    ),
    (grant("tool", "invoke", On::Tool, "*"), LooksLike::Forbidden),
];

const fn control(verb: &'static str, resource: &'static str) -> (Grant, LooksLike) {
    (
        grant("agent", verb, On::Target, resource),
        LooksLike::UnknownAgent,
    )
}

const CASES: &[Case] = &[
    Case {
        path: &["agent", "new"],
        mode: "create",
        grants: &[(
            grant("agent", "create", On::TargetType, ""),
            LooksLike::DecidedByCreation,
        )],
    },
    Case {
        path: &["agent", "invoke"],
        mode: "start",
        grants: &[VIEW_DENIED, control("invoke", "*")],
    },
    Case {
        path: &["agent", "invoke"],
        mode: "resume-session",
        grants: &[
            VIEW_DENIED,
            (
                grant("invocation", "read", On::TargetInvocations, ""),
                LooksLike::UnknownInvocation,
            ),
        ],
    },
    Case {
        path: &["agent", "get"],
        mode: "get",
        grants: &[VIEW_DENIED],
    },
    Case {
        path: &["agent", "delete"],
        mode: "delete",
        grants: &[VIEW_DENIED, control("delete", "")],
    },
    Case {
        path: &["agent", "list"],
        mode: "list",
        grants: &[(
            grant("agent", "view", On::TargetType, ""),
            LooksLike::UnknownAgent,
        )],
    },
    Case {
        path: &["agent", "stream"],
        mode: "follow",
        grants: &[
            VIEW_DENIED,
            (
                grant("invocation", "read", On::TargetInvocations, ""),
                LooksLike::UnknownInvocation,
            ),
        ],
    },
    Case {
        path: &["agent", "repl-stream"],
        mode: "follow",
        grants: &[(
            grant("invocation", "read", On::TargetInvocations, ""),
            LooksLike::UnknownInvocation,
        )],
    },
    Case {
        path: &["agent", "update"],
        mode: "automatic",
        grants: &[VIEW_DENIED, control("update-revision", "")],
    },
    Case {
        path: &["agent", "update"],
        mode: "manual",
        grants: &[VIEW_DENIED, control("update-revision", "")],
    },
    Case {
        path: &["agent", "interrupt"],
        mode: "interrupt",
        grants: &[VIEW_DENIED, control("interrupt", "")],
    },
    Case {
        path: &["agent", "resume"],
        mode: "resume",
        grants: &[VIEW_DENIED, control("resume", "")],
    },
    Case {
        path: &["agent", "simulate-crash"],
        mode: "simulate-crash",
        grants: &[VIEW_DENIED, control("simulate-crash", "")],
    },
    Case {
        path: &["agent", "oplog"],
        mode: "page",
        grants: &[VIEW_DENIED, (OPLOG_READ, LooksLike::UnknownAgent)],
    },
    Case {
        path: &["agent", "oplog"],
        mode: "search",
        grants: &[VIEW_DENIED, (OPLOG_READ, LooksLike::UnknownAgent)],
    },
    Case {
        path: &["agent", "revert"],
        mode: "to-oplog-index",
        grants: &[VIEW_DENIED, control("revert", "*")],
    },
    Case {
        path: &["agent", "revert"],
        mode: "by-invocation-count",
        grants: &[VIEW_DENIED, control("revert", "*")],
    },
    Case {
        path: &["agent", "cancel-invocation"],
        mode: "cancel",
        grants: &[
            VIEW_DENIED,
            (
                grant("invocation", "cancel", On::TargetInvocations, ""),
                LooksLike::UnknownInvocation,
            ),
        ],
    },
    Case {
        path: &["agent", "files"],
        mode: "list",
        grants: &[
            VIEW_DENIED,
            (
                grant("filesystem", "list", On::Target, "/**"),
                LooksLike::UnknownAgent,
            ),
        ],
    },
    Case {
        path: &["agent", "file-contents"],
        mode: "read",
        grants: &[
            VIEW_DENIED,
            (
                grant("filesystem", "read", On::Target, "/**"),
                LooksLike::UnknownAgent,
            ),
        ],
    },
    Case {
        path: &["agent", "activate-plugin"],
        mode: "activate",
        grants: &[VIEW_DENIED, control("activate-processor", "*")],
    },
    Case {
        path: &["agent", "deactivate-plugin"],
        mode: "deactivate",
        grants: &[VIEW_DENIED, control("deactivate-processor", "*")],
    },
    Case {
        path: &["ssh"],
        mode: "session",
        grants: EXEC,
    },
    Case {
        path: &["ssh"],
        mode: "script",
        grants: EXEC,
    },
    Case {
        path: &["tool", "invoke"],
        mode: "hosted-start",
        grants: EXEC,
    },
    Case {
        path: &["tool", "invoke"],
        mode: "standalone-start",
        grants: &[
            TOOL_VIEW,
            (grant("tool", "invoke", On::Tool, "*"), LooksLike::Forbidden),
        ],
    },
    Case {
        path: &["tool", "invoke"],
        mode: "lookup",
        grants: &[(
            grant("invocation", "read", On::TargetInvocations, ""),
            LooksLike::UnknownInvocation,
        )],
    },
];

/// The modes that change an execution or start work, which `view` and
/// `oplog : read` alone must never allow.
const CONTROLS_AND_STARTS: &[(&[&str], &str)] = &[
    (&["agent", "new"], "create"),
    (&["agent", "invoke"], "start"),
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

fn modes() -> BTreeSet<(String, &'static str)> {
    COMMAND_TABLE
        .iter()
        .flat_map(|row| row.modes.iter().map(|mode| (row.path.join(" "), mode.name)))
        .collect()
}

/// Every mode of the command table has one adapter and one case, and the
/// read-only case covers every control and exec mode and no read mode.
#[test]
fn management_every_command_mode_has_an_adapter_and_a_case() {
    let modes = modes();
    let adapters = ADAPTERS
        .iter()
        .map(|adapter| (adapter.path.join(" "), adapter.mode))
        .collect::<Vec<_>>();
    let cases = CASES
        .iter()
        .map(|case| (case.path.join(" "), case.mode))
        .collect::<Vec<_>>();
    let mut problems = Vec::new();
    for (what, keys) in [("adapter", &adapters), ("case", &cases)] {
        let unique = keys.iter().cloned().collect::<BTreeSet<_>>();
        if unique.len() != keys.len() {
            problems.push(format!("a mode has more than one {what}"));
        }
        for (path, mode) in modes.difference(&unique) {
            problems.push(format!("`{path}` mode `{mode}` has no {what}"));
        }
        for (path, mode) in unique.difference(&modes) {
            problems.push(format!("the {what} of `{path}` mode `{mode}` has no row"));
        }
    }
    let read_only = CONTROLS_AND_STARTS
        .iter()
        .map(|(path, mode)| (path.join(" "), *mode))
        .collect::<BTreeSet<_>>();
    for row in COMMAND_TABLE {
        for mode in row.modes {
            let key = (row.path.join(" "), mode.name);
            let covered = read_only.contains(&key);
            match mode.class {
                CommandClass::ManagementControl | CommandClass::Exec if !covered => {
                    problems.push(format!(
                        "`{}` mode `{}` is missing from the read-only case",
                        key.0, key.1
                    ))
                }
                CommandClass::ManagementRead if covered => problems.push(format!(
                    "`{}` mode `{}` is a read in the read-only case",
                    key.0, key.1
                )),
                _ => {}
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Allowed with its grants, and denied without any one of them, with the
/// same response as an unknown target where the target is a Rule 21 kind.
#[test]
#[timeout("30m")]
#[ignore = "pending: the 2.0 card model (GOL-1270, GOL-1272, GOL-1276, GOL-1288, GOL-1289; Aditya Salunkhe); the creation contract (GOL-1296); the invocation entry points (GOL-1051); the standalone tool invocation (GOL-1079)"]
async fn management_every_operation_is_allowed_with_its_grants_and_looks_unknown_without_them(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    for case in CASES {
        let adapter = adapter(case.path, case.mode);
        let all = case
            .grants
            .iter()
            .map(|(grant, _)| *grant)
            .collect::<Vec<_>>();
        for (index, (_, looks_like)) in case.grants.iter().enumerate() {
            let seed = Seed::new(deps).await?;
            let mut without = all.clone();
            without.remove(index);
            give(&seed, "management-without-one", &without).await?;
            let before = seed.observe().await?;
            let denied = (adapter.run)(&seed).await?;
            assert!(
                matches!(denied, Outcome::Denied(_)),
                "`{}` mode `{}` was allowed without {:?}",
                case.path.join(" "),
                case.mode,
                all[index]
            );
            assert_eq!(
                seed.observe().await?,
                before,
                "a denied request changed the target"
            );
            match looks_like {
                LooksLike::UnknownAgent | LooksLike::UnknownInvocation => {
                    let unknown = (adapter.run)(&seed.with_unknown_target()?).await?;
                    assert_eq!(denied, unknown, "a denial differs from an unknown target");
                }
                LooksLike::Forbidden => assert!(
                    matches!(denied, Outcome::Denied(Denial::Rest { status: 403, .. })),
                    "a denied part on a tool by name is not a 403: {denied:?}"
                ),
                LooksLike::DecidedByCreation => {}
            }
        }
        let seed = Seed::new(deps).await?;
        give(&seed, "management-all", &all).await?;
        assert_eq!(
            (adapter.run)(&seed).await?,
            Outcome::Allowed,
            "`{}` mode `{}` was denied with its grants",
            case.path.join(" "),
            case.mode
        );
    }
    Ok(())
}

/// `view` and `oplog : read` neither control nor invoke, also through REST
/// calls that no client preflight guards.
#[test]
#[timeout("30m")]
#[ignore = "pending: the 2.0 card model (GOL-1270, GOL-1272, GOL-1276, GOL-1288, GOL-1289; Aditya Salunkhe)"]
async fn management_read_grants_cannot_control_or_invoke(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let seed = Seed::new(deps).await?;
    give(&seed, "management-read-only", &[VIEW, OPLOG_READ]).await?;
    for (path, mode) in CONTROLS_AND_STARTS {
        let before = seed.observe().await?;
        let outcome = (adapter(path, mode).run)(&seed).await?;
        assert!(
            matches!(outcome, Outcome::Denied(_)),
            "`{}` mode `{mode}` was allowed with read grants",
            path.join(" ")
        );
        assert_eq!(
            seed.observe().await?,
            before,
            "`{}` mode `{mode}` changed the target",
            path.join(" ")
        );
    }
    Ok(())
}

/// Cancelling needs the invocation's `cancel` grant; `read` is not enough.
/// A denied cancel leaves the invocation in place, so the later cancel
/// under the `cancel` grant still cancels it.
#[test]
#[timeout("10m")]
#[ignore = "pending: the invocation class (GOL-1276) and the 2.0 card model (GOL-1289; Aditya Salunkhe); the invocation cancellation entry point (GOL-1051)"]
async fn management_cancel_needs_the_invocation_cancel_grant(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let cancel = adapter(&["agent", "cancel-invocation"], "cancel");
    let seed = Seed::new(deps).await?;
    give(
        &seed,
        "management-invocation-read",
        &[VIEW, grant("invocation", "read", On::TargetInvocations, "")],
    )
    .await?;
    let denied = (cancel.run)(&seed).await?;
    let mut unknown_invocation = seed.with_unknown_target()?;
    unknown_invocation.target = seed.target.clone();
    unknown_invocation.target_id = seed.target_id.clone();
    unknown_invocation.scheduled_invocation = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        denied,
        (cancel.run)(&unknown_invocation).await?,
        "a cancel under a read grant differs from a cancel of an unknown invocation"
    );
    give(
        &seed,
        "management-invocation-cancel",
        &[grant("invocation", "cancel", On::TargetInvocations, "")],
    )
    .await?;
    let cancelled = worker_client(&seed.caller)
        .await
        .cancel_invocation(
            &seed.component.id.0,
            seed.agent_name(),
            &seed.scheduled_invocation,
        )
        .await?;
    assert!(
        cancelled.canceled,
        "the denied cancel already removed the invocation"
    );
    Ok(())
}

/// A type-wide `replay` grant updates and deletes a sandbox fork of the type,
/// and neither updates nor deletes an agent of the type without the mark. The
/// manual update of a marked agent is not a case here: it waits for decision
/// 2 (`MANUAL_UPDATE_OF_A_MARKED_AGENT`).
#[test]
#[timeout("10m")]
#[ignore = "pending: the never-live mark and the fork that sets it (GOL-1420); the replay verb (GOL-1276) and the 2.0 card model (GOL-1289; Aditya Salunkhe)"]
async fn management_replay_cannot_change_a_real_agent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let update = adapter(&["agent", "update"], "automatic");
    let delete = adapter(&["agent", "delete"], "delete");
    let seed = Seed::new(deps).await?;
    give(
        &seed,
        "management-replay",
        &[
            grant("agent", "view", On::TargetType, ""),
            grant("agent", "replay", On::TargetType, ""),
        ],
    )
    .await?;
    for real in [update, delete] {
        let before = seed.observe().await?;
        let outcome = (real.run)(&seed).await?;
        assert_eq!(outcome, (real.run)(&seed.with_unknown_target()?).await?);
        assert_eq!(seed.observe().await?, before, "replay changed a real agent");
    }
    anyhow::bail!(
        "the marked half of this case needs a sandbox fork with the never-live mark, which GOL-1420 supplies"
    )
}
