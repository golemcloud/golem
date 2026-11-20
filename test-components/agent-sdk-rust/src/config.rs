use golem_rust::agentic::{Config, Secret};
use golem_rust::bindings::golem::secrets::{reveal, types};
use golem_rust::secrets::GuestSecretHandle;
use golem_rust::{
    ConfigSchema, FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, agent_definition,
    agent_implementation, decode_schema_value, encode_schema_graph,
};
use golem_rust::{PromiseId, blocking_await_promise, create_promise};
use serde::Serialize;
use serde_json::json;

#[derive(Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
pub struct StaticDefaultSettings {
    pub label: String,
    pub count: u32,
}

#[derive(Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
#[schema(union)]
pub enum StaticDefaultChoice {
    #[schema(prefix = "selected:")]
    Text(String),
    #[schema(prefix = "other:")]
    Other(String),
}

#[derive(ConfigSchema, Serialize)]
pub struct StaticDefaultFields {
    #[config_schema(default = r#"{"label":"code-default","count":31}"#)]
    pub settings: StaticDefaultSettings,
    #[config_schema(default = r#""selected:value""#)]
    pub choice: StaticDefaultChoice,
    #[config_schema(default = r#"[null,7,"retained"]"#)]
    pub slots: (Option<i32>, Option<i32>, String),
    pub omitted: Option<String>,
}

#[agent_definition]
pub trait StaticDefaultsAgent {
    fn new(name: String, #[agent_config] config: Config<StaticDefaultFields>) -> Self;

    fn read(&self) -> String;
}

struct StaticDefaultsAgentImpl {
    config: Config<StaticDefaultFields>,
}

#[agent_implementation]
impl StaticDefaultsAgent for StaticDefaultsAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<StaticDefaultFields>) -> Self {
        Self { config }
    }

    fn read(&self) -> String {
        serde_json::to_string(&self.config.get().expect("static config is captured")).unwrap()
    }
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
pub struct StaticMail {
    pub kind: String,
    pub address: String,
    pub attempts: u32,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
#[schema(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct StaticQueue {
    pub kind: String,
    pub queue_name: String,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
#[schema(union)]
#[serde(untagged)]
pub enum StaticDelivery {
    #[schema(rename = "mail", field_equals(field = "kind", literal = "mail"))]
    Mail(StaticMail),
    #[schema(rename = "queue", field_equals(field = "kind", literal = "queue"))]
    Queue(StaticQueue),
}

#[derive(ConfigSchema, Serialize)]
pub struct StaticDeliveryFields {
    #[config_schema(default = r#"{"kind":"mail","address":"ops@example.test","attempts":3}"#)]
    pub delivery: StaticDelivery,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
#[schema(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct StaticNode {
    pub display_name: String,
    pub next: Option<Box<StaticNode>>,
}

#[derive(ConfigSchema, Serialize)]
pub struct StaticRecursiveFields {
    #[config_schema(
        default = r#"{"displayName":"outer","next":{"displayName":"inner","next":null}}"#
    )]
    pub tree: StaticNode,
}

#[derive(ConfigSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticNamedFields {
    #[config_schema(default = "19")]
    pub max_chars: u32,
}

#[derive(ConfigSchema, Serialize)]
pub struct StaticRequiredFields {
    pub required: String,
}

#[derive(ConfigSchema, Serialize)]
pub struct StaticOptionalFields {
    #[config_schema(default = r#""from-code""#)]
    pub optional: Option<String>,
}

#[derive(ConfigSchema, Serialize)]
pub struct StaticEmptyFields {
    #[config_schema(default = "true")]
    pub enabled: bool,
    #[config_schema(default = r#"["from-code"]"#)]
    pub names: Vec<String>,
    #[config_schema(default = r#""from-code""#)]
    pub label: String,
}

macro_rules! static_config_agent {
    ($agent:ident, $implementation:ident, $config:ty) => {
        #[agent_definition]
        pub trait $agent {
            fn new(name: String, #[agent_config] config: Config<$config>) -> Self;
            fn read(&self) -> String;
        }

        struct $implementation {
            config: Config<$config>,
        }

        #[agent_implementation]
        impl $agent for $implementation {
            fn new(_name: String, #[agent_config] config: Config<$config>) -> Self {
                Self { config }
            }

            fn read(&self) -> String {
                serde_json::to_string(&self.config.get().expect("static config is captured"))
                    .unwrap()
            }
        }
    };
}

static_config_agent!(
    StaticDeliveryAgent,
    StaticDeliveryAgentImpl,
    StaticDeliveryFields
);
static_config_agent!(
    StaticRecursiveAgent,
    StaticRecursiveAgentImpl,
    StaticRecursiveFields
);
static_config_agent!(StaticNamedAgent, StaticNamedAgentImpl, StaticNamedFields);
static_config_agent!(
    StaticRequiredAgent,
    StaticRequiredAgentImpl,
    StaticRequiredFields
);
static_config_agent!(
    StaticOptionalAgent,
    StaticOptionalAgentImpl,
    StaticOptionalFields
);
static_config_agent!(StaticEmptyAgent, StaticEmptyAgentImpl, StaticEmptyFields);

#[derive(ConfigSchema)]
pub struct NestedConfig {
    #[config_schema(secret)]
    pub nested_secret: Secret<i32>,
    pub a: bool,
    pub b: Vec<i32>,
}

#[derive(ConfigSchema, Serialize)]
pub struct AliasedNestedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<i32>,
}

