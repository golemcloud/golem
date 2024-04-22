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

use async_trait::async_trait;
use version_compare::Version;

use crate::clients::health_check::HealthCheckClient;
use crate::model::{GolemError, GolemResult};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[async_trait]
pub trait VersionHandler {
    async fn check(&self) -> Result<GolemResult, GolemError>;
}

pub struct VersionHandlerLive<
    T: HealthCheckClient + Send + Sync,
    W: HealthCheckClient + Send + Sync,
> {
    pub component_client: T,
    pub worker_client: W,
}

#[async_trait]
impl<T: HealthCheckClient + Send + Sync, W: HealthCheckClient + Send + Sync> VersionHandler
    for VersionHandlerLive<T, W>
{
    async fn check(&self) -> Result<GolemResult, GolemError> {
        let component_version_info = self.component_client.version().await?;
        let worker_version_info = self.worker_client.version().await?;

        let cli_version = Version::from(VERSION).unwrap();
        let component_version = Version::from(component_version_info.version.as_str()).unwrap();
        let worker_version = Version::from(worker_version_info.version.as_str()).unwrap();

        let warning = |cli_version: Version, server_version: Version| -> String {
            format!("Warning: golem-cli {} is older than the targeted Golem servers ({})\nInstall the matching version with:\ncargo install golem-cli@{}\n", cli_version.as_str(), server_version.as_str(), server_version.as_str()).to_string()
        };

        if cli_version < component_version && cli_version < worker_version {
            if component_version > worker_version {
                Err(GolemError(warning(cli_version, component_version)))
            } else {
                Err(GolemError(warning(cli_version, worker_version)))
            }
        } else if cli_version < component_version {
            Err(GolemError(warning(cli_version, component_version)))
        } else if cli_version < worker_version {
            Err(GolemError(warning(cli_version, worker_version)))
        } else {
            Ok(GolemResult::Str("No updates found".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::model::GolemResult;
    use crate::{
        clients::health_check::HealthCheckClient,
        model::GolemError,
        version::{VersionHandler, VersionHandlerLive},
    };
    use async_trait::async_trait;
    use golem_client::model::VersionInfo;

    pub struct HealthCheckClientTest {
        version: &'static str,
    }

    #[async_trait]
    impl HealthCheckClient for HealthCheckClientTest {
        async fn version(&self) -> Result<VersionInfo, GolemError> {
            Ok(VersionInfo {
                version: self.version.to_string(),
            })
        }
    }

    fn client(v: &'static str) -> HealthCheckClientTest {
        HealthCheckClientTest { version: v }
    }

    fn warning(server_version: &str) -> String {
        format!("Warning: golem-cli 0.0.0 is older than the targeted Golem servers ({})\nInstall the matching version with:\ncargo install golem-cli@{}\n", server_version, server_version).to_string()
    }

    async fn check_version(
        component_version: &'static str,
        worker_version: &'static str,
    ) -> String {
        let update_srv = VersionHandlerLive {
            component_client: client(component_version),
            worker_client: client(worker_version),
        };

        let checked = update_srv.check().await;
        match checked {
            Ok(GolemResult::Str(s)) => s,
            Err(e) => e.to_string(),
            _ => "error".to_string(),
        }
    }

    #[tokio::test]
    pub async fn same_version() {
        let result = check_version("0.0.0", "0.0.0").await;
        let expected = "No updates found".to_string();
        assert_eq!(result, expected)
    }

    #[tokio::test]
    pub async fn both_older() {
        let result = check_version("0.0.0-snapshot", "0.0.0-snapshot").await;
        let expected = "No updates found".to_string();
        assert_eq!(result, expected)
    }

    #[tokio::test]
    pub async fn both_newer_component_newest() {
        let result = check_version("0.1.0", "0.0.3").await;
        let expected = warning("0.1.0");
        assert_eq!(result, expected)
    }

    #[tokio::test]
    pub async fn both_newer_worker_newest() {
        let result = check_version("0.1.1", "0.2.0").await;
        let expected = warning("0.2.0");
        assert_eq!(result, expected)
    }

    #[tokio::test]
    pub async fn newer_worker() {
        let result = check_version("0.0.0", "0.0.1").await;
        let expected = warning("0.0.1");
        assert_eq!(result, expected)
    }

    #[tokio::test]
    pub async fn newer_component() {
        let result = check_version("0.0.1", "0.0.0").await;
        let expected = warning("0.0.1");
        assert_eq!(result, expected)
    }
}
