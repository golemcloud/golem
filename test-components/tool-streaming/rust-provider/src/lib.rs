use futures_concurrency::prelude::*;
use golem_rust::agentic::{
    AgentStream, InputStream, OutputStream, Principal, Secret as ConfigSecret, pump_tool_stdin,
    spawn_local,
};
use golem_rust::bindings::golem::permissions::{
    derive as permission_derive, types as permission_types,
};
use golem_rust::golem_agentic::golem::agent::host as agent_host;
use golem_rust::golem_agentic::golem::tool::host::{self as tool_host, ByteStreamFailure, ToolRpc};
use golem_rust::quota::QuotaToken;
use golem_rust::schema::wit::GuestPermissionCardHandle;
use golem_rust::schema::wit::direct::{
    WireError, WirePreflight, WireReader, WireSchemaBuilder, WireWriter,
};
use golem_rust::schema::wit::wire;
use golem_rust::schema::{
    FromSchemaError, QuotaTokenSpec, SchemaBuilder, SchemaType, SchemaValue, TypeId,
};
use golem_rust::secrets::GuestSecretHandle;
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoTypedSchemaValue, IntoWire, ToolError, WireSchema,
    decode_schema_value, encode_schema_graph, tool_definition, tool_implementation,
};
use std::cell::Cell;
use std::rc::Rc;
use wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

const MARKER: &[u8] = b"marker:";

async fn race_p3_sleeps(secs: Vec<u64>) -> u64 {
    let waits: Vec<_> = secs
        .into_iter()
        .map(|secs| async move {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                secs.saturating_mul(1_000_000_000),
            )
            .await;
            secs
        })
        .collect();
    waits.race().await
}

#[derive(Debug, Clone, ToolError)]
pub enum ClockRaceError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Failed { message: String },
}

#[tool_definition(version = "1.0.0")]
pub trait ClockRace {
    async fn race_p3_sleeps(&self, secs: Vec<u64>) -> Result<u64, ClockRaceError>;
    async fn race_promise_and_p3_sleep(&self, secs: u64) -> Result<String, ClockRaceError>;
    async fn polling_loop_vs_watchdog(&self) -> Result<String, ClockRaceError>;
    async fn follow_up(&self) -> Result<String, ClockRaceError>;
}

struct ClockRaceImpl;

#[tool_implementation]
impl ClockRace for ClockRaceImpl {
    async fn race_p3_sleeps(&self, secs: Vec<u64>) -> Result<u64, ClockRaceError> {
        Ok(race_p3_sleeps(secs).await)
    }

    async fn race_promise_and_p3_sleep(&self, secs: u64) -> Result<String, ClockRaceError> {
        let promise_id = golem_rust::create_promise();
        let promise = async {
            golem_rust::await_promise(&promise_id).await;
            "promise".to_string()
        };
        let timer = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                secs.saturating_mul(1_000_000_000),
            )
            .await;
            "timer".to_string()
        };
        Ok((promise, timer).race().await)
    }

    async fn polling_loop_vs_watchdog(&self) -> Result<String, ClockRaceError> {
        let flag = Rc::new(Cell::new(false));
        let set_flag = {
            let flag = flag.clone();
            async move {
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(3_000_000_000).await;
                flag.set(true);
            }
        };
        let poll = {
            let flag = flag.clone();
            async move {
                loop {
                    if flag.get() {
                        break "flag".to_string();
                    }
                    golem_rust::wasip3::clocks::monotonic_clock::wait_for(100_000_000).await;
                }
            }
        };
        let watchdog = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(600_000_000_000).await;
            "watchdog".to_string()
        };
        let (_, result) = (set_flag, (poll, watchdog).race()).join().await;
        Ok(result)
    }

    async fn follow_up(&self) -> Result<String, ClockRaceError> {
        Ok("settled".to_string())
    }
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct MatrixDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct MatrixRequest {
    pub source: String,
    pub dimensions: MatrixDimensions,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct MatrixResult {
    pub provider: String,
    pub command: String,
    pub normalized_source: String,
    pub weighted_size: i64,
    pub label_summary: String,
    pub principal: String,
    pub owner_agent_id: String,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct MatrixRejection {
    pub field: String,
    pub reason: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, ToolError)]
pub enum MatrixError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Rejected(MatrixRejection),
}

pub struct MatrixArtifactSubtree;

#[tool_definition(version = "1.0.0")]
pub trait MatrixCore {
    #[command(subtree = Artifact)]
    fn artifact(&self) -> MatrixArtifactSubtree;
}

struct MatrixCoreImpl;

#[tool_implementation]
impl MatrixCore for MatrixCoreImpl {
    fn artifact(&self) -> MatrixArtifactSubtree {
        MatrixArtifactSubtree
    }
}

#[tool_definition]
pub trait Artifact {
    async fn inspect(
        &self,
        request: MatrixRequest,
        multiplier: i64,
        principal: golem_rust::agentic::Principal,
    ) -> Result<MatrixResult, MatrixError>;
}

struct MatrixArtifactImpl;

fn matrix_principal(principal: &Principal) -> String {
    match principal {
        Principal::Anonymous => "anonymous".to_string(),
        Principal::Oidc(value) => format!("oidc:{}", value.sub),
        Principal::Agent(_) => "agent".to_string(),
        Principal::GolemUser(_) => "golem-user".to_string(),
    }
}

#[tool_implementation]
impl Artifact for MatrixArtifactImpl {
    async fn inspect(
        &self,
        request: MatrixRequest,
        multiplier: i64,
        principal: golem_rust::agentic::Principal,
    ) -> Result<MatrixResult, MatrixError> {
        if request.source == "reject.me" {
            return Err(MatrixError::Rejected(MatrixRejection {
                field: "request.source".to_string(),
                reason: "unsupported source".to_string(),
                retryable: false,
            }));
        }
        let metadata = golem_rust::get_self_metadata().expect("matrix owner metadata");
        Ok(MatrixResult {
            provider: "rust".to_string(),
            command: "artifact/inspect".to_string(),
            normalized_source: request.source.to_uppercase(),
            weighted_size: i64::from(request.dimensions.width)
                * i64::from(request.dimensions.height)
                * multiplier
                + request.labels.len() as i64,
            label_summary: request
                .labels
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("|"),
            principal: matrix_principal(&principal),
            owner_agent_id: metadata.agent_id.agent_id,
        })
    }
}

pub struct MatrixCapacityToken(QuotaToken);

impl WireSchema for MatrixCapacityToken {
    fn wire_type_id() -> String {
        QuotaToken::wire_type_id()
    }

    fn append_schema(builder: &mut WireSchemaBuilder) -> i32 {
        builder.push(wire::SchemaTypeBody::QuotaTokenType(wire::QuotaTokenSpec {
            resource_name: Some("matrix-capacity".to_string()),
        }))
    }
}

impl FromWire for MatrixCapacityToken {
    fn read_wire(reader: &mut WireReader, index: i32) -> Result<Self, WireError> {
        QuotaToken::read_wire(reader, index).map(Self)
    }
}

