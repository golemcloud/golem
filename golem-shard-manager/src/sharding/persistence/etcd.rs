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

use super::{
    ExternalRevision, NO_REVISION, RoutingTablePersistence, check_prev_revision,
    check_state_for_write, check_stored_revision, decode_shard_state,
};
use crate::config::EtcdConfig;
use crate::sharding::error::ShardManagerError;
use crate::sharding::model::ShardLeaseState;
use async_trait::async_trait;
use etcd_client::{Client, Compare, CompareOp, ConnectOptions, Txn, TxnOp};
use golem_common::serialization::serialize;
use tracing::info;

/// Key holding the serialized [`ShardLeaseState`].
pub const STATE_KEY: &str = "/golem/shard-manager/state";

pub struct EtcdRoutingTablePersistence {
    client: Client,
    number_of_shards: usize,
}

impl EtcdRoutingTablePersistence {
    pub async fn new(
        config: &EtcdConfig,
        number_of_shards: usize,
    ) -> Result<Self, ShardManagerError> {
        if config.endpoints.is_empty() {
            return Err(ShardManagerError::Internal(
                "etcd shard state persistence requires at least one endpoint".to_string(),
            ));
        }

        // TLS is not configurable, so an `https://` endpoint could only fail at connect time with
        // an opaque transport error. Refuse it up front and say why instead.
        if let Some(endpoint) = config
            .endpoints
            .iter()
            .find(|endpoint| !endpoint.starts_with("http://"))
        {
            return Err(ShardManagerError::Internal(format!(
                "etcd endpoint {endpoint} is not an http:// URL; TLS is not supported"
            )));
        }

        let options = ConnectOptions::new()
            .with_connect_timeout(config.connect_timeout)
            .with_timeout(config.request_timeout);

        let client = Client::connect(&config.endpoints, Some(options)).await?;
        info!(
            endpoints = config.endpoints.join(", "),
            state_key = STATE_KEY,
            "Connected to etcd for shard lease state persistence"
        );

        Ok(Self {
            client,
            number_of_shards,
        })
    }
}

#[async_trait]
impl RoutingTablePersistence for EtcdRoutingTablePersistence {
    async fn read(&self) -> Result<(ShardLeaseState, ExternalRevision), ShardManagerError> {
        // `KvClient` is a cheap handle over the shared multiplexed channel; cloning it per call is
        // how the generated stubs' `&mut self` is satisfied.
        let mut kv = self.client.kv_client();
        let response = kv.get(STATE_KEY, None).await?;

        let Some(kv_pair) = response.kvs().first() else {
            return Ok((ShardLeaseState::new(self.number_of_shards), NO_REVISION));
        };

        // etcd's store starts at revision 1 and every mutation increments it, so a live key cannot
        // have mod_revision 0. If it does, the "0 means absent" invariant is broken.
        let revision = kv_pair.mod_revision();
        if revision < 1 {
            return Err(ShardManagerError::Internal(format!(
                "etcd returned key {STATE_KEY} with mod_revision {revision}, which is reserved for \
                 absent keys"
            )));
        }

        Ok((decode_shard_state(kv_pair.value())?, revision))
    }

    async fn write(
        &self,
        shard_state: &ShardLeaseState,
        prev_revision: ExternalRevision,
    ) -> Result<ExternalRevision, ShardManagerError> {
        check_prev_revision(prev_revision)?;
        check_state_for_write(shard_state)?;
        let encoded = serialize(shard_state).map_err(ShardManagerError::SerializationError)?;

        // etcd reports mod_revision 0 for a key that does not exist, so `prev_revision ==
        // NO_REVISION` already means exactly "must not exist yet" - unlike the SQL backend, no
        // branching is needed here.
        //
        // `or_else` is deliberately empty. Returning the winning state from a lost compare-and-swap
        // would have to travel inside the error variant, which the SQL backend cannot fill in
        // without a second query; the caller re-reads instead.
        let txn = Txn::new()
            .when([Compare::mod_revision(
                STATE_KEY,
                CompareOp::Equal,
                prev_revision,
            )])
            .and_then([TxnOp::put(STATE_KEY, encoded, None)]);

        let mut kv = self.client.kv_client();
        let response = kv.txn(txn).await?;

        if !response.succeeded() {
            return Err(ShardManagerError::ConcurrentModification);
        }

        // A transaction that applied at least one mutation reports the NEW store revision in its
        // header, and etcd stamps every key written by that transaction with exactly that
        // revision - so this is the mod_revision our PUT produced.
        //
        // Do NOT replace this with a follow-up GET: another writer can land between the
        // transaction and the GET, and we would cache their revision as ours, so our next
        // compare-and-swap would compare-equal against their write and silently clobber it.
        let revision = response
            .header()
            .ok_or_else(|| {
                ShardManagerError::Internal(
                    "etcd transaction response carried no header".to_string(),
                )
            })?
            .revision();

        check_stored_revision(revision, prev_revision)
    }
}
