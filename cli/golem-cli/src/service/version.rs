// Copyright 2024-2025 Golem Cloud
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

use crate::clients::health_check::HealthCheckClient;
use crate::model::GolemError;
use async_trait::async_trait;
use std::cmp::Ordering;
use std::sync::Arc;
use tokio::task::JoinSet;
use version_compare::Version;

pub enum VersionCheckResult {
    Ok,
    NewerServerVersionAvailable {
        cli_version: String,
        server_version: String,
    },
}

#[async_trait]
pub trait VersionService {
    async fn check(&self, cli_version: &str) -> Result<VersionCheckResult, GolemError>;
}

pub struct VersionServiceLive {
    pub clients: Vec<Arc<dyn HealthCheckClient + Send + Sync>>,
}

#[async_trait]
impl VersionService for VersionServiceLive {
    async fn check(&self, cli_version: &str) -> Result<VersionCheckResult, GolemError> {
        let mut requests = JoinSet::new();
        for client in self.clients.clone() {
            requests.spawn(async move { client.version().await });
        }

        let mut versions = Vec::with_capacity(self.clients.len());
        while let Some(result) = requests.join_next().await {
            versions.push(result.expect("Failed to join version request")?);
        }

        let server_versions = {
            let mut server_versions: Vec<_> = vec![];
            for version in &versions {
                match Version::from(version.version.as_str()) {
                    Some(version) => {
                        server_versions.push(version);
                    }
                    None => {
                        return Err(GolemError(format!(
                            "Failed to parse server version: {}",
                            version.version
                        )))
                    }
                }
            }
            server_versions
        };

        let cli_version = {
            match Version::from(cli_version) {
                Some(version) => version,
                None => {
                    return Err(GolemError(format!(
                        "Failed to parse cli version: {}",
                        cli_version
                    )))
                }
            }
        };

        let newer_server_version = server_versions
            .iter()
            .filter(|&v| v > &cli_version)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

        Ok(newer_server_version
            .map(
                |server_version| VersionCheckResult::NewerServerVersionAvailable {
                    cli_version: cli_version.as_str().to_string(),
                    server_version: server_version.as_str().to_string(),
                },
            )
            .unwrap_or_else(|| VersionCheckResult::Ok))
    }
}
