// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! etcd connections for the shard manager.
//!
//! Two clients, never cloned from one another: `etcd-client` turns its request timeout into a
//! tonic endpoint timeout that applies to every RPC on the channel, and `Client::clone` shares
//! that channel. The election RPCs block by design, so they need a channel with no timeout.

use crate::config::EtcdConfig;
use crate::sharding::error::ShardManagerError;
use etcd_client::{Client, ConnectOptions};

fn check_endpoints(config: &EtcdConfig) -> Result<(), ShardManagerError> {
    if config.endpoints.is_empty() {
        return Err(ShardManagerError::Internal(
            "etcd shard state persistence requires at least one endpoint".to_string(),
        ));
    }

    // TLS is not configurable, and anything else - a scheme-less `host:port` included - would
    // otherwise fail at connect time with an opaque error.
    if let Some(endpoint) = config
        .endpoints
        .iter()
        .find(|endpoint| !endpoint.starts_with("http://"))
    {
        return Err(ShardManagerError::Internal(format!(
            "etcd endpoint {endpoint} must start with http:// (TLS is not supported)"
        )));
    }

    Ok(())
}

/// A client for ordinary key/value requests, bounded by `request_timeout`.
pub async fn connect_for_requests(config: &EtcdConfig) -> Result<Client, ShardManagerError> {
    check_endpoints(config)?;

    let options = ConnectOptions::new()
        .with_connect_timeout(config.connect_timeout)
        .with_timeout(config.request_timeout);

    Ok(Client::connect(&config.endpoints, Some(options)).await?)
}

/// A client for the leadership campaign and its lease keepalive.
pub async fn connect_for_election(config: &EtcdConfig) -> Result<Client, ShardManagerError> {
    check_endpoints(config)?;

    let options = ConnectOptions::new().with_connect_timeout(config.connect_timeout);

    Ok(Client::connect(&config.endpoints, Some(options)).await?)
}
