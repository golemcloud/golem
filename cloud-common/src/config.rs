use golem_common::config::{
    ConfigExample, ConfigLoader, ConfigLoaderConfig, HasConfigExamples, RetryConfig,
};
use http::Uri;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;
use uuid::Uuid;

pub struct MergedConfigLoader<T> {
    config_file_name: PathBuf,
    config: figment::Result<T>,
}

impl<T: ConfigLoaderConfig> MergedConfigLoader<T> {
    pub fn new(name: &str, config_loader: ConfigLoader<T>) -> MergedConfigLoader<T> {
        MergedConfigLoader {
            config_file_name: config_loader.config_file_name.clone(),
            config: Ok(()),
        }
        .add(name, config_loader, |_, config| config)
    }

    pub fn add<U: ConfigLoaderConfig, V>(
        self,
        name: &str,
        config_loader: ConfigLoader<U>,
        merge: fn(T, U) -> V,
    ) -> MergedConfigLoader<V> {
        if self.config_file_name != config_loader.config_file_name {
            panic!(
                "config_file_name mismatch while loading for '{}' config: {:?} <-> {:?}",
                name, self.config_file_name, config_loader.config_file_name,
            );
        }

        let config = match self.config {
            Ok(base_config) => match config_loader.load() {
                Ok(config) => Ok(merge(base_config, config)),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };

        MergedConfigLoader {
            config_file_name: self.config_file_name,
            config,
        }
    }
}

impl<T> MergedConfigLoader<T> {
    pub fn finish(self) -> figment::Result<T> {
        self.config
    }
}

pub struct MergedConfigLoaderOrDumper<T> {
    config_file_name: PathBuf,
    config: Option<T>,
    dummy: bool,
}

impl<T: ConfigLoaderConfig> MergedConfigLoaderOrDumper<T> {
    pub fn new(name: &str, config_loader: ConfigLoader<T>) -> MergedConfigLoaderOrDumper<T> {
        MergedConfigLoaderOrDumper {
            config_file_name: config_loader.config_file_name.clone(),
            config: Some(()),
            dummy: true,
        }
        .add(name, config_loader, |_, config| config)
    }

    pub fn add<U: ConfigLoaderConfig, V>(
        self,
        name: &str,
        config_loader: ConfigLoader<U>,
        merge: fn(T, U) -> V,
    ) -> MergedConfigLoaderOrDumper<V> {
        if self.config_file_name != config_loader.config_file_name {
            panic!(
                "config_file_name mismatch while loading (or dumping) for '{}' config: {:?} <-> {:?}",
                name, self.config_file_name, config_loader.config_file_name,
            );
        }

        let config = match self.config {
            Some(base_config) => match config_loader.load_or_dump_config() {
                Some(config) => Some(merge(base_config, config)),
                None if self.dummy => None,
                None => {
                    panic!("illegal state while dumping, got no config for '{}'", name,);
                }
            },
            None => {
                match config_loader.load_or_dump_config() {
                    Some(_) => {
                        panic!("illegal state while loading, got config for '{}', while expected dumping", name);
                    }
                    None => None,
                }
            }
        };

        MergedConfigLoaderOrDumper {
            config_file_name: self.config_file_name,
            config,
            dummy: false,
        }
    }
}

impl<T> MergedConfigLoaderOrDumper<T> {
    pub fn finish(self) -> Option<T> {
        self.config
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteCloudServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl RemoteCloudServiceConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse CloudService URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build CloudService URI")
    }
}

impl Default for RemoteCloudServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            access_token: Uuid::parse_str("5c832d93-ff85-4a8f-9803-513950fdfdb1")
                .expect("invalid UUID"),
            retries: RetryConfig::default(),
        }
    }
}

impl HasConfigExamples<RemoteCloudServiceConfig> for RemoteCloudServiceConfig {
    fn examples() -> Vec<ConfigExample<RemoteCloudServiceConfig>> {
        vec![]
    }
}