impl IntoWire for MatrixCapacityToken {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        self.0.preflight(resources)
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<i32, WireError> {
        self.0.write_wire(writer)
    }
}

impl IntoSchema for MatrixCapacityToken {
    fn type_id() -> TypeId {
        QuotaToken::type_id()
    }

    fn register_in(_builder: &mut SchemaBuilder) -> SchemaType {
        SchemaType::quota_token(QuotaTokenSpec {
            resource_name: Some("matrix-capacity".to_string()),
        })
    }

    fn to_value(&self) -> SchemaValue {
        self.0.to_value()
    }
}

impl FromSchema for MatrixCapacityToken {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        QuotaToken::from_value(value).map(Self)
    }
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct SecretExchange {
    pub provider: String,
    pub principal: String,
    pub owner_agent_id: String,
    pub revealed: bool,
    pub secret: GuestSecretHandle,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct QuotaExchange {
    pub provider: String,
    pub principal: String,
    pub owner_agent_id: String,
    pub reserved: bool,
    pub token: MatrixCapacityToken,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct PermissionExchange {
    pub provider: String,
    pub principal: String,
    pub owner_agent_id: String,
    pub card: GuestPermissionCardHandle,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(rename_all = "camelCase")]
pub struct PermissionIssue {
    pub card: GuestPermissionCardHandle,
    pub issuer: String,
    pub principal: String,
    pub owner_agent_id: String,
}

#[tool_definition(version = "1.0.0")]
pub trait MatrixPermissionIssuer {
    async fn issue(&self, principal: golem_rust::agentic::Principal) -> PermissionIssue;
}

struct MatrixPermissionIssuerImpl;

#[tool_implementation]
impl MatrixPermissionIssuer for MatrixPermissionIssuerImpl {
    async fn issue(&self, principal: Principal) -> PermissionIssue {
        let card = permission_derive::derive_from_wallet(&[], &[], &[], &[], None)
            .expect("matrix permission issuer derives a card from the owner wallet");
        let owner_agent_id = golem_rust::get_self_metadata()
            .expect("matrix permission issuer owner metadata")
            .agent_id
            .agent_id;
        PermissionIssue {
            card: GuestPermissionCardHandle::new(card),
            issuer: "rust".to_string(),
            principal: matrix_principal(&principal),
            owner_agent_id,
        }
    }
}

pub struct MatrixSecretSubtree;
pub struct MatrixQuotaSubtree;
pub struct MatrixPermissionsSubtree;
pub struct MatrixTypedSubtree;

#[tool_definition(version = "1.0.0")]
pub trait MatrixResource {
    #[command(subtree = Secret)]
    fn secret(&self) -> MatrixSecretSubtree;

    #[command(subtree = Quota)]
    fn quota(&self) -> MatrixQuotaSubtree;

    #[command(subtree = Permissions)]
    fn permissions(&self) -> MatrixPermissionsSubtree;

    #[command(subtree = Typed)]
    fn typed(&self) -> MatrixTypedSubtree;
}

struct MatrixResourceImpl;

#[tool_implementation]
impl MatrixResource for MatrixResourceImpl {
    fn secret(&self) -> MatrixSecretSubtree {
        MatrixSecretSubtree
    }

    fn quota(&self) -> MatrixQuotaSubtree {
        MatrixQuotaSubtree
    }

    fn permissions(&self) -> MatrixPermissionsSubtree {
        MatrixPermissionsSubtree
    }

    fn typed(&self) -> MatrixTypedSubtree {
        MatrixTypedSubtree
    }
}

fn matrix_resource_evidence(principal: &Principal) -> (String, String, String) {
    let owner_agent_id = golem_rust::get_self_metadata()
        .expect("matrix resource owner metadata")
        .agent_id
        .agent_id;
    (
        "rust".to_string(),
        matrix_principal(principal),
        owner_agent_id,
    )
}

#[tool_definition]
pub trait Secret {
    async fn exchange(
        &self,
        secret: GuestSecretHandle,
        principal: golem_rust::agentic::Principal,
    ) -> SecretExchange;
}

struct SecretImpl;

#[tool_implementation]
impl Secret for SecretImpl {
    async fn exchange(&self, secret: GuestSecretHandle, principal: Principal) -> SecretExchange {
        let (provider, principal, owner_agent_id) = matrix_resource_evidence(&principal);
        let revealed = reveal_string(&secret)
            .map(|value| value == "matrix-secret-value")
            .unwrap_or(false);
        SecretExchange {
            provider,
            principal,
            owner_agent_id,
            revealed,
            secret,
        }
    }
}

#[tool_definition]
pub trait Quota {
    async fn exchange(
        &self,
        token: MatrixCapacityToken,
        principal: golem_rust::agentic::Principal,
    ) -> QuotaExchange;
}

struct QuotaImpl;

#[tool_implementation]
impl Quota for QuotaImpl {
    async fn exchange(&self, token: MatrixCapacityToken, principal: Principal) -> QuotaExchange {
        let (provider, principal, owner_agent_id) = matrix_resource_evidence(&principal);
        let reserved = token
            .0
            .reserve(1)
            .map(|reservation| reservation.commit(1))
            .is_ok();
        QuotaExchange {
            provider,
            principal,
            owner_agent_id,
            reserved,
            token,
        }
    }
}

#[tool_definition]
pub trait Permissions {
    async fn exchange(
        &self,
        card: GuestPermissionCardHandle,
        principal: golem_rust::agentic::Principal,
    ) -> PermissionExchange;
}

struct PermissionsImpl;

#[tool_implementation]
impl Permissions for PermissionsImpl {
    async fn exchange(
        &self,
        card: GuestPermissionCardHandle,
        principal: Principal,
    ) -> PermissionExchange {
        assert_eq!(
            card.with_handle(permission_types::is_polymorphic),
            Some(false),
            "matrix resource permission card must be non-polymorphic"
        );
        let (provider, principal, owner_agent_id) = matrix_resource_evidence(&principal);
        PermissionExchange {
            provider,
            principal,
            owner_agent_id,
            card,
        }
    }
}

#[tool_definition]
pub trait Typed {
    fn transform(&self, input: AgentStream<u32>) -> AgentStream<u32>;
}

struct TypedImpl;

#[tool_implementation]
impl Typed for TypedImpl {
    fn transform(&self, mut input: AgentStream<u32>) -> AgentStream<u32> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Ok(Some(value)) = input.next().await {
                if writer.write_one(value * 3 + 1).await.is_err() {
                    break;
                }
            }
        });
        output
    }
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct EnvironmentProbeEvidence {
    pub marker: String,
    pub secret: String,
    pub reserved: bool,
}

#[tool_definition(version = "1.0.0")]
pub trait EnvironmentProbe {
    async fn observe(
        &self,
        expected_use: u64,
        amount: u64,
        commit_amount: u64,
    ) -> EnvironmentProbeEvidence;
}

struct EnvironmentProbeImpl;

