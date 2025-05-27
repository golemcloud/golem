use golem_common::config::{ConfigExample, HasConfigExamples};
use golem_common::model::RetryConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

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
