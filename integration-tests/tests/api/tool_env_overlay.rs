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

use async_trait::async_trait;
use golem_client::api::AgentClient;
use golem_client::invocation_session::{
    InvocationSession, InvocationSessionRequestProvider, InvocationSessionStateSnapshot,
    ReconnectPolicy, SessionTransportError, drive_native_tool_session,
};
use golem_client::model::{
    NativeToolInvocationMode, NativeToolInvocationRequest, NativeToolResult,
};
use golem_common::agent_id;
use golem_common::model::AgentId;
use golem_common::model::IdempotencyKey;
use golem_common::model::auth::TokenSecret;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_VERSION, PublicClientMessage, PublicErrorCode, PublicInvocationResult,
    PublicNativeToolTarget, PublicTypedValue,
};
use golem_common::schema::public_json::{
    PublicSchemaValueError, PublicStreamReferencePolicy, decode_public_schema_value,
    encode_public_schema_value,
};
use golem_common::schema::{
    ExternalTypedSchemaValue, FromSchema, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use test_r::{inherit_test_dep, test, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

inherit_test_dep!(EnvBasedTestDependencies);

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct EnvOverlayObservation {
    term: Option<String>,
    columns: Option<String>,
    lines: Option<String>,
    golem_agent_id: Option<String>,
}

struct SessionRequest {
    url: String,
    token: TokenSecret,
}

#[async_trait]
impl InvocationSessionRequestProvider for SessionRequest {
    async fn request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, SessionTransportError> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(SessionTransportError::Request)?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {}", self.token.secret())
                .parse()
                .map_err(|error| SessionTransportError::RequestProvider(format!("{error}")))?,
        );
        Ok(request)
    }
}

fn overlay(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn empty_input() -> TypedSchemaValue {
    TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![])),
        SchemaValue::Record { fields: vec![] },
    )
}

fn public_empty_input() -> anyhow::Result<PublicTypedValue> {
    let (graph, value) = empty_input().into_parts();
    let value = encode_public_schema_value(&graph, &graph.root, &value, |_, _| {
        Err(PublicSchemaValueError::new(
            PublicErrorCode::UnsupportedValue,
            "env-overlay-probe takes no streams",
        ))
    })
    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok(PublicTypedValue { graph, value })
}

