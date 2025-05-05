use cloud_common::config::RemoteCloudServiceConfig;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_service_base::config::MergedConfigLoaderOrDumper;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Default)]
pub struct WorkerServiceCloudConfig {
    pub base_config: WorkerServiceBaseConfig,
    pub cloud_specific_config: CloudSpecificWorkerServiceConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudSpecificWorkerServiceConfig {
    pub workspace: String,
    pub domain_records: DomainRecordsConfig,
    pub cloud_service: RemoteCloudServiceConfig,
    pub cors_origin_regex: String,
}

impl Default for CloudSpecificWorkerServiceConfig {
    fn default() -> Self {
        Self {
            workspace: "release".to_string(),
            domain_records: DomainRecordsConfig::default(),
            cloud_service: RemoteCloudServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
        }
    }
}

impl HasConfigExamples<CloudSpecificWorkerServiceConfig> for CloudSpecificWorkerServiceConfig {
    fn examples() -> Vec<ConfigExample<CloudSpecificWorkerServiceConfig>> {
        vec![]
    }
}

impl WorkerServiceCloudConfig {
    pub fn is_local_env(&self) -> bool {
        self.base_config.environment.to_lowercase() == "local"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainRecordsConfig {
    pub subdomain_black_list: Vec<String>,
    pub domain_allow_list: Vec<String>,
    pub register_domain_black_list: Vec<String>,
}

impl Default for DomainRecordsConfig {
    fn default() -> Self {
        Self {
            subdomain_black_list: vec![
                "api-gateway".to_string(),
                "release".to_string(),
                "grafana".to_string(),
            ],
            domain_allow_list: vec![],
            register_domain_black_list: vec![
                "dev-api.golem.cloud".to_string(),
                "api.golem.cloud".to_string(),
            ],
        }
    }
}

impl DomainRecordsConfig {
    pub fn is_domain_available_for_registration(&self, domain_name: &str) -> bool {
        let dn = domain_name.to_lowercase();
        !self
            .register_domain_black_list
            .iter()
            .any(|d| domain_match(&dn, d))
    }

    pub fn is_domain_available(&self, domain_name: &str) -> bool {
        let dn = domain_name.to_lowercase();

        let in_register_black_list = self
            .register_domain_black_list
            .iter()
            .any(|d| domain_match(&dn, d));

        if in_register_black_list {
            let in_allow_list = self.domain_allow_list.iter().any(|d| domain_match(&dn, d));

            if !in_allow_list {
                return false;
            }
        }

        true
    }

    pub fn is_site_available(&self, api_site: &str, hosted_zone: &str) -> bool {
        let hz = if hosted_zone.ends_with('.') {
            &hosted_zone[0..hosted_zone.len() - 1]
        } else {
            hosted_zone
        };

        let s = api_site.to_lowercase();

        !self.subdomain_black_list.iter().any(|p| {
            let d = format!("{}.{}", p, hz).to_lowercase();
            d == s
        })
    }
}

fn domain_match(domain: &str, domain_cfg: &str) -> bool {
    if domain.ends_with(domain_cfg) {
        let prefix = &domain[0..domain.len() - domain_cfg.len()];
        prefix.is_empty() || prefix.ends_with('.')
    } else {
        false
    }
}

const CONFIG_FILE_NAME: &str = "config/worker-service.toml";

pub fn make_worker_service_base_config_loader() -> ConfigLoader<WorkerServiceBaseConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from(CONFIG_FILE_NAME))
}

pub fn make_cloud_specific_config_loader() -> ConfigLoader<CloudSpecificWorkerServiceConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from(CONFIG_FILE_NAME))
}

pub fn load_or_dump_config() -> Option<WorkerServiceCloudConfig> {
    MergedConfigLoaderOrDumper::new(
        "worker_service_base_config",
        make_worker_service_base_config_loader(),
    )
    .add(
        "cloud_specific_worker_service_config",
        make_cloud_specific_config_loader(),
        |base, specific| WorkerServiceCloudConfig {
            base_config: base,
            cloud_specific_config: specific,
        },
    )
    .finish()
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::{domain_match, load_or_dump_config, DomainRecordsConfig};

    #[test]
    pub fn config_is_loadable() {
        load_or_dump_config().expect("Failed to load config");
    }

    #[test]
    pub fn test_is_domain_match() {
        assert!(domain_match("dev-api.golem.cloud", "dev-api.golem.cloud"));
        assert!(domain_match("api.golem.cloud", "api.golem.cloud"));
        assert!(domain_match("dev.api.golem.cloud", "api.golem.cloud"));
        assert!(!domain_match("dev.api.golem.cloud", "dev-api.golem.cloud"));
        assert!(!domain_match("dev-api.golem.cloud", "api.golem.cloud"));
    }

    #[test]
    pub fn test_is_domain_available_for_registration() {
        let config = DomainRecordsConfig::default();
        assert!(!config.is_domain_available_for_registration("dev-api.golem.cloud"));
        assert!(!config.is_domain_available_for_registration("api.golem.cloud"));
        assert!(!config.is_domain_available_for_registration("my.dev-api.golem.cloud"));
        assert!(config.is_domain_available_for_registration("test.cloud"));
    }

    #[test]
    pub fn test_is_domain_available() {
        let config = DomainRecordsConfig {
            domain_allow_list: vec!["dev-api.golem.cloud".to_string()],
            ..Default::default()
        };

        assert!(config.is_domain_available("dev-api.golem.cloud"));
        assert!(config.is_domain_available("test.cloud"));
        assert!(!config.is_domain_available("api.golem.cloud"));
    }

    #[test]
    pub fn test_is_site_available() {
        let config = DomainRecordsConfig::default();

        let hosted_zone = "dev-api.golem.cloud.";

        assert!(!config.is_site_available("api-gateway.dev-api.golem.cloud", hosted_zone));
        assert!(!config.is_site_available("RELEASE.dev-api.golem.cloud", hosted_zone));
        assert!(!config.is_site_available("Grafana.dev-api.golem.cloud", hosted_zone));
        assert!(config.is_site_available("foo.dev-api.golem.cloud", hosted_zone));
    }
}
