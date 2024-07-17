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

pub type ConfigExample<T> = (&'static str, T);

pub trait HasConfigExamples<T> {
    fn examples() -> Vec<ConfigExample<T>>;
}

pub struct ConfigLoader<T: ConfigLoaderConfig> {
    pub config_file_name: String,
    pub make_examples: Option<fn() -> Vec<ConfigExample<T>>>,
    config_type: std::marker::PhantomData<T>,
}

impl<T: ConfigLoaderConfig> ConfigLoader<T> {
    pub fn new(config_file_name: String) -> ConfigLoader<T> {
        Self {
            config_file_name,
            make_examples: None,
            config_type: std::marker::PhantomData,
        }
    }

    pub fn new_with_examples(config_file_name: String) -> ConfigLoader<T>
    where
        T: HasConfigExamples<T>,
    {
        Self {
            config_file_name,
            make_examples: Some(T::examples),
            config_type: std::marker::PhantomData,
        }
    }

    pub fn default_figment(&self) -> Figment {
        Figment::new().merge(Serialized::defaults(T::default()))
    }

    pub fn example_figments(&self) -> Vec<(&'static str, Figment)> {
        self.make_examples
            .map_or(Vec::new(), |make| make())
            .into_iter()
            .map(|(name, example)| (name, Figment::new().merge(Serialized::defaults(example))))
            .collect()
    }

    pub fn figment(&self) -> Figment {
        Figment::new()
            .merge(Serialized::defaults(T::default()))
            .merge(Toml::file(self.config_file_name.clone()))
            .merge(env_config_provider())
    }

    pub fn load(&self) -> figment::Result<T> {
        self.figment().extract()
    }

    fn default_dump_source(&self) -> dump::Source {
        dump::Source::Default {
            default: self.default_figment(),
            examples: self.example_figments(),
        }
    }

    fn loaded_dump_source(&self) -> dump::Source {
        dump::Source::Loaded(self.figment())
    }

    pub fn load_or_dump_config(&self) -> Option<T> {
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map_or("", |a| a.as_str()) {
            "--dump-config-default" => self.dump(self.default_dump_source(), &dump::print(false)),
            "--dump-config-default-env-var" => {
                self.dump(self.default_dump_source(), &dump::env_var())
            }
            "--dump-config-default-toml" => self.dump(self.default_dump_source(), &dump::toml()),
            "--dump-config" => self.dump(self.loaded_dump_source(), &dump::print(true)),
            "--dump-config-env-var" => self.dump(self.loaded_dump_source(), &dump::env_var()),
            "--dump-config-toml" => self.dump(self.loaded_dump_source(), &dump::toml()),
            other => {
                if other.starts_with("--dump-config") {
                    panic!("Unknown dump config parameter: {}", other);
                } else {
                    Some(self.load().expect("Failed to load config"))
                }
            }
        }
    }

    fn dump<U>(&self, source: dump::Source, dump: &dump::Dump) -> Option<U> {
        let extract =
            |figment: &Figment| -> Value { figment.extract().expect("Failed to extract config") };

        let dump_figment = |header: &str, is_example: bool, figment: &Figment| match dump {
            dump::Dump::RootValue(dump) => dump.root_value(header, is_example, &extract(figment)),
            dump::Dump::Visitor(dump) => {
                dump.begin(header, is_example);
                Self::visit_dump(
                    figment,
                    dump.as_ref(),
                    &mut Vec::new(),
                    "",
                    &extract(figment),
                )
            }
        };

        match source {
            dump::Source::Default { default, examples } => {
                dump_figment("Generated from default config", false, &default);
                for (name, example) in examples {
                    dump_figment(
                        &format!("Generated from example config: {}", name),
                        true,
                        &example,
                    );
                }
            }
            dump::Source::Loaded(loaded) => {
                dump_figment("Generated from loaded config", false, &loaded);
            }
        }

        None
    }

    fn visit_dump<'a>(
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

                for (name, value) in dict.iter().filter(|(_, v)| v.as_dict().is_none()) {
                    Self::visit_dump(figment, dump, path, name, value)
                }
                for (name, value) in dict.iter().filter(|(_, v)| v.as_dict().is_some()) {
                    Self::visit_dump(figment, dump, path, name, value)
                }

                if !name.is_empty() {
                    path.pop();
                }
            }
            value => dump.value(path, name, figment.get_metadata(value.tag()), value),
        }
    }
}

pub(crate) mod dump {
    use crate::config::{ENV_VAR_NESTED_SEPARATOR, ENV_VAR_PREFIX};
    use figment::value::Value;
    use figment::{Figment, Metadata};

    pub enum Source {
        Default {
            default: Figment,
            examples: Vec<(&'static str, Figment)>,
        },
        Loaded(Figment),
    }

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
        fn root_value(&self, header: &str, is_example: bool, value: &Value);
    }

    pub trait VisitorDump {
        fn begin(&self, header: &str, is_example: bool);
        fn value(&self, path: &[&str], name: &str, metadata: Option<&Metadata>, value: &Value);
    }

    struct Print {
        pub show_source: bool,
    }

    impl VisitorDump for Print {
        fn begin(&self, header: &str, is_example: bool) {
            if is_example {
                println!();
            }
            println!(":: {}\n", header)
        }

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
        fn begin(&self, header: &str, is_example: bool) {
            if is_example {
                println!();
            }
            println!("### {}\n", header)
        }

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
        fn root_value(&self, header: &str, is_example: bool, value: &Value) {
            if is_example {
                println!();
            }
            println!("## {}", header);

            let mut config_as_toml_str =
                toml::to_string(value).expect("Failed to serialize as TOML");

            if is_example {
                config_as_toml_str = config_as_toml_str
                    .lines()
                    .map(|l| format!("# {}", l))
                    .collect::<Vec<String>>()
                    .join("\n")
            }

            println!("{}", config_as_toml_str);
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
            port: 6380,
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
        Self::max_attempts_5()
    }
}

impl RetryConfig {
    pub fn max_attempts_3() -> RetryConfig {
        Self {
            max_attempts: 3,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(1),
            multiplier: 3,
        }
    }

    pub fn max_attempts_5() -> RetryConfig {
        Self {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2,
        }
    }
}

pub fn env_config_provider() -> Env {
    Env::prefixed(ENV_VAR_PREFIX).split(ENV_VAR_NESTED_SEPARATOR)
}