fn rest_observation(result: Option<NativeToolResult>) -> anyhow::Result<EnvOverlayObservation> {
    let Some(NativeToolResult::Success(success)) = result else {
        anyhow::bail!("expected a successful env-overlay-probe result, got {result:?}");
    };
    let value = success
        .result
        .ok_or_else(|| anyhow::anyhow!("env-overlay-probe returned no value"))?
        .into_inner()
        .into_parts()
        .1;
    EnvOverlayObservation::from_value(&value).map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn session_observation(result: PublicInvocationResult) -> anyhow::Result<EnvOverlayObservation> {
    let PublicInvocationResult::ToolSuccess {
        result: Some(PublicTypedValue { graph, value }),
    } = result
    else {
        anyhow::bail!("expected a successful env-overlay-probe result, got {result:?}");
    };
    let value = decode_public_schema_value(
        &graph,
        &graph.root,
        &value,
        PublicStreamReferencePolicy::None,
        |_, _| {
            Err(PublicSchemaValueError::new(
                PublicErrorCode::UnsupportedValue,
                "env-overlay-probe returns no streams",
            ))
        },
    )
    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    EnvOverlayObservation::from_value(&value).map_err(|error| anyhow::anyhow!("{error:?}"))
}

/// The env overlay of `/invoke-tool` and of the session `toolStart` reaches the tool body of a
/// hosted invocation. It is fixed at start: the same key with another overlay does not replace
/// it, and a later lookup of the invocation returns what the original overlay produced.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn env_overlay_of_external_tool_start_reaches_hosted_body(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (app, env) = user.app_and_env().await?;
    let caller = user
        .component(&env.id, "golem_it_tool_streaming_rust_caller_release")
        .name("golem-it:tool-streaming-rust-caller")
        .store()
        .await?;
    user.component(&env.id, "golem_it_tool_streaming_rust_provider_release")
        .name("golem-it:tool-streaming-rust-provider")
        .with_tool_agent_binding("env-overlay-probe", "EnvOverlayCaller")?
        .store()
        .await?;

    let agent: AgentId = user
        .start_agent_with(
            &caller.id,
            agent_id!("EnvOverlayCaller", "env-overlay-integration"),
            HashMap::from([("TERM".to_string(), "owner-term".to_string())]),
            Vec::new(),
        )
        .await?;
    let golem_agent_id = Some(agent.agent_id.clone());
    let client = deps.worker_service().agent_http_client(&user.token).await;
    let rest_request =
        |key: &IdempotencyKey,
         mode: NativeToolInvocationMode,
         env_overlay: Option<HashMap<String, String>>| NativeToolInvocationRequest {
            app_name: app.name.to_string(),
            env_name: env.name.to_string(),
            agent_id: Some(agent.clone()),
            component_id: None,
            tool_name: "env-overlay-probe".to_string(),
            command_path: vec!["observe".to_string()],
            input: matches!(mode, NativeToolInvocationMode::Await).then(|| {
                ExternalTypedSchemaValue::try_from(empty_input()).expect("empty record input")
            }),
            mode,
            schedule_at: None,
            idempotency_key: Some(key.value.clone()),
            env_overlay,
        };

    let rest_key = IdempotencyKey::fresh();
    let rest = client
        .invoke_tool(
            Some(&rest_key.value),
            &rest_request(
                &rest_key,
                NativeToolInvocationMode::Await,
                Some(overlay(&[
                    ("TERM", "xterm-256color"),
                    ("COLUMNS", "132"),
                    ("GOLEM_AGENT_ID", "spoofed"),
                ])),
            ),
        )
        .await?;
    let expected_rest = EnvOverlayObservation {
        term: Some("xterm-256color".to_string()),
        columns: Some("132".to_string()),
        lines: None,
        golem_agent_id: golem_agent_id.clone(),
    };
    assert_eq!(rest_observation(rest.result)?, expected_rest);

    let changed = client
        .invoke_tool(
            Some(&rest_key.value),
            &rest_request(
                &rest_key,
                NativeToolInvocationMode::Await,
                Some(overlay(&[("COLUMNS", "80")])),
            ),
        )
        .await;
    assert!(
        changed.is_err(),
        "the same key with another overlay must be refused, got {changed:?}"
    );
    let lookup_key = IdempotencyKey::new(rest.idempotency_key.to_string());
    let looked_up = client
        .invoke_tool(
            None,
            &rest_request(&lookup_key, NativeToolInvocationMode::Lookup, None),
        )
        .await?;
    assert_eq!(rest_observation(looked_up.result)?, expected_rest);

    let worker_service = deps.worker_service();
    let session_key = IdempotencyKey::fresh();
    let session = InvocationSession::open(
        Arc::new(SessionRequest {
            url: format!(
                "ws://{}:{}/v1/agents/invoke-agent-session",
                worker_service.http_host(),
                worker_service.http_port()
            ),
            token: user.token.clone(),
        }),
        None,
        ReconnectPolicy::Unlimited,
        InvocationSessionStateSnapshot {
            delivered_output_cursors: BTreeMap::new(),
            stable_stream_bindings: BTreeMap::new(),
            pending_operation: Some(PublicClientMessage::ToolStart {
                attempt_id: uuid::Uuid::new_v4(),
                application: app.name.to_string(),
                environment: env.name.to_string(),
                idempotency_key: session_key.value.clone(),
                tool_name: "env-overlay-probe".to_string(),
                command_path: vec!["observe".to_string()],
                target: PublicNativeToolTarget::Agent {
                    component_id: agent.component_id.0,
                    agent_id: agent.agent_id.clone(),
                },
                input: Box::new(public_empty_input()?),
                stdin: false,
                stdout: false,
                stderr: false,
                env_overlay: BTreeMap::from([
                    ("COLUMNS".to_string(), "99".to_string()),
                    ("LINES".to_string(), "40".to_string()),
                ]),
                version: INVOCATION_SESSION_VERSION,
            }),
            session_token: None,
            accepted_invocation: None,
        },
        false,
        Arc::new(()),
    )
    .await?;
    let deliveries = session.delivery_tracker();
    let result = drive_native_tool_session(
        session,
        None::<tokio::io::Empty>,
        &mut tokio::io::sink(),
        &mut tokio::io::sink(),
    )
    .await?;
    let expected_session = EnvOverlayObservation {
        term: Some("owner-term".to_string()),
        columns: Some("99".to_string()),
        lines: Some("40".to_string()),
        golem_agent_id,
    };
    assert_eq!(session_observation(result)?, expected_session);

    let accepted = deliveries
        .snapshot()
        .accepted_invocation
        .ok_or_else(|| anyhow::anyhow!("the tool session ended without an acceptance"))?;
    let looked_up = client
        .invoke_tool(
            None,
            &rest_request(
                &IdempotencyKey::new(accepted.uuid().to_string()),
                NativeToolInvocationMode::Lookup,
                None,
            ),
        )
        .await?;
    assert_eq!(rest_observation(looked_up.result)?, expected_session);

    Ok(())
}
