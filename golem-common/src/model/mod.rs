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

pub mod account;
pub mod account_usage;
pub mod agent;
pub mod application;
pub mod auth;
pub mod base64;
pub mod card;
pub mod certificate;
pub mod component;
pub mod component_metadata;
pub mod deployment;
pub mod diff;
pub mod domain_registration;
pub mod entity;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_tool_grant;
pub mod environment_tool_middleware_grant;
pub mod error;
pub mod filesystem;
pub mod http_api_deployment;
pub mod invocation_context;
pub mod invocation_session_public;
pub mod login;
pub mod lucene;
pub mod mcp_deployment;
pub mod oplog;
pub mod parsed_function_name;
pub mod permission_share;
pub mod plan;
pub mod plugin_registration;
pub mod poem;
pub mod protobuf;
pub mod quota;
pub mod regions;
pub mod reports;
pub mod retry_policy;
pub mod security_scheme;
pub mod sharding;
#[cfg(test)]
mod tests;
pub mod tool_middleware;
pub mod tool_middleware_release;
pub mod tool_release;

use std::fmt::{Display, Formatter};
use std::ops::Add;
use std::time::Duration;

pub use agent::{
    AgentEvent, AgentInvocation, AgentInvocationKind, AgentInvocationOutput,
    AgentInvocationPayload, AgentInvocationResult, AgentMetadata, AgentStatusRecord,
    AssistedSelection, AuthoritativeSnapshot, AuthoritativeSnapshotKind, AutomaticSnapshot,
    DEFAULT_INVOCATION_RESULT_BLOOM_BITS, DEFAULT_INVOCATION_RESULT_BLOOM_HASHES,
    DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY, DURABLE_STREAM_SESSION_RECENT_CAPACITY,
    DurableStreamPublicBinding, DurableStreamSessionIndex, DurableStreamSessionStatus,
    ExportForkAdmissions, ExportForkReservation, FailedUpdateRecord, InvocationResultBloom,
    InvocationResultMembership, InvocationStatus, LogLevel, OplogProcessorCheckpointState,
    OwnedAgentId, PendingCardEventRef, PendingInvocationRef, PendingUpdateKind, PendingUpdateRef,
    RedactedAgentInvocationResult, ScheduleId, ScheduledAction, SnapshotFiles,
    SuccessfulUpdateRecord, TimestampedAgentInvocation, UsableAutomaticSnapshot,
};
pub use card::{
    ReceivedCardTransferIndex, ReceivedCardTransferState, SourceCardTransferIndex,
    SourceCardTransferState,
};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
pub use poem::{Page, PoemMultipartTypeRequirements, PoemTypeRequirements};
pub use retry_policy::{
    FixedRng, NamedRetryPolicy, Predicate, PredicateValue, PredicateValueType, RetryConfig,
    RetryContext, RetryEvaluationError, RetryPolicy, RetryPolicyState, RetryProperties,
    RetryVerdict, RngSource, ThreadRng,
};
pub use sharding::{
    NumberOfShards, Pod, RoutingTable, RoutingTableEntry, ShardAssignment, ShardDeliveryOutcome,
    ShardEpoch, ShardLeaseRevision,
};
use url::Url;

#[cfg(test)]
use self::account::{AccountEmail, AccountId};
use self::component::{AgentFilePermissions, ComponentId};
#[cfg(test)]
use crate::base_model::agent::{AgentMode, Principal};
pub use crate::base_model::*;

impl Timestamp {
    pub fn now_utc() -> Timestamp {
        Timestamp(iso8601_timestamp::Timestamp::now_utc())
    }

    pub fn to_millis(&self) -> u64 {
        self.0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH)
            .whole_milliseconds() as u64
    }

    pub fn rounded(self) -> Self {
        Self::from(self.to_millis())
    }
}

impl BinarySerializer for Timestamp {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        BinarySerializer::serialize(
            &(self
                .0
                .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH)
                .whole_milliseconds() as u64),
            context,
        )
    }
}

impl BinaryDeserializer for Timestamp {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let timestamp: u64 = BinaryDeserializer::deserialize(context)?;
        Ok(Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(timestamp)),
        ))
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub enum ForkResult {
    /// The original worker that called `fork`
    Original,
    /// The new worker
    Forked,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct RdbmsPoolKey {
    pub address: Url,
}

impl RdbmsPoolKey {
    pub fn new(address: Url) -> Self {
        Self { address }
    }

    pub fn from(address: &str) -> Result<Self, String> {
        let url = Url::parse(address).map_err(|e| e.to_string())?;
        Ok(Self::new(url))
    }

    pub fn masked_address(&self) -> String {
        let mut output: String = self.address.scheme().to_string();
        output.push_str("://");

        let username = self.address.username();
        output.push_str(username);

        let password = self.address.password();
        if password.is_some() {
            output.push_str(":*****");
        }

        if let Some(h) = self.address.host_str() {
            if !username.is_empty() || password.is_some() {
                output.push('@');
            }

            output.push_str(h);

            if let Some(p) = self.address.port() {
                output.push(':');
                output.push_str(p.to_string().as_str());
            }
        }

        output.push_str(self.address.path());

        let query_pairs = self.address.query_pairs();

        if query_pairs.count() > 0 {
            output.push('?');
        }
        for (index, (key, value)) in query_pairs.enumerate() {
            let key = &*key;
            output.push_str(key);
            output.push('=');

            if key == "password" || key == "secret" {
                output.push_str("*****");
            } else {
                output.push_str(&value);
            }
            if index < query_pairs.count() - 1 {
                output.push('&');
            }
        }

        output
    }
}

impl Display for RdbmsPoolKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.masked_address())
    }
}