fn owner_config_string(key: &str) -> Result<String, String> {
    let graph =
        golem_rust::schema::try_into_schema_graph::<String>().map_err(|error| error.to_string())?;
    let expected = encode_schema_graph(&graph).map_err(|error| error.to_string())?;
    let value = agent_host::get_config_value(&[key.to_string()], &expected)
        .map_err(|error| format!("{error:?}"))?;
    let value = decode_schema_value(value).map_err(|error| error.to_string())?;
    String::from_value(&value).map_err(|error| error.to_string())
}

#[tool_implementation]
impl EnvironmentProbe for EnvironmentProbeImpl {
    async fn observe(
        &self,
        expected_use: u64,
        amount: u64,
        commit_amount: u64,
    ) -> EnvironmentProbeEvidence {
        let marker = owner_config_string("marker").expect("caller owner marker is configured");
        let secret = ConfigSecret::<String>::new(vec!["secret".to_string()])
            .get()
            .expect("caller environment secret is readable and revealable");
        let token = QuotaToken::new("owner-capacity", expected_use);
        let reserved = match token.reserve(amount) {
            Ok(reservation) => {
                reservation.commit(commit_amount);
                true
            }
            Err(_) => false,
        };

        EnvironmentProbeEvidence {
            marker,
            secret,
            reserved,
        }
    }
}

#[tool_definition(version = "1.0.0")]
pub trait MiddlewareProbe {
    async fn apply(&self, value: String) -> String;
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct SecretPolicyObservation {
    pub label: String,
    pub config_resolved: bool,
    pub configured_secret_revealed: bool,
    pub input_secret_revealed: bool,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct SecretPolicyEvidence {
    pub middleware: Vec<SecretPolicyObservation>,
    pub leaf_revealed: bool,
}

#[tool_definition(version = "1.0.0")]
pub trait SecretPolicyProbe {
    async fn inspect(&self, value: GuestSecretHandle) -> SecretPolicyEvidence;
}

fn reveal_string(value: &GuestSecretHandle) -> Result<String, String> {
    let graph =
        golem_rust::schema::try_into_schema_graph::<String>().map_err(|error| error.to_string())?;
    let expected = encode_schema_graph(&graph).map_err(|error| error.to_string())?;
    let value = value
        .with_handle(|handle| {
            golem_rust::bindings::golem::secrets::reveal::reveal(handle, &expected)
        })
        .ok_or_else(|| "secret handle was transferred".to_string())?
        .map_err(|error| format!("{error:?}"))?;
    let value = decode_schema_value(value).map_err(|error| error.to_string())?;
    String::from_value(&value).map_err(|error| error.to_string())
}

struct SecretPolicyProbeImpl;

#[tool_implementation]
impl SecretPolicyProbe for SecretPolicyProbeImpl {
    async fn inspect(&self, value: GuestSecretHandle) -> SecretPolicyEvidence {
        SecretPolicyEvidence {
            middleware: Vec::new(),
            leaf_revealed: reveal_string(&value).is_ok(),
        }
    }
}

struct MiddlewareProbeImpl;

#[tool_implementation]
impl MiddlewareProbe for MiddlewareProbeImpl {
    async fn apply(&self, value: String) -> String {
        if value.starts_with("early-child(")
            || value.starts_with("race-cancelled(")
            || value.starts_with("race-detached(")
        {
            let _ = golem_rust::generate_idempotency_key();
            if value.starts_with("early-child(")
                && std::env::var("PROVIDER_PROMISE_CHECKPOINT_PORT").is_ok()
            {
                wait_at_promise_checkpoint("middleware-early-child").await;
                return format!("leaf({value})");
            }
            let checkpoint = if value.starts_with("early-child(") {
                "middleware-early-child"
            } else if value.starts_with("race-cancelled(") {
                "middleware-race-cancelled"
            } else {
                "middleware-race-detached"
            };
            wait_at_crash_checkpoint(&value, checkpoint).await;
            if value == "early-child(fail-after-parent)" {
                panic!("nested middleware child trap after parent return");
            }
        }
        if value.starts_with("partial-completed(")
            || value.starts_with("partial-pending(")
            || value.starts_with("approval-")
        {
            announce_middleware_probe_effect(&value).await;
            if value.starts_with("partial-pending(") {
                wait_at_crash_checkpoint(&value, "middleware-partial-pending").await;
            }
        }
        if value.starts_with("lifecycle-effect(") {
            announce_middleware_probe_effect(&value).await;
        }
        if value.starts_with("cascade-blocked(") {
            wait_at_crash_checkpoint(&value, "cascade-blocked-leaf").await;
        }
        if value.starts_with("cascade-trap(") {
            panic!("mixed lifecycle cascade trap");
        }
        if value.starts_with("rate-limit-crash(") {
            announce_middleware_probe_effect(&value).await;
            wait_at_crash_checkpoint(&value, "rate-limit-leaf-after-effect").await;
        }
        format!("leaf({value})")
    }
}

async fn announce_middleware_probe_effect(value: &str) {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("MIDDLEWARE_PROBE_EFFECT_PORT")
        .expect("middleware probe effect port is configured");
    let headers = types::Fields::from_list(&[]).expect("valid effect fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set effect method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set effect scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set effect authority");
    request
        .set_path_with_query(Some(&format!("/{value}")))
        .expect("set effect path");
    let send = async move { client::send(request).await.expect("send probe effect") };
    let finish = async move {
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish effect trailers");
        transmit.await.expect("transmit probe effect");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
}

#[derive(Debug, Clone, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct TypedOutputItem {
    pub ordinal: u32,
    pub label: String,
    pub asymmetric_extra: u64,
}

#[tool_definition(version = "1.0.0")]
pub trait TypedOutputStream {
    async fn produce(&self, tag: String) -> AgentStream<TypedOutputItem>;
}

struct TypedOutputStreamImpl;

#[tool_implementation]
impl TypedOutputStream for TypedOutputStreamImpl {
    async fn produce(&self, tag: String) -> AgentStream<TypedOutputItem> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            writer
                .write_one(TypedOutputItem {
                    ordinal: 11,
                    label: format!("{tag}-first"),
                    asymmetric_extra: 1_001,
                })
                .await
                .expect("write first typed tool output item");
            if std::env::var("PROVIDER_PROMISE_CHECKPOINT_PORT").is_ok() {
                wait_at_promise_checkpoint("typed-output-after-first").await;
            } else {
                wait_at_crash_checkpoint(&tag, "typed-output-after-first").await;
            }
            writer
                .write_all([
                    TypedOutputItem {
                        ordinal: 29,
                        label: format!("{tag}-second"),
                        asymmetric_extra: 2_003,
                    },
                    TypedOutputItem {
                        ordinal: 47,
                        label: format!("{tag}-third"),
                        asymmetric_extra: 4_009,
                    },
                ])
                .await
                .expect("write remaining typed tool output items");
        });
        output
    }
}

#[derive(Debug, Clone, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct TypedInputItem {
    pub label: String,
    pub ordinal: u32,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct TypedInputEvidence {
    pub label: String,
    pub ordinal: u32,
}

