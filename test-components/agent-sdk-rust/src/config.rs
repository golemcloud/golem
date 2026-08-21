use golem_rust::agentic::{Config, Secret};
use golem_rust::bindings::golem::secrets::{reveal, types};
use golem_rust::secrets::GuestSecretHandle;
use golem_rust::{
    ConfigSchema, FromSchema, IntoSchema, agent_definition, agent_implementation,
    decode_schema_value, encode_schema_graph,
};
use golem_rust::{PromiseId, blocking_await_promise, create_promise};
use serde::Serialize;
use serde_json::json;

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

#[derive(IntoSchema, FromSchema, Serialize)]
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
        let secret = config.secret.get().expect("secret reveal should be allowed");
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

    async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
        Ok(())
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
        let client = LocalConfigAgentClient::get_with_config(
            self.name.clone(),
            LocalConfigAgentConfigRpc {
                foo: Some(config.foo.clone()),
                nested: NestedLocalAgentConfigRpc {
                    a: config.nested_a,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        client.echo_local_config().await
    }
}
