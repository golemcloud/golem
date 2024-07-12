use golem_common::config::{ConfigLoader, ConfigLoaderConfig};

pub struct MergedConfigLoader<T> {
    config_file_name: String,
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
                "config_file_name mismatch while loading for '{}' config: {} <-> {}",
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
    config_file_name: String,
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
                "config_file_name mismatch while loading (or dumping) for '{}' config: {} <-> {}",
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