#[tool_definition(version = "1.0.0")]
pub trait TypedInputStream {
    async fn consume(&self, input: AgentStream<TypedInputItem>) -> Vec<TypedInputEvidence>;
}

struct TypedInputStreamImpl;

#[tool_implementation]
impl TypedInputStream for TypedInputStreamImpl {
    async fn consume(&self, mut input: AgentStream<TypedInputItem>) -> Vec<TypedInputEvidence> {
        let first = input
            .next()
            .await
            .expect("read first typed tool input item")
            .expect("typed tool input has a first item");
        wait_at_promise_checkpoint("typed-input-provider-consumed-first").await;
        let mut evidence = vec![TypedInputEvidence {
            label: first.label,
            ordinal: first.ordinal,
        }];
        while let Some(item) = input
            .next()
            .await
            .expect("read remaining typed tool input item")
        {
            evidence.push(TypedInputEvidence {
                label: item.label,
                ordinal: item.ordinal,
            });
        }
        evidence
    }
}

#[derive(Debug, Clone, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct StreamSummary {
    pub chunks_read: u32,
    pub bytes_read: u64,
    pub output_closed: bool,
}

#[derive(Debug, Clone, ToolError)]
pub enum StreamingError {
    #[tool_error(kind = "runtime-error", exit_code = 7)]
    Declared { bytes_read: u64 },
}

#[derive(Debug, Clone, ToolError)]
pub enum SecretEchoError {
    #[tool_error(kind = "runtime-error", exit_code = 8)]
    Returned { value: GuestSecretHandle },
}

#[derive(IntoSchema)]
struct RawRunInput {
    mode: String,
}

#[derive(IntoSchema)]
struct RawCapableInput {
    path: String,
}

#[tool_definition(version = "1.0.0")]
pub trait Streaming {
    async fn run(
        &self,
        mode: String,
        stdin: InputStream,
        stdout: OutputStream,
        principal: golem_rust::agentic::Principal,
    ) -> Result<StreamSummary, StreamingError>;

    async fn no_stream(&self, value: String) -> Result<String, StreamingError>;

    async fn echo_secret(
        &self,
        value: GuestSecretHandle,
        fail: bool,
    ) -> Result<GuestSecretHandle, SecretEchoError>;

    async fn optional_streams(
        &self,
        stdin: Option<InputStream>,
        stdout: Option<OutputStream>,
    ) -> Result<StreamSummary, StreamingError>;

    async fn produce(
        &self,
        chunk_count: u32,
        chunk_size: u32,
        stdout: OutputStream,
    ) -> Result<StreamSummary, StreamingError>;

    #[arg(diagnostics, channel = "stderr")]
    async fn dual_reconstruct(
        &self,
        mode: String,
        stdout: OutputStream,
        diagnostics: OutputStream,
    ) -> Result<StreamSummary, StreamingError>;
}

#[tool_definition(version = "1.0.0")]
pub trait CapableStreaming {
    async fn run_capable(
        &self,
        path: String,
        stdin: InputStream,
        stdout: OutputStream,
    ) -> Result<StreamSummary, StreamingError>;

    #[arg(diagnostics, channel = "stderr")]
    async fn dual_pressure(
        &self,
        path: String,
        output_size: u64,
        checkpoint_before_terminal: bool,
        stdout: OutputStream,
        diagnostics: OutputStream,
    ) -> Result<StreamSummary, StreamingError>;
}

struct StreamingImpl;

async fn write_chunk(stdout: &mut OutputStream, chunk: Vec<u8>) -> bool {
    stdout.write(chunk).await.is_ok()
}

async fn stream_through_http(
    mode: String,
    mut stdin: InputStream,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("HTTP_GATE_PORT").expect("HTTP_GATE_PORT is configured");
    let tag = mode
        .strip_prefix("http-")
        .expect("HTTP streaming mode has a tag");
    let headers =
        types::Fields::from_list(&[("x-stream-tag".to_string(), tag.as_bytes().to_vec())])
            .expect("valid HTTP fields");
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set HTTP method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set HTTP scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set HTTP authority");
    request
        .set_path_with_query(Some(&format!("/{tag}")))
        .expect("set HTTP path");

    let upload = async move {
        let mut chunks_read = 0;
        let mut bytes_read = 0;
        while let Some(item) = stdin.next().await {
            let chunk = item.expect("tool stdin failed during HTTP upload");
            chunks_read += 1;
            bytes_read += chunk.len() as u64;
            assert!(
                body_tx.write_all(chunk).await.is_empty(),
                "HTTP request body closed before tool stdin"
            );
        }
        drop(body_tx);
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish HTTP request trailers");
        (chunks_read, bytes_read)
    };
    let download = async move {
        let response = client::send(request).await.expect("send HTTP request");
        assert_eq!(response.get_status_code(), 200);
        let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
        let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
        let mut buffer = Vec::with_capacity(4096);
        let mut output_closed = false;
        loop {
            let (result, next_buffer) = body.read(buffer).await;
            buffer = next_buffer;
            match result {
                StreamResult::Complete(len) => {
                    if stdout.write(buffer[..len].to_vec()).await.is_err() {
                        output_closed = true;
                        break;
                    }
                    buffer.clear();
                }
                StreamResult::Dropped => break,
                StreamResult::Cancelled => panic!("HTTP response body read was cancelled"),
            }
        }
        drop(body);
        trailers.await.expect("read HTTP response trailers");
        response_done_tx
            .write(Ok(()))
            .await
            .expect("acknowledge HTTP response body");
        let _ = stdout.finish().await;
        output_closed
    };
    let finish_transmit = async move {
        transmit.await.expect("transmit HTTP request body");
    };

    let ((chunks_read, bytes_read), output_closed, ()) =
        (upload, download, finish_transmit).join().await;
    Ok(StreamSummary {
        chunks_read,
        bytes_read,
        output_closed,
    })
}

async fn record_native_order_external_effect() {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port =
        std::env::var("NATIVE_ORDER_HTTP_PORT").expect("NATIVE_ORDER_HTTP_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid native-order HTTP fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set native-order HTTP method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set native-order HTTP scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set native-order HTTP authority");
    request
        .set_path_with_query(Some("/"))
        .expect("set native-order HTTP path");
    let receive_response = async move {
        client::send(request)
            .await
            .expect("send native-order HTTP request")
    };
    let finish_request = async move {
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish native-order HTTP request");
        transmit.await.expect("transmit native-order HTTP request");
    };
    let (response, ()) = (receive_response, finish_request).join().await;
    assert_eq!(response.get_status_code(), 204);
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (body, trailers) = types::Response::consume_body(response, response_done_rx);
    response_done_tx
        .write(Ok(()))
        .await
        .expect("finish native-order HTTP response");
    drop(body);
    drop(trailers);
}

