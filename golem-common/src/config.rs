// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bincode::{Decode, Encode};
use figment::providers::{Env, Format, Serialized, Toml};
use figment::value::Value;
use figment::Figment;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const ENV_VAR_PREFIX: &str = "GOLEM__";
const ENV_VAR_NESTED_SEPARATOR: &str = "__";

pub trait ConfigLoaderConfig: Default + Serialize + Deserialize<'static> {}
impl<T: Default + Serialize + Deserialize<'static>> ConfigLoaderConfig for T {}

pub struct ConfigLoader<T: ConfigLoaderConfig> {
    pub config_file_name: String,
    config_type: std::marker::PhantomData<T>,
}

impl<T: ConfigLoaderConfig> ConfigLoader<T> {
    pub fn new(config_file_name: String) -> ConfigLoader<T> {
        Self {
            config_file_name,
            config_type: std::marker::PhantomData,
        }
    }

    pub fn default_figment(&self) -> Figment {
        Figment::new().merge(Serialized::defaults(T::default()))
    }

    pub fn figment(&self) -> Figment {
        Figment::new()
            .merge(Serialized::defaults(T::default()))
            .merge(Toml::file(self.config_file_name.clone()))
            .merge(Env::prefixed(ENV_VAR_PREFIX).split(ENV_VAR_NESTED_SEPARATOR))
    }

    pub fn load(&self) -> figment::Result<T> {
        self.figment().extract()
    }

    pub fn load_or_dump_config(&self) -> Option<T> {
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map_or("", |a| a.as_str()) {
            "--dump-default-config" => Self::dump(&self.default_figment(), dump::print(false)),
            "--dump-default-config-env-var" => Self::dump(&self.default_figment(), dump::env_var()),
            "--dump-default-config-toml" => Self::dump(&self.default_figment(), dump::toml()),
            "--dump-config" => Self::dump(&self.figment(), dump::print(true)),
            "--dump-config-env-var" => Self::dump(&self.figment(), dump::env_var()),
            "--dump-config-toml" => Self::dump(&self.figment(), dump::toml()),
            _ => Some(self.load().expect("Failed to load config")),
        }
    }

    fn dump<U>(figment: &Figment, dump: dump::Dump) -> Option<U> {
        let config: Value = figment
            .extract()
            .expect("Failed to extract config for dump");

        match dump {
            dump::Dump::RootValue(dump) => dump.root_value(&config),
            dump::Dump::Visitor(dump) => {
                Self::dump_value(figment, &*dump, &mut Vec::new(), "", &config)
            }
        }

        None
    }

    fn dump_value<'a>(
        figment: &Figment,
        dump: &dyn dump::VisitorDump,
        path: &mut Vec<&'a str>,
        name: &'a str,
        value: &'a Value,
    ) {
        match value {
            Value::Dict(_, dict) => {
                if !name.is_empty() {
                    path.push(name);
                }

                for (name, value) in dict {
                    if value.as_dict().is_none() {
                        Self::dump_value(figment, dump, path, name, value)
                    }
                }
                for (name, value) in dict {
                    if value.as_dict().is_some() {
                        Self::dump_value(figment, dump, path, name, value)
                    }
                }

                if !name.is_empty() {
                    path.pop();
                }
            }
            Value::Array(_, _) => {
                panic!("Arrays are not supported in config dump");
            }
            value => dump.value(path, name, figment.get_metadata(value.tag()), value),
        }
    }
}

pub(crate) mod dump {
    use crate::config::{ENV_VAR_NESTED_SEPARATOR, ENV_VAR_PREFIX};
    use figment::value::Value;
    use figment::Metadata;

    pub fn print(show_source: bool) -> Dump {
        Dump::Visitor(Box::new(Print { show_source }))
    }

    pub fn env_var() -> Dump {
        Dump::Visitor(Box::new(EnvVar))
    }

    pub fn toml() -> Dump {
        Dump::RootValue(Box::new(Toml))
    }

    pub enum Dump {
        RootValue(Box<dyn RootValueDump>),
        Visitor(Box<dyn VisitorDump>),
    }

    pub trait RootValueDump {
        fn root_value(&self, value: &Value);
    }

    pub trait VisitorDump {
        fn value(&self, path: &[&str], name: &str, metadata: Option<&Metadata>, value: &Value);
    }

    struct Print {
        pub show_source: bool,
    }

    impl VisitorDump for Print {
        fn value(&self, path: &[&str], name: &str, metadata: Option<&Metadata>, value: &Value) {
            if !path.is_empty() {
                for elem in path {
                    print!("{}.", elem);
                }
            }
            if !name.is_empty() {
                print!("{}", name);
            }

            print!(
                ": {}",
                serde_json::to_string(value).expect("Failed to pretty print value")
            );

            if self.show_source {
                let name = metadata.map_or("unknown", |m| &m.name);
                let source = metadata
                    .and_then(|m| m.source.as_ref())
                    .map_or("unknown".to_owned(), |s| s.to_string());
                println!(" ({} - {})", name, source);
            } else {
                println!()
            }
        }
    }

    struct EnvVar;

    impl VisitorDump for EnvVar {
        fn value(&self, path: &[&str], name: &str, _metadata: Option<&Metadata>, value: &Value) {
            let is_empty_value = value.to_empty().is_some();

            if is_empty_value {
                print!("#");
            }

            print!("{}", ENV_VAR_PREFIX);

            if !path.is_empty() {
                for elem in path {
                    print!("{}{}", elem.to_uppercase(), ENV_VAR_NESTED_SEPARATOR);
                }
            }
            if !name.is_empty() {
                print!("{}", name.to_uppercase());
            }

            if !is_empty_value {
                println!(
                    "={}",
                    serde_json::to_string(value).expect("Failed to pretty print value")
                );
            } else {
                println!("=");
            }
        }
    }

    pub struct Toml;

    impl RootValueDump for Toml {
        fn root_value(&self, value: &Value) {
            println!(
                "{}",
                toml::to_string(value).expect("Failed to serialize as TOML")
            );
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub database: usize,
    pub tracing: bool,
    pub pool_size: usize,
    pub retries: RetryConfig,
    pub key_prefix: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RedisConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!(
            "redis://{}:{}/{}",
            self.host, self.port, self.database
        ))
        .expect("Failed to parse Redis URL")
    }
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 6379,
            database: 0,
            tracing: false,
            pool_size: 8,
            retries: RetryConfig::default(),
            key_prefix: "".to_string(),
            username: None,
            password: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct RetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2,
        }
    }
}
