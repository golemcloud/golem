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

//! Each record is one key, `mcp_session:<id>`, whose Redis expiry is the session's expiry.

use super::{McpSessionId, McpSessionRecord, McpSessionStore, millis};
use crate::custom_api::oidc::session_store::SessionStoreError;
use async_trait::async_trait;
use bytes::Bytes;
use fred::types::{Expiration, SetOptions};
use golem_common::redis::RedisPool;
use std::time::Duration;

const SVC_NAME: &str = "mcp_session_store";

fn key(id: McpSessionId) -> String {
    format!("mcp_session:{id}")
}

/// MCP session store on Redis. The pool's database index and key prefix apply.
pub struct RedisMcpSessionStore {
    redis: RedisPool,
}

impl RedisMcpSessionStore {
    pub fn new(redis: RedisPool) -> Self {
        Self { redis }
    }
}

#[async_trait]
impl McpSessionStore for RedisMcpSessionStore {
    async fn create(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
        ttl: Duration,
    ) -> Result<bool, SessionStoreError> {
        let stored: Option<String> = self
            .redis
            .with(SVC_NAME, "create")
            .set(
                key(id),
                record.encode()?,
                Some(Expiration::PX(millis(ttl).max(1))),
                Some(SetOptions::NX),
                false,
            )
            .await?;
        Ok(stored.is_some())
    }

    async fn get(
        &self,
        id: McpSessionId,
        extend: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, SessionStoreError> {
        let bytes: Option<Bytes> = match extend {
            Some(idle) => {
                self.redis
                    .with(SVC_NAME, "get_and_extend")
                    .getex(key(id), idle)
                    .await?
            }
            None => self.redis.with(SVC_NAME, "get").get(key(id)).await?,
        };
        bytes
            .map(|bytes| McpSessionRecord::decode(&bytes))
            .transpose()
    }

    async fn replace(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
    ) -> Result<bool, SessionStoreError> {
        let stored: Option<String> = self
            .redis
            .with(SVC_NAME, "replace")
            .set(
                key(id),
                record.encode()?,
                Some(Expiration::KEEPTTL),
                Some(SetOptions::XX),
                false,
            )
            .await?;
        Ok(stored.is_some())
    }

    async fn delete(&self, id: McpSessionId) -> Result<(), SessionStoreError> {
        let _: i64 = self.redis.with(SVC_NAME, "delete").del(key(id)).await?;
        Ok(())
    }
}