fn raw_run_input(mode: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawRunInput {
        mode: mode.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode nested tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode nested tool wire input")
}

fn raw_capable_input(path: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawCapableInput {
        path: path.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode nested capable tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode nested capable tool wire input")
}

fn nested_input(bytes: Vec<u8>) -> InputStream {
    let (mut writer, reader) =
        golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
    spawn_local(async move {
        if !bytes.is_empty() {
            let _ = writer.write_all(vec![Ok(bytes)]).await;
        }
    });
    reader
}

fn launch_retained_crash_child() {
    ToolRpc::create("streaming")
        .expect("tool RPC creation failed")
        .invoke(
            &["run".to_string()],
            raw_run_input("hold-capable-terminal-child"),
            Some(pump_tool_stdin(nested_input(Vec::new()))),
        )
        .expect("launch retained incapable crash-checkpoint child");
}

fn launch_atomic_idempotency_child() {
    ToolRpc::create("streaming")
        .expect("tool RPC creation failed")
        .invoke(
            &["run".to_string()],
            raw_run_input("atomic-idempotency-child"),
            Some(pump_tool_stdin(nested_input(Vec::new()))),
        )
        .expect("launch atomic idempotency child");
}

async fn send_idempotent_effect() {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port =
        std::env::var("IDEMPOTENCY_EFFECT_PORT").expect("IDEMPOTENCY_EFFECT_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid effect fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set effect method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set effect scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set effect authority");
    request
        .set_path_with_query(Some("/effect"))
        .expect("set effect path");
    let receive_response =
        async move { client::send(request).await.expect("send idempotent effect") };
    let finish_request = async move {
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish effect request");
        transmit.await.expect("transmit effect request");
    };
    let (response, ()) = (receive_response, finish_request).join().await;
    assert_eq!(response.get_status_code(), 200);
}

fn principal_class(principal: &Principal) -> &'static str {
    match principal {
        Principal::Anonymous => "anonymous",
        Principal::Oidc(_) => "oidc",
        Principal::Agent(_) => "agent",
        Principal::GolemUser(_) => "golem-user",
    }
}

async fn run_nested_principal(
    principal: &Principal,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    use futures_concurrency::prelude::*;

    let outer_class = principal_class(principal);
    let rpc = ToolRpc::create("streaming").expect("tool RPC creation failed");
    let (nested_target, nested_stdout) = tool_host::create_output();
    let nested = rpc.invoke_and_await(
        vec!["run".to_string()],
        raw_run_input("principal"),
        Some(pump_tool_stdin(nested_input(Vec::new()))),
        Some(nested_target),
        None,
    );
    let (nested_result, nested_output) = (nested, async move {
        let mut output = Vec::new();
        let mut nested_stdout = nested_stdout;
        while let Some(item) = nested_stdout.next().await {
            output.extend(item.expect("nested principal stdout failed"));
        }
        output
    })
        .join()
        .await;
    nested_result.expect("nested principal tool result");
    let nested_class = String::from_utf8(nested_output).expect("principal class is UTF-8");
    assert_eq!(nested_class, outer_class);
    let output_closed = !write_chunk(
        &mut stdout,
        format!("{outer_class}:{nested_class}").into_bytes(),
    )
    .await;
    let _ = stdout.finish().await;
    Ok(StreamSummary {
        chunks_read: 0,
        bytes_read: 0,
        output_closed,
    })
}

async fn run_nested_capable(bytes: Vec<u8>) -> Vec<u8> {
    use futures_concurrency::prelude::*;

    let rpc = ToolRpc::create("capable-streaming").expect("tool RPC creation failed");
    let (stdout_target, nested_stdout) = tool_host::create_output();
    let nested = rpc.invoke_and_await(
        vec!["run-capable".to_string()],
        raw_capable_input("order:N:/capable-nested-inner.bin"),
        Some(pump_tool_stdin(nested_input(bytes))),
        Some(stdout_target),
        None,
    );
    let (result, output) = (nested, async move {
        let mut stdout = nested_stdout;
        let mut output = Vec::new();
        while let Some(item) = stdout.next().await {
            output.extend(item.expect("nested capable stdout failed"));
        }
        output
    })
        .join()
        .await;
    result.expect("nested capable result");
    output
}

async fn run_nested(
    stdin: InputStream,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    use futures_concurrency::prelude::*;

    let rpc = ToolRpc::create("streaming").expect("tool RPC creation failed");
    let (nested_target, mut nested_stdout) = tool_host::create_output();
    let nested = rpc.invoke_and_await(
        vec!["run".to_string()],
        raw_run_input("marker-echo"),
        Some(golem_rust::agentic::pump_tool_stdin(stdin)),
        Some(nested_target),
        None,
    );
    let forward = async move {
        let mut chunks_read = 0;
        let mut bytes_read = 0;
        let mut output_closed = false;
        while let Some(item) = nested_stdout.next().await {
            let chunk = item.expect("nested tool stdout failed");
            chunks_read += 1;
            bytes_read += chunk.len() as u64;
            if stdout.write(chunk).await.is_err() {
                output_closed = true;
                break;
            }
        }
        let _ = stdout.finish().await;
        (chunks_read, bytes_read, output_closed)
    };
    let (nested_result, (chunks_read, bytes_read, output_closed)) = (nested, forward).join().await;
    nested_result.expect("nested streaming tool result");
    Ok(StreamSummary {
        chunks_read,
        bytes_read,
        output_closed,
    })
}

async fn run_nested_capable_parent_end(
    stdin: InputStream,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    let rpc = ToolRpc::create("capable-streaming").expect("tool RPC creation failed");
    let (nested_target, nested_stdout) = tool_host::create_output();
    let nested = rpc.async_invoke_and_await(
        &["run-capable".to_string()],
        raw_capable_input("/nested-capable-parent-end.bin"),
        Some(golem_rust::agentic::pump_tool_stdin(stdin)),
        Some(nested_target),
        None,
    );
    drop(nested);
    drop(nested_stdout);
    let output_closed = !write_chunk(&mut stdout, b"nested-capable-started".to_vec()).await;
    Ok(StreamSummary {
        chunks_read: 0,
        bytes_read: 0,
        output_closed,
    })
}

fn write_owner_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    let (root, _) = wasi::filesystem::preopens::get_directories()
        .into_iter()
        .next()
        .ok_or_else(|| "capable tool has no preopened owner filesystem".to_string())?;
    let file = root
        .open_at(
            PathFlags::empty(),
            path.trim_start_matches('/'),
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
            DescriptorFlags::WRITE,
        )
        .map_err(|error| format!("failed to open owner file: {error:?}"))?;
    let stream = file
        .write_via_stream(0)
        .map_err(|error| format!("failed to open owner file stream: {error:?}"))?;
    stream
        .blocking_write_and_flush(bytes)
        .map_err(|error| format!("failed to write owner file: {error:?}"))
}

