use golem_rust::{ConfigSchema, agent_definition, agent_implementation};
use golem_rust::agentic::{Config, Secret};
use serde_json::json;
use serde::Serialize;

#[derive(ConfigSchema)]
pub struct NestedConfig {
    pub nested_secret: Secret<i32>,
    pub a: bool,
    pub b: Vec<i32>,
}

#[derive(ConfigSchema, Serialize)]
pub struct AliasedNestedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<i32>
}

#[derive(ConfigSchema)]
pub struct ConfigAgentConfig {
  pub foo: i32,
  pub bar: String,
  pub secret: Secret<String>,
  pub nested: NestedConfig,
  pub aliased_nested: AliasedNestedConfig,
}

#[agent_definition]
pub trait ConfigAgent {
    fn new(name: String, #[autoinject] config: Config<ConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;
}

struct ConfigAgentImpl {
    config: Config<ConfigAgentConfig>
}

#[agent_implementation]
impl ConfigAgent for ConfigAgentImpl {
    fn new(_name: String, #[autoinject] config: Config<ConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get();
        let result_json = json!({
            "foo": config.foo,
            "bar": config.bar,
            "secret": config.secret.get(),
            "nested": {
              "nestedSecret": config.nested.nested_secret.get(),
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
  pub nested: NestedLocalAgentConfig,
  pub aliased_nested: AliasedNestedConfig,
}

#[agent_definition]
pub trait LocalConfigAgent {
    fn new(name: String, #[autoinject] config: Config<LocalConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;
}

struct LocalConfigAgentImpl {
    config: Config<LocalConfigAgentConfig>
}

#[agent_implementation]
impl LocalConfigAgent for LocalConfigAgentImpl {
    fn new(_name: String, #[autoinject] config: Config<LocalConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get();
        let result_json = json!({
            "foo": config.foo,
            "bar": config.bar,
            "nested": config.nested,
            "aliasedNested": config.aliased_nested
        });

        serde_json::to_string(&result_json).unwrap()
    }
}


#[derive(ConfigSchema)]
pub struct SharedConfigAgentConfig {
    pub secret: Secret<String>,
}

#[agent_definition]
pub trait SharedConfigAgent {
    fn new(name: String, #[autoinject] config: Config<SharedConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;
}

struct SharedConfigAgentImpl {
    config: Config<SharedConfigAgentConfig>
}

#[agent_implementation]
impl SharedConfigAgent for SharedConfigAgentImpl {
    fn new(_name: String, #[autoinject] config: Config<SharedConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get();
        let result_json = json!({
            "secret": config.secret.get(),
        });

        serde_json::to_string(&result_json).unwrap()
    }
}

#[derive(ConfigSchema)]
pub struct LocalCasingSharedConfigAgentConfig {
    pub secret_path: Secret<String>,
}

#[agent_definition]
pub trait LocalCasingSharedConfigAgent {
    fn new(name: String, #[autoinject] config: Config<LocalCasingSharedConfigAgentConfig>) -> Self;

    fn echo_local_config(&self) -> String;
}

struct LocalCasingSharedConfigAgentImpl {
    config: Config<LocalCasingSharedConfigAgentConfig>
}

#[agent_implementation]
impl LocalCasingSharedConfigAgent for LocalCasingSharedConfigAgentImpl {
    fn new(_name: String, #[autoinject] config: Config<LocalCasingSharedConfigAgentConfig>) -> Self {
        Self { config }
    }

    fn echo_local_config(&self) -> String {
        let config = self.config.get();
        let result_json = json!({
            "secretPath": config.secret_path.get(),
        });

        serde_json::to_string(&result_json).unwrap()
    }
}