#[derive(ConfigSchema)]
pub struct ConfigAgentConfig {
    pub foo: i32,
    pub bar: String,
    #[config_schema(secret)]
    pub secret: Secret<String>,
    #[config_schema(nested)]
    pub nested: NestedConfig,
    #[config_schema(nested)]
    pub aliased_nested: AliasedNestedConfig,
}

#[agent_definition]
pub trait ConfigAgent {
    fn new(name: String, #[agent_config] config: Config<ConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;
}

struct ConfigAgentImpl {
    config: Config<ConfigAgentConfig>,
}

#[agent_implementation]
impl ConfigAgent for ConfigAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<ConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get().expect("config access should be allowed");
        let result_json = json!({
            "foo": config.foo,
            "bar": config.bar,
            "secret": config.secret.get().expect("secret reveal should be allowed"),
            "nested": {
              "nestedSecret": config.nested.nested_secret.get().expect("secret reveal should be allowed"),
              "a": config.nested.a,
              "b": config.nested.b,
            },
            "aliasedNested": config.aliased_nested
        });

        serde_json::to_string(&result_json).unwrap()
    }
}

#[derive(ConfigSchema, Serialize)]
pub struct NestedLocalAgentConfig {
    pub a: bool,
    pub b: Vec<i32>,
}

#[derive(ConfigSchema)]
pub struct LocalConfigAgentConfig {
    pub foo: i32,
    pub bar: String,
    #[config_schema(nested)]
    pub nested: NestedLocalAgentConfig,
    #[config_schema(nested)]
    pub aliased_nested: AliasedNestedConfig,
}

#[agent_definition]
pub trait LocalConfigAgent {
    fn new(name: String, #[agent_config] config: Config<LocalConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;
}

struct LocalConfigAgentImpl {
    config: Config<LocalConfigAgentConfig>,
}

#[agent_implementation]
impl LocalConfigAgent for LocalConfigAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<LocalConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get().expect("config access should be allowed");
        let result_json = json!({
            "foo": config.foo,
            "bar": config.bar,
            "nested": config.nested,
            "aliasedNested": config.aliased_nested
        });

        serde_json::to_string(&result_json).unwrap()
    }
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema, Serialize)]
pub struct ComplexSecret {
    foo: String,
    bar: u32,
}

#[derive(ConfigSchema)]
pub struct SharedConfigAgentConfig {
    #[config_schema(secret)]
    pub secret: Secret<String>,
    #[config_schema(secret)]
    pub complex_secret: Secret<ComplexSecret>,
}

#[agent_definition]
pub trait SharedConfigAgent {
    fn new(name: String, #[agent_config] config: Config<SharedConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;

    fn create_replay_gate(&self) -> PromiseId;

    fn reveal_secret_then_await_replay_gate(&self, promise_id: PromiseId) -> String;
}

struct SharedConfigAgentImpl {
    config: Config<SharedConfigAgentConfig>,
}

#[agent_implementation]
impl SharedConfigAgent for SharedConfigAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<SharedConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get().expect("config access should be allowed");
        let result_json = json!({
            "secret": config.secret.get().expect("secret reveal should be allowed"),
            "complexSecret": config.complex_secret.get().expect("secret reveal should be allowed"),
        });

        serde_json::to_string(&result_json).unwrap()
    }

    fn create_replay_gate(&self) -> PromiseId {
        create_promise()
    }

    fn reveal_secret_then_await_replay_gate(&self, promise_id: PromiseId) -> String {
        let config = self.config.get().expect("config access should be allowed");
        let secret = config
            .secret
            .get()
            .expect("secret reveal should be allowed");
        blocking_await_promise(&promise_id);
        secret
    }
}

#[derive(ConfigSchema)]
pub struct LocalCasingSharedConfigAgentConfig {
    #[config_schema(secret)]
    pub secret_path: Secret<String>,
}

#[agent_definition]
pub trait LocalCasingSharedConfigAgent {
    fn new(
        name: String,
        #[agent_config] config: Config<LocalCasingSharedConfigAgentConfig>,
    ) -> Self;