async fn is_first_trap_attempt() -> bool {
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
    };
    use golem_rust::wasip3::wit_bindgen::StreamResult;

    let port = std::env::var("TRAP_ONCE_PORT")
        .expect("TRAP_ONCE_PORT is configured")
        .parse()
        .expect("TRAP_ONCE_PORT is a valid port");
    let socket = TcpSocket::create(IpAddressFamily::Ipv4).expect("create trap-attempt socket");
    socket
        .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            address: (127, 0, 0, 1),
            port,
        }))
        .await
        .expect("connect to trap-attempt server");
    let (mut stream, received) = socket.receive();
    let mut bytes = Vec::new();
    let mut buffer = Vec::with_capacity(1);
    loop {
        let (result, next_buffer) = stream.read(buffer).await;
        buffer = next_buffer;
        match result {
            StreamResult::Complete(len) => {
                bytes.extend_from_slice(&buffer[..len]);
                buffer.clear();
            }
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("trap-attempt read was cancelled"),
        }
    }
    drop(stream);
    received.await.expect("finish trap-attempt receive");
    assert_eq!(bytes.len(), 1, "trap-attempt server returns one byte");
    bytes[0] == 0
}

async fn wait_at_crash_checkpoint<T>(_retained: &T, name: &str) {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
    };
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("PROVIDER_CRASH_CHECKPOINT_PORT")
        .or_else(|_| std::env::var("CRASH_CHECKPOINT_PORT"))
        .expect("provider crash checkpoint port is configured");
    let gate_port = std::env::var("PROVIDER_CRASH_CHECKPOINT_GATE_PORT")
        .or_else(|_| std::env::var("CRASH_CHECKPOINT_GATE_PORT"))
        .expect("provider crash checkpoint gate port is configured")
        .parse()
        .expect("provider crash checkpoint gate port is a valid port");
    let headers = types::Fields::from_list(&[]).expect("valid checkpoint fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set checkpoint method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set checkpoint scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set checkpoint authority");
    request
        .set_path_with_query(Some(&format!("/{name}")))
        .expect("set checkpoint path");
    let receive_response = async move {
        client::send(request)
            .await
            .expect("send checkpoint request")
    };
    let finish_request = async move {
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish checkpoint request");
        transmit.await.expect("transmit checkpoint request");
    };
    let (response, ()) = (receive_response, finish_request).join().await;
    assert_eq!(response.get_status_code(), 204);
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (body, trailers) = types::Response::consume_body(response, response_done_rx);
    response_done_tx
        .write(Ok(()))
        .await
        .expect("finish checkpoint response");
    drop(body);
    drop(trailers);

    golem_rust::atomically_async(|| async {
        let socket =
            TcpSocket::create(IpAddressFamily::Ipv4).expect("create checkpoint gate socket");
        socket
            .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port: gate_port,
            }))
            .await
            .expect("connect to checkpoint gate");
        let (mut stream, received) = socket.receive();
        let mut bytes = Vec::new();
        let mut buffer = Vec::with_capacity(1);
        loop {
            let (result, next_buffer) = stream.read(buffer).await;
            buffer = next_buffer;
            match result {
                StreamResult::Complete(len) => {
                    bytes.extend_from_slice(&buffer[..len]);
                    buffer.clear();
                }
                StreamResult::Dropped => break,
                StreamResult::Cancelled => panic!("checkpoint gate read was cancelled"),
            }
        }
        drop(stream);
        received.await.expect("finish checkpoint gate receive");
        assert_eq!(bytes, [1], "checkpoint gate returns one release byte");
    })
    .await;
}

async fn wait_at_promise_checkpoint(name: &str) {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::{wit_future, wit_stream};

    let promise = golem_rust::create_promise();
    let port = std::env::var("PROVIDER_PROMISE_CHECKPOINT_PORT")
        .expect("provider promise checkpoint port is configured");
    let headers = types::Fields::from_list(&[]).expect("valid checkpoint fields");
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set checkpoint method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set checkpoint scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set checkpoint authority");
    request
        .set_path_with_query(Some(&format!("/{name}")))
        .expect("set checkpoint path");
    let payload = promise.oplog_idx.to_string().into_bytes();
    let send = async move {
        client::send(request)
            .await
            .expect("send checkpoint request")
    };
    let finish = async move {
        assert!(body_tx.write_all(payload).await.is_empty());
        drop(body_tx);
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish checkpoint trailers");
        transmit.await.expect("transmit checkpoint request");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
    golem_rust::await_promise(&promise).await;
}

fn append_owner_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    let (root, _) = wasi::filesystem::preopens::get_directories()
        .into_iter()
        .next()
        .ok_or_else(|| "capable tool has no preopened owner filesystem".to_string())?;
    let file = root
        .open_at(
            PathFlags::empty(),
            path.trim_start_matches('/'),
            OpenFlags::CREATE,
            DescriptorFlags::READ | DescriptorFlags::WRITE,
        )
        .map_err(|error| format!("failed to open owner append file: {error:?}"))?;
    let offset = file
        .stat()
        .map_err(|error| format!("failed to stat owner append file: {error:?}"))?
        .size;
    let stream = file
        .write_via_stream(offset)
        .map_err(|error| format!("failed to open owner append stream: {error:?}"))?;
    stream
        .blocking_write_and_flush(bytes)
        .map_err(|error| format!("failed to append owner file: {error:?}"))
}

