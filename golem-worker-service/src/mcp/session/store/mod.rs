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

//! The shared store of MCP session records, on Redis, Postgres or SQLite. Each implementation
//! gets its connection pool from its caller, so it can share the pool of the gateway session
//! store.

mod db;
mod redis;

pub use db::{DbMcpSessionStore, PostgresMcpSessionStore, SqliteMcpSessionStore};
pub use redis::RedisMcpSessionStore;

use crate::custom_api::oidc::session_store::SessionStoreError;
use anyhow::anyhow;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use rand::TryRngCore;
use rand::rngs::OsRng;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

/// Whole milliseconds of `duration`, saturating at `i64::MAX`.
fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Expiring records of MCP sessions, shared by every replica. A read never returns a record that
/// is not live, and no operation makes a record live again.
#[async_trait]
pub trait McpSessionStore: Send + Sync {
    /// Stores a new record, live for `ttl`. `false` when the ID is taken.
    async fn create(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
        ttl: Duration,
    ) -> Result<bool, SessionStoreError>;

    /// The live record. With `Some(idle)` the record stays live for at least `idle` from now;
    /// the record itself is not changed.
    async fn get(
        &self,
        id: McpSessionId,
        extend: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, SessionStoreError>;

    /// Replaces the live record and keeps its expiry. `false` when it is not live.
    async fn replace(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
    ) -> Result<bool, SessionStoreError>;

    /// Removes the record. Removing a missing record is not an error.
    async fn delete(&self, id: McpSessionId) -> Result<(), SessionStoreError>;
}

/// What a session is bound to: the domain and environment it was opened for, and the
/// deployment revision its client last listed (the pin).
#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct McpSessionRecord {
    pub domain: Domain,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
}

impl McpSessionRecord {
    fn encode(&self) -> Result<Vec<u8>, SessionStoreError> {
        golem_common::serialization::serialize(self)
            .map_err(|e| anyhow!("McpSessionRecord serialization error: {e}").into())
    }

    fn decode(bytes: &[u8]) -> Result<Self, SessionStoreError> {
        golem_common::serialization::deserialize(bytes)
            .map_err(|e| anyhow!("McpSessionRecord deserialization error: {e}").into())
    }
}

/// The 128-bit random identifier of an MCP session. Its text form, used in the
/// `Mcp-Session-Id` header, is 32 lowercase hexadecimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct McpSessionId([u8; 16]);

impl McpSessionId {
    /// A new identifier from the operating system's random number generator.
    pub fn generate() -> Result<Self, rand::rand_core::OsError> {
        let mut bytes = [0u8; 16];
        OsRng.try_fill_bytes(&mut bytes)?;
        Ok(Self(bytes))
    }

    fn uuid(self) -> Uuid {
        Uuid::from_bytes(self.0)
    }
}

impl Display for McpSessionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", u128::from_be_bytes(self.0))
    }
}

impl FromStr for McpSessionId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let is_id = value.len() == 32
            && value
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        is_id
            .then(|| u128::from_str_radix(value, 16).ok())
            .flatten()
            .map(|id| Self(id.to_be_bytes()))
            .ok_or("an MCP session ID is 32 lowercase hexadecimal digits")
    }
}

#[cfg(test)]
mod tests {
    use super::McpSessionId;
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn session_id_text_form_round_trips() {
        let id = McpSessionId::generate().unwrap();
        let text = id.to_string();
        assert_eq!(text.len(), 32);
        assert_eq!(McpSessionId::from_str(&text), Ok(id));
    }

    #[test]
    fn session_id_rejects_other_text() {
        [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789abcdef",
            "+123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdeg",
        ]
        .into_iter()
        .for_each(|text| assert!(McpSessionId::from_str(text).is_err(), "{text}"));
    }
}