    fn echo_local_config(&self) -> String;
}

struct LocalCasingSharedConfigAgentImpl {
    config: Config<LocalCasingSharedConfigAgentConfig>,
}

#[agent_implementation]
impl LocalCasingSharedConfigAgent for LocalCasingSharedConfigAgentImpl {
    fn new(
        _name: String,
        #[agent_config] config: Config<LocalCasingSharedConfigAgentConfig>,
    ) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get().expect("config access should be allowed");
        let result_json = json!({
            "secretPath": config.secret_path.get().expect("secret reveal should be allowed"),
        });

        serde_json::to_string(&result_json).unwrap()
    }
}

#[derive(ConfigSchema)]
pub struct SecretHandleAgentConfig {
    #[config_schema(secret)]
    pub secret_path: Secret<String>,
}

#[agent_definition(snapshotting = "enabled")]
pub trait SecretHandleAgent {
    fn new(name: String, #[agent_config] config: Config<SecretHandleAgentConfig>) -> Self;

    fn secret_id_result(&self) -> Result<String, String>;

    fn secret_metadata_result(&self) -> Result<String, String>;

    fn reveal_secret_result(&self) -> Result<String, String>;
}

struct SecretHandleAgentImpl {
    secret: GuestSecretHandle,
}

#[agent_implementation]
impl SecretHandleAgent for SecretHandleAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<SecretHandleAgentConfig>) -> Self {
        Self {
            secret: config
                .get()
                .expect("config access should be allowed")
                .secret_path
                .handle()
                .expect("secret handle access should be allowed"),
        }
    }

    fn secret_id_result(&self) -> Result<String, String> {
        self.secret
            .with_handle(types::id)
            .map(|id| format!("{:02x?}", id.bytes))
            .ok_or_else(|| "secret handle has already been transferred".to_string())
    }

    fn secret_metadata_result(&self) -> Result<String, String> {
        self.secret
            .with_handle(types::metadata)
            .map(|metadata| format!("{metadata:?}"))
            .ok_or_else(|| "secret handle has already been transferred".to_string())
    }

    fn reveal_secret_result(&self) -> Result<String, String> {
        let inner_graph = golem_rust::schema::try_into_schema_graph::<String>()
            .map_err(|error| error.to_string())?;
        let expected_type = encode_schema_graph(&inner_graph).map_err(|error| error.to_string())?;
        let value = self
            .secret
            .with_handle(|handle| reveal::reveal(handle, &expected_type))
            .ok_or_else(|| "secret handle has already been transferred".to_string())?
            .map_err(|error| format!("{error:?}"))?;
        let value = decode_schema_value(value).map_err(|error| error.to_string())?;
        String::from_value(&value).map_err(|error| error.to_string())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let config = Config::<SecretHandleAgentConfig>::new();
        Ok(Self {
            secret: config
                .get()
                .expect("config access should be allowed")
                .secret_path
                .handle()
                .expect("secret handle access should be allowed"),
        })
    }
}

#[derive(ConfigSchema)]
pub struct RpcLocalConfigAgentConfig {
    pub foo: i32,
    pub nested_a: Option<bool>,
}

#[agent_definition]
pub trait RpcLocalConfigAgent {
    fn new(name: String, #[agent_config] config: Config<RpcLocalConfigAgentConfig>) -> Self;

    async fn echo_local_config(&self) -> String;
}

struct RpcLocalConfigAgentImpl {
    name: String,
    config: Config<RpcLocalConfigAgentConfig>,
}

#[agent_implementation]
impl RpcLocalConfigAgent for RpcLocalConfigAgentImpl {
    fn new(name: String, #[agent_config] config: Config<RpcLocalConfigAgentConfig>) -> Self {
        Self { name, config }
    }

    async fn echo_local_config(&self) -> String {
        let config = self.config.get().expect("config access should be allowed");
        // A keyed create sets the overrides on a new agent; an existing agent keeps its
        // configuration and is reused.
        let created = LocalConfigAgentClient::create(
            self.name.clone(),
            golem_rust::CreateOptions {
                keyed: true,
                config: LocalConfigAgentConfigRpc {
                    foo: Some(config.foo.clone()),
                    nested: NestedLocalAgentConfigRpc {
                        a: config.nested_a,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                env: Vec::new(),
            },
        )
        .await;
        let client = match created {
            Ok(client) => client,
            Err(golem_rust::GolemReflectError::Conflict(_)) => {
                LocalConfigAgentClient::upsert(self.name.clone())
            }
            Err(error) => panic!("failed to create the LocalConfigAgent: {error}"),
        };
        client.echo_local_config().await
    }
}