#[tool_implementation]
impl Streaming for StreamingImpl {
    async fn run(
        &self,
        mode: String,
        mut stdin: InputStream,
        mut stdout: OutputStream,
        principal: golem_rust::agentic::Principal,
    ) -> Result<StreamSummary, StreamingError> {
        if mode.starts_with("http-") {
            return stream_through_http(mode, stdin, stdout).await;
        }
        if mode == "principal" {
            let output_closed =
                !write_chunk(&mut stdout, principal_class(&principal).as_bytes().to_vec()).await;
            let _ = stdout.finish().await;
            return Ok(StreamSummary {
                chunks_read: 0,
                bytes_read: 0,
                output_closed,
            });
        }
        if mode == "nested-principal" {
            return run_nested_principal(&principal, stdout).await;
        }
        if mode == "nested" {
            return run_nested(stdin, stdout).await;
        }
        if mode == "nested-capable-parent-end" {
            return run_nested_capable_parent_end(stdin, stdout).await;
        }

        let mut summary = StreamSummary {
            chunks_read: 0,
            bytes_read: 0,
            output_closed: false,
        };

        if matches!(
            mode.as_str(),
            "marker-echo"
                | "trap"
                | "trap-after-clean-eof"
                | "declared-error"
                | "explicit-stdout-failure"
                | "finish-failure"
                | "writer-abandonment"
                | "finish-after-reader-drop"
                | "stream-failure-success"
        ) {
            if matches!(mode.as_str(), "declared-error" | "stream-failure-success") {
                for byte in MARKER {
                    if !write_chunk(&mut stdout, vec![*byte]).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            } else {
                summary.output_closed = !write_chunk(&mut stdout, MARKER.to_vec()).await;
            }
        }

        match mode.as_str() {
            "empty" => {}
            "binary" => {
                summary.output_closed =
                    !write_chunk(&mut stdout, vec![0, 255, 1, 128, 0, 13, 10, 254]).await;
            }
            "fragmented" => {
                for byte in [b'f', b'r', b'a', b'g', 0, 255] {
                    if !write_chunk(&mut stdout, vec![byte]).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
            "large" => {
                for index in 0..512_u32 {
                    let chunk = vec![(index % 251) as u8; 4096];
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
            "backpressure" => {
                for index in 0..8192_u32 {
                    let chunk = vec![(index % 251) as u8; 4096];
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
            "early-stdin-close" => {
                summary.output_closed = !write_chunk(&mut stdout, b"stdin-ignored".to_vec()).await;
            }
            "early-stdout-close" => {
                let _ = stdout.finish().await;
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                return Ok(summary);
            }
            "explicit-stdout-failure" => {
                stdout
                    .fail(ByteStreamFailure::Failed(
                        "rust provider explicit stdout failure".to_string(),
                    ))
                    .await
                    .expect("fail stdout explicitly");
                return Ok(summary);
            }
            "writer-abandonment" => return Ok(summary),
            "finish-after-reader-drop" => {
                while stdin.next().await.is_some() {}
                summary.output_closed = stdout.finish().await.is_err();
                return Ok(summary);
            }
            "hold-after-eof" => {
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.write(b"eof-observed".to_vec()).await;
                wait_at_crash_checkpoint(&stdout, "after-eof-before-terminal").await;
            }
            "hold-large-after-eof" => {
                while stdin.next().await.is_some() {}
                stdout
                    .write(vec![b'h'; 8 * 1024 * 1024])
                    .await
                    .expect("buffer incomplete replay attachment");
                wait_at_crash_checkpoint(&stdout, "after-large-eof-before-terminal").await;
            }
            "hold-after-stdout-terminal" => {
                let _ = stdout.write(b"ready".to_vec()).await;
                if let Some(Ok(chunk)) = stdin.next().await {
                    summary.chunks_read += 1;
                    summary.bytes_read += chunk.len() as u64;
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                    }
                }
                let _ = stdout.finish().await;
                while stdin.next().await.is_some() {}
                return Ok(summary);
            }
            "hold-capable-terminal-child" => {
                let _ = golem_rust::generate_idempotency_key();
                wait_at_crash_checkpoint(&stdout, "capable-terminal-retained-child").await;
            }
            "atomic-idempotency-child" => {
                let _ = golem_rust::generate_idempotency_key();
                send_idempotent_effect().await;
            }
            "historical-reconstruction-gate" => {
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.finish().await;
                wait_at_crash_checkpoint(&summary, "historical-reconstruction-body").await;
                return Ok(summary);
            }
            "historical-reconstruction-exclusive" => {
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.finish().await;
                return Ok(summary);
            }
            "entity-custom-root" => {
                // A positional atomic region of this body, then a root custom invocation of this
                // body: both are recorded under the entity invocation.
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.finish().await;
                golem_rust::atomically_async(|| async {}).await;
                golem_rust::durability::Durability::<(), String>::new(
                    "golem-it",
                    "entity-custom-root",
                    golem_rust::durability::DurableFunctionType::WriteLocal,
                    &(),
                )
                .run_infallible_async(|| async {})
                .await;
                return Ok(summary);
            }
            "stream-failure-success" => {
                let _ = stdout
                    .fail(ByteStreamFailure::Failed(
                        "provider-selected-failure".to_string(),
                    ))
                    .await;
                return Ok(summary);
            }
            _ => {
                while let Some(item) = stdin.next().await {
                    let Ok(chunk) = item else {
                        break;
                    };
                    summary.chunks_read += 1;
                    summary.bytes_read += chunk.len() as u64;
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
        }

        if mode == "declared-error" {
            return Err(StreamingError::Declared {
                bytes_read: summary.bytes_read,
            });
        }
        if mode == "trap" {
            panic!("deterministic streaming tool trap");
        }
        if mode == "trap-after-clean-eof" {
            let _ = stdout.finish().await;
            wait_at_crash_checkpoint(&(), "provider-clean-stdout-before-trap").await;
            panic!("deterministic streaming tool trap after clean stdout");
        }
        if mode == "changing-stdout-in-atomic-region" {
            golem_rust::atomically_async(|| async {
                let first_attempt = is_first_trap_attempt().await;
                stdout
                    .write(if first_attempt {
                        b"first".to_vec()
                    } else {
                        b"second".to_vec()
                    })
                    .await
                    .expect("publish attempt-dependent stdout");
                stdout
                    .finish()
                    .await
                    .expect("finish attempt-dependent stdout");
                if first_attempt {
                    wait_at_crash_checkpoint(&(), "provider-changing-stdout").await;
                }
            })
            .await;
            return Ok(summary);
        }

        summary.output_closed |= stdout.finish().await.is_err();
        Ok(summary)
    }

    async fn no_stream(&self, value: String) -> Result<String, StreamingError> {
        if value == "first" || value == "second" {
            let _ = golem_rust::get_oplog_index();
        }
        if value == "hold-attempt-identity" {
            wait_at_crash_checkpoint(&value, "attempt-identity-accepted").await;
        }
        if value == "native-error" {
            append_owner_file("/native-tool-order.log", b"E")
                .expect("append native declared-error invocation order");
            return Err(StreamingError::Declared { bytes_read: 0 });
        }
        if value == "native-order" {
            record_native_order_external_effect().await;
            append_owner_file("/native-tool-order.log", b"T")
                .expect("append native external tool invocation order");
        }
        Ok(format!("no-stream:{value}"))
    }

    async fn dual_reconstruct(
        &self,
        mode: String,
        mut stdout: OutputStream,
        mut diagnostics: OutputStream,
    ) -> Result<StreamSummary, StreamingError> {
        if mode == "redaction-terminals" {
            for byte in MARKER {
                stdout.write(vec![*byte]).await.unwrap();
            }
            for byte in b"stderr-visible" {
                diagnostics.write(vec![*byte]).await.unwrap();
            }
            stdout.finish().await.unwrap();
            diagnostics.finish().await.unwrap();
            return Ok(StreamSummary {
                chunks_read: 0,
                bytes_read: 0,
                output_closed: false,
            });
        }
        announce_middleware_probe_effect(&format!("dual-reconstruct-{mode}")).await;
        match mode.as_str() {
            "before-either-output" => {}
            "after-stdout-only" => {
                stdout.write(b"stdout-first".to_vec()).await.unwrap();
            }
            "after-stderr-only" => {
                diagnostics.write(b"stderr-first".to_vec()).await.unwrap();
            }
            "after-both-partial" => {
                stdout.write(b"stdout-first".to_vec()).await.unwrap();
                diagnostics.write(b"stderr-first".to_vec()).await.unwrap();
            }
            "after-stdout-terminal" => {
                stdout.write(b"stdout-first".to_vec()).await.unwrap();
                stdout.clone().finish().await.unwrap();
            }
            other => panic!("unknown dual-output reconstruction mode: {other}"),
        }
        wait_at_crash_checkpoint(&mode, &mode).await;
        if mode != "after-stdout-terminal" {
            stdout.write(b"stdout-last".to_vec()).await.unwrap();
            stdout.finish().await.unwrap();
        }
        diagnostics.write(b"stderr-last".to_vec()).await.unwrap();
        diagnostics.finish().await.unwrap();
        Ok(StreamSummary {
            chunks_read: 0,
            bytes_read: 0,
            output_closed: false,
        })
    }

    async fn echo_secret(
        &self,
        value: GuestSecretHandle,
        fail: bool,
    ) -> Result<GuestSecretHandle, SecretEchoError> {
        if fail {
            Err(SecretEchoError::Returned { value })
        } else {
            Ok(value)
        }
    }

    async fn optional_streams(
        &self,
        mut stdin: Option<InputStream>,
        mut stdout: Option<OutputStream>,
    ) -> Result<StreamSummary, StreamingError> {
        let mut summary = StreamSummary {
            chunks_read: 0,
            bytes_read: 0,
            output_closed: false,
        };
        if let Some(stdin) = stdin.as_mut() {
            while let Some(item) = stdin.next().await {
                let Ok(chunk) = item else {
                    break;
                };
                summary.chunks_read += 1;
                summary.bytes_read += chunk.len() as u64;
                if let Some(stdout) = stdout.as_mut()
                    && stdout.write(chunk).await.is_err()
                {
                    summary.output_closed = true;
                    break;
                }
            }
        }
        if let Some(stdout) = stdout {
            let _ = stdout.finish().await;
        }
        Ok(summary)
    }

    async fn produce(
        &self,
        chunk_count: u32,
        chunk_size: u32,
        mut stdout: OutputStream,
    ) -> Result<StreamSummary, StreamingError> {
        let mut output_closed = false;
        for index in 0..chunk_count {
            let chunk = vec![(index % 251) as u8; chunk_size as usize];
            if !write_chunk(&mut stdout, chunk).await {
                output_closed = true;
                break;
            }
        }
        let _ = stdout.finish().await;
        Ok(StreamSummary {
            chunks_read: chunk_count,
            bytes_read: u64::from(chunk_count) * u64::from(chunk_size),
            output_closed,
        })
    }
}

struct CapableStreamingImpl;

#[tool_implementation]
impl CapableStreaming for CapableStreamingImpl {
    async fn run_capable(
        &self,
        path: String,
        mut stdin: InputStream,
        mut stdout: OutputStream,
    ) -> Result<StreamSummary, StreamingError> {
        let mut bytes = Vec::new();
        let mut chunks_read = 0;
        while let Some(item) = stdin.next().await {
            let Ok(chunk) = item else {
                break;
            };
            chunks_read += 1;
            bytes.extend(chunk);
        }

        let output = if let Some(rest) = path.strip_prefix("order:") {
            let (tag, path) = rest
                .split_once(':')
                .expect("ordered capable path contains a tag and file path");
            write_owner_file(path, &bytes).expect("capable tool must share the owner filesystem");
            append_owner_file("/capable-order.log", tag.as_bytes())
                .expect("append capable execution order");
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("nested-capable:") {
            let nested_output = run_nested_capable(bytes.clone()).await;
            assert_eq!(nested_output, bytes);
            write_owner_file(path, &nested_output)
                .expect("outer nested capable tool must share the owner filesystem");
            append_owner_file("/capable-order.log", b"O")
                .expect("append outer nested capable execution order");
            nested_output
        } else if let Some(path) = path.strip_prefix("stdout-exact:") {
            write_owner_file(path, &bytes).expect("capable tool must share the owner filesystem");
            vec![b'x'; 64]
        } else if let Some(path) = path.strip_prefix("stdout-over:") {
            write_owner_file(path, &bytes).expect("capable tool must share the owner filesystem");
            vec![b'x'; 65]
        } else if let Some(path) = path.strip_prefix("trap-once:") {
            write_owner_file(path, &bytes)
                .expect("capable trap marker must share the owner filesystem");
            let _effect = golem_rust::generate_idempotency_key();
            golem_rust::atomically_async(|| async {
                if is_first_trap_attempt().await {
                    panic!("deterministic capable streaming tool first-attempt trap");
                }
            })
            .await;
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("hold-body:") {
            write_owner_file(path, &bytes)
                .expect("capable body checkpoint must share the owner filesystem");
            let _ = golem_rust::get_oplog_index();
            stdout
                .write(b"body-checkpoint".to_vec())
                .await
                .expect("publish capable body checkpoint");
            wait_at_crash_checkpoint(&stdout, "capable-body").await;
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("hold-completion:") {
            write_owner_file(path, &bytes)
                .expect("capable completion checkpoint must share the owner filesystem");
            stdout
                .write(bytes.clone())
                .await
                .expect("buffer capable output before the checkpoint");
            append_owner_file(path, b":buffered")
                .expect("record buffered capable completion checkpoint");
            let _ = golem_rust::get_oplog_index();
            stdout
                .write(b"completion-checkpoint".to_vec())
                .await
                .expect("publish capable completion checkpoint");
            wait_at_crash_checkpoint(&stdout, "capable-completion").await;
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("hold-publication:") {
            write_owner_file(path, &bytes)
                .expect("capable publication checkpoint must share the owner filesystem");
            stdout
                .write(bytes.clone())
                .await
                .expect("buffer capable output before lane return");
            wait_at_crash_checkpoint(&stdout, "provider-capable-before-publication").await;
            Vec::new()
        } else if let Some(path) = path.strip_prefix("hold-terminal:") {
            write_owner_file(path, &bytes)
                .expect("capable terminal checkpoint must share the owner filesystem");
            launch_retained_crash_child();
            bytes.clone()
        } else if path == "atomic-idempotency-parent" {
            golem_rust::atomically_async(|| async {
                launch_atomic_idempotency_child();
            })
            .await;
            bytes.clone()
        } else {
            write_owner_file(&path, &bytes).expect("capable tool must share the owner filesystem");
            bytes.clone()
        };
        let output_closed = stdout.write(output).await.is_err();
        let _ = stdout.finish().await;
        Ok(StreamSummary {
            chunks_read,
            bytes_read: bytes.len() as u64,
            output_closed,
        })
    }

    async fn dual_pressure(
        &self,
        path: String,
        output_size: u64,
        checkpoint_before_terminal: bool,
        mut stdout: OutputStream,
        mut diagnostics: OutputStream,
    ) -> Result<StreamSummary, StreamingError> {
        let file_bytes = vec![b'i'; output_size as usize];
        write_owner_file(&path, &file_bytes)
            .expect("dual-pressure tool must share the owner filesystem");
        stdout
            .write(vec![b'o'; output_size as usize])
            .await
            .expect("buffer dual-pressure stdout");
        diagnostics
            .write(vec![b'e'; output_size as usize])
            .await
            .expect("buffer dual-pressure stderr");
        if checkpoint_before_terminal {
            wait_at_crash_checkpoint(&stdout, "after-dual-output-before-terminal").await;
        }
        stdout.finish().await.expect("finish dual-pressure stdout");
        diagnostics
            .finish()
            .await
            .expect("finish dual-pressure stderr");
        Ok(StreamSummary {
            chunks_read: 0,
            bytes_read: output_size,
            output_closed: false,
        })
    }
}
