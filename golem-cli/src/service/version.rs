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

use crate::clients::health_check::HealthCheckClient;
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;
use std::cmp::Ordering;
use version_compare::Version;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[async_trait]
pub trait VersionService {
    async fn check(&self) -> Result<GolemResult, GolemError>;
}

pub struct VersionServiceLive {
    pub clients: Vec<Box<dyn HealthCheckClient + Send + Sync>>,
}

#[async_trait]
impl VersionService for VersionServiceLive {
    async fn check(&self) -> Result<GolemResult, GolemError> {
        let mut versions = Vec::with_capacity(self.clients.len());
        for client in &self.clients {
            versions.push(client.version().await?)
        }

        let srv_versions = versions
            .iter()
            .map(|v| Version::from(v.version.as_str()).unwrap())
            .collect::<Vec<_>>();

        let cli_version = Version::from(VERSION).unwrap();

        let warning = |cli_version: Version, server_version: &Version| -> String {
            format!("Warning: golem-cli {} is older than the targeted Golem servers ({})\nInstall the matching version with:\ncargo install golem-cli@{}\n", cli_version.as_str(), server_version.as_str(), server_version.as_str()).to_string()
        };

        let newer = srv_versions
            .iter()
            .filter(|&v| v > &cli_version)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

        if let Some(version) = newer {
            Err(GolemError(warning(cli_version, version)))
        } else {
            Ok(GolemResult::Str("No updates found".to_string()))
        }
    }
}
