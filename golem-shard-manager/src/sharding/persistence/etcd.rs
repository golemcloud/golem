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
use crate::sharding::etcd_connection::connect_for_requests;
use crate::sharding::etcd_retry::retry_retriable_until;
use crate::sharding::leader_election::LeaderFence;
use crate::sharding::model::ShardLeaseState;
use crate::sharding::shard_management::PERSISTENCE_TIMEOUT;
use async_trait::async_trait;
use etcd_client::{Client, Compare, CompareOp, Txn, TxnOp, TxnOpResponse, TxnResponse};
use golem_common::serialization::serialize;
use std::time::Duration;
use tokio::time::Instant;
use tracing::info;

/// Key holding the serialized [`ShardLeaseState`].
pub const STATE_KEY: &str = "/golem/shard-manager/state";

/// How long [`EtcdRoutingTablePersistence::read`] may spend retrying transient failures.
///
/// Kept under [`PERSISTENCE_TIMEOUT`], which fail-stops the whole round trip: retrying past it
/// would only replace a failure that names its cause with one that does not.
const READ_RETRY_BUDGET: Duration = Duration::from_secs(10);
// Leaves room for the attempt that may still be in flight when the budget is spent.
const _: () = assert!(READ_RETRY_BUDGET.as_secs() * 2 <= PERSISTENCE_TIMEOUT.as_secs());

pub struct EtcdRoutingTablePersistence {
    client: Client,
    number_of_shards: usize,
    /// Proof that this process won the leadership campaign, added to every write.
    fence: LeaderFence,
}

impl EtcdRoutingTablePersistence {
    pub async fn new(
        config: &EtcdConfig,
        number_of_shards: usize,
        fence: LeaderFence,
    ) -> Result<Self, ShardManagerError> {
        let client = connect_for_requests(config).await?;
        info!(
            endpoints = config.endpoints.join(", "),
            state_key = STATE_KEY,
            "Configured the etcd client for shard lease state persistence"
        );

        Ok(Self::with_client(client, number_of_shards, fence))
    }

    /// Builds a persistence over a client `run()` opened before campaigning, so its checks
    /// against the stored state happen while another replica still holds leadership.
    pub fn with_client(client: Client, number_of_shards: usize, fence: LeaderFence) -> Self {
        Self {
            client,
            number_of_shards,
            fence,
        }
    }

    /// The shard count the stored state was written with, or `None` if nothing is stored.
    ///
    /// Takes a bare client rather than `&self` because it runs before the campaign, where there
    /// is no fence to build a persistence with; reads are not fenced anyway.
    pub async fn stored_number_of_shards(
        client: &Client,
    ) -> Result<Option<usize>, ShardManagerError> {
        let mut kv = client.kv_client();
        let response = kv.get(STATE_KEY, None).await?;

        let Some(kv_pair) = response.kvs().first() else {
            return Ok(None);
        };

        Ok(Some(decode_shard_state(kv_pair.value())?.number_of_shards))
    }
}

#[async_trait]
impl RoutingTablePersistence for EtcdRoutingTablePersistence {
    async fn read(&self) -> Result<(ShardLeaseState, ExternalRevision), ShardManagerError> {
        let response = retry_retriable_until(
            "reading the shard lease state",
            || {
                let mut kv = self.client.kv_client();
                async move { Ok(kv.get(STATE_KEY, None).await?) }
            },
            Instant::now() + READ_RETRY_BUDGET,
        )
        .await?;

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
        // Not retried, unlike `read`: a retry re-sends the same expected revision, so an attempt
        // that did land comes back as a conflict. A refused write stops the process instead.
        check_prev_revision(prev_revision)?;
        check_state_for_write(shard_state)?;
        let encoded = serialize(shard_state).map_err(ShardManagerError::SerializationError)?;

        // etcd reports mod_revision 0 for a key that does not exist, so comparing it equal to
        // `prev_revision == NO_REVISION` already means "the key must not exist yet" - create-only
        // semantics, without the separate INSERT statement the SQL backend needs for the same
        // guarantee.
        let txn = Txn::new()
            // The revision compare alone is not a leadership fence - two replicas that both
            // read revision R both pass it. Only the fence makes leadership a precondition.
            .when([
                self.fence.compare(),
                Compare::mod_revision(STATE_KEY, CompareOp::Equal, prev_revision),
            ])
            .and_then([TxnOp::put(STATE_KEY, encoded, None)])
            // So a rejected write can say which precondition failed.
            .or_else([TxnOp::get(self.fence.key(), None)]);

        let mut kv = self.client.kv_client();
        let response = kv.txn(txn).await?;

        if !response.succeeded() {
            return Err(self.classify_failure(&response));
        }

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

impl EtcdRoutingTablePersistence {
    /// Tells the two rejection causes apart using the transaction's else-branch read.
    fn classify_failure(&self, response: &TxnResponse) -> ShardManagerError {
        let still_leader = matches!(
            response.op_responses().first(),
            Some(TxnOpResponse::Get(get))
                if get.kvs().first().is_some_and(|kv| {
                    kv.create_revision() == self.fence.create_revision()
                })
        );

        if still_leader {
            ShardManagerError::ConcurrentModification
        } else {
            // Leadership that cannot be confirmed, including a missing else-response, is
            // safer reported as lost.
            ShardManagerError::LeadershipLost {
                leader_key: self.fence.key_str(),
                create_revision: self.fence.create_revision(),
            }
        }
    }
}
