use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::RetryConfig;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use http::Uri;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub struct CloudServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl CloudServiceConfig {
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

impl Default for CloudServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            access_token: Uuid::new_v4(),
            retries: RetryConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct WorkerServiceCloudConfig {
    pub base_config: WorkerServiceBaseConfig,
    pub cloud_specific_config: CloudSpecificWorkerServiceConfig,
    pub cloud_service: CloudServiceConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CloudSpecificWorkerServiceConfig {
    pub workspace: String,
    pub domain_records: DomainRecordsConfig,
}

impl Default for CloudSpecificWorkerServiceConfig {
    fn default() -> Self {
        Self {
            workspace: "release".to_string(),
            domain_records: DomainRecordsConfig::default(),
        }
    }
}

impl WorkerServiceCloudConfig {
    pub fn is_local_env(&self) -> bool {
        self.base_config.environment.to_lowercase() == "local"
    }

    pub fn load() -> Self {
        let common_file = "config/worker-service.toml";
        let env_prefix = Env::prefixed("GOLEM__").split("__");

        let base_config: WorkerServiceBaseConfig = Figment::new()
            .merge(Toml::file(common_file))
            .merge(env_prefix.clone())
            .extract()
            .expect("Failed to parse base worker service config");

        let cloud_specific_config: CloudSpecificWorkerServiceConfig = Figment::new()
            .merge(Toml::file(common_file))
            .merge(env_prefix.clone())
            .extract()
            .expect("Failed to parse cloud specific worker service config");

        let cloud_service: CloudServiceConfig = Figment::new()
            .merge(Toml::file(common_file))
            .merge(env_prefix.clone())
            .extract_inner("cloud_service")
            .expect("Failed to parse cloud service config");

        WorkerServiceCloudConfig {
            base_config,
            cloud_specific_config,
            cloud_service,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
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

#[cfg(test)]
mod tests {
    use crate::config::{domain_match, DomainRecordsConfig};

    #[test]
    pub fn config_is_loadable() {
        // The following settings are always coming through environment variables:
        std::env::set_var("GOLEM__ENVIRONMENT", "dev");
        std::env::set_var("GOLEM__WORKSPACE", "release");
        std::env::set_var("GOLEM__DB__TYPE", "Postgres");
        std::env::set_var("GOLEM__DB__CONFIG__USERNAME", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__PASSWORD", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__SCHEMA", "test");
        std::env::set_var("GOLEM__CLOUD_SERVICE__HOST", "localhost");
        std::env::set_var("GOLEM__CLOUD_SERVICE__PORT", "7899");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__HOST", "localhost");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__PORT", "1234");
        std::env::set_var("GOLEM__ROUTING_TABLE__HOST", "localhost");
        std::env::set_var("GOLEM__ROUTING_TABLE__PORT", "1234");
        std::env::set_var(
            "GOLEM__CLOUD_SERVICE__ACCESS_TOKEN",
            "5C832D93-FF85-4A8F-9803-513950FDFDB1",
        );
        std::env::set_var(
            "GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN",
            "5C832D93-FF85-4A8F-9803-513950FDFDB1",
        );
        std::env::set_var(
            "GOLEM__DOMAIN_RECORDS__REGISTER_DOMAIN_BLACK_LIST",
            "[api.golem.cloud,local-api.golem.cloud]",
        );
        std::env::set_var(
            "GOLEM__DOMAIN_RECORDS__DOMAIN_ALLOW_LIST",
            "[api.golem.cloud]",
        );
        std::env::set_var("GOLEM__DOMAIN_RECORDS__SUBDOMAIN_BLACK_LIST", "[api]");

        // The rest can be loaded from the toml
        let config = super::WorkerServiceCloudConfig::load();

        println!("config: {:?}", config);
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
