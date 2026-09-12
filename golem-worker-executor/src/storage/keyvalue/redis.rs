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

use async_trait::async_trait;
use bytes::Bytes;
use fred::error::ErrorKind;
use fred::types::SetOptions;
use golem_common::metrics::redis::{record_redis_deserialized_size, record_redis_serialized_size};
use golem_common::redis::{RedisError, RedisPool};
use std::collections::HashMap;
use std::sync::Arc;

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};

impl From<RedisError> for KeyValueStorageError {
    fn from(error: RedisError) -> Self {
        let message = error.to_string();
        match error.kind() {
            // The client refused to queue the command, and asks the caller to retry it.
            ErrorKind::Backpressure => Self::NotAttempted(message),
            // The command may already have been written to the connection when it failed. The
            // client's own reconnect policy only re-establishes the connection; it does not make
            // these failures invisible to the caller.
            ErrorKind::IO
            | ErrorKind::Timeout
            | ErrorKind::Canceled
            | ErrorKind::Cluster
            | ErrorKind::Routing => Self::Transient(message),
            // A server reply the client has no kind for. Most are permanent (`ERR`, `WRONGTYPE`
            // arrive with their own kinds; what lands here is unparsed). The exceptions are the
            // replies a node gives while a failover is in progress, which the client also folds
            // into `Unknown` - see `is_failover_refusal`.
            ErrorKind::Unknown if is_failover_refusal(error.details()) => {
                Self::NotAttempted(message)
            }
            _ => Self::Other(message),
        }
    }
}

/// Whether a server reply is one of the refusals a node sends while a failover is in progress.
///
/// fred 10.1.0 (`protocol::utils::pretty_error`) maps only `MOVED`, `ASK` and `CLUSTERDOWN` to
/// `ErrorKind::Cluster`; the other replies a failover produces come back as `ErrorKind::Unknown`
/// with the raw reply as the details. All four are refusals - the server did not execute the
/// command - so a retry cannot duplicate a write:
///
/// * `READONLY`: the node this connection is on was demoted to a replica. With the
///   `custom-reconnect-errors` feature enabled in the workspace, fred treats this (and `LOADING`
///   and `CLUSTERDOWN`) as a reconnect trigger and replays the command up to its
///   `max_command_attempts` before it reaches this classification, so what arrives here is the
///   residue after the client's own reconnect gave up.
/// * `LOADING`: the node is still loading its dataset, typically the new primary right after
///   promotion.
/// * `MASTERDOWN`: a replica that has lost its primary and is configured not to serve stale data.
/// * `TRYAGAIN`: a multi-key command hit a slot that is mid-migration.
///
/// The prefix is matched as the first whitespace-delimited token, which is how Redis formats every
/// error reply and how fred's own classification reads them.
fn is_failover_refusal(details: &str) -> bool {
    matches!(
        details.split_whitespace().next(),
        Some("READONLY" | "LOADING" | "MASTERDOWN" | "TRYAGAIN")
    )
}

#[derive(Debug)]
pub struct RedisKeyValueStorage {
    redis: RedisPool,
}

impl RedisKeyValueStorage {
    pub fn new(redis: RedisPool) -> Self {
        Self { redis }
    }

    fn use_hash(namespace: &KeyValueStorageNamespace) -> Option<String> {
        match namespace {
            KeyValueStorageNamespace::Worker { .. } => None,
            // Per-agent hash: each agent's split status fields live in their own hash so that
            // `hkeys`/`hdel` operate on a single agent's fields and multi-field writes are atomic.
            KeyValueStorageNamespace::AgentStatus { agent_id } => {
                Some(format!("agent-status:{}", agent_id.to_redis_key()))
            }
            KeyValueStorageNamespace::AgentInvocationResultIndex { agent_id } => Some(format!(
                "agent-invocation-result-index:{}",
                agent_id.to_redis_key()
            )),
            // Per-agent clean checkpoint hash; same per-agent isolation as `AgentStatus`.
            KeyValueStorageNamespace::AgentStatusCheckpoint { agent_id } => Some(format!(
                "agent-status-checkpoint:{}",
                agent_id.to_redis_key()
            )),
            KeyValueStorageNamespace::AgentDurableStreamSessionIndex { agent_id } => Some(format!(
                "agent:durable_stream_session_index:{}",
                agent_id.to_redis_key()
            )),
            KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots { agent_id } => Some(format!(
                "agent:rejected_periodic_snapshots:{}",
                agent_id.to_redis_key()
            )),
            KeyValueStorageNamespace::RunningWorkers => None,
            KeyValueStorageNamespace::Promise { .. } => Some("promises".to_string()),
            KeyValueStorageNamespace::Schedule => None,
            KeyValueStorageNamespace::UserDefined {
                environment_id,
                bucket,
            } => Some(format!("user-defined:{environment_id}:{bucket}")),
        }
    }
}

#[async_trait]
impl KeyValueStorage for RedisKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hset(ns, (key, value))
                .await
                .map_err(KeyValueStorageError::from),
            None => self
                .redis
                .with(svc_name, api_name)
                .set(key, value, None, None, false)
                .await
                .map_err(KeyValueStorageError::from),
        }
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), KeyValueStorageError> {
        let mut map: HashMap<&str, &[u8]> = HashMap::new();
        for (k, v) in pairs {
            map.insert(*k, *v);
            record_redis_serialized_size(svc_name, entity_name, v.len());
        }
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hmset(ns, map)
                .await
                .map_err(KeyValueStorageError::from),
            None => self
                .redis
                .with(svc_name, api_name)
                .mset(map)
                .await
                .map_err(KeyValueStorageError::from),
        }
    }

    async fn compare_and_set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        expected: Option<&[u8]>,
        pairs: &[(&str, &[u8])],
    ) -> Result<bool, KeyValueStorageError> {
        for (_, value) in pairs {
            record_redis_serialized_size(svc_name, entity_name, value.len());
        }
        let Some(namespace) = Self::use_hash(&namespace) else {
            // A shape the caller got wrong, not a backend that is briefly unwell, so it must not
            // be retried.
            return Err(KeyValueStorageError::Other(
                "compare_and_set_many is unsupported for non-hash Redis namespaces".to_string(),
            ));
        };
        self.redis
            .with(svc_name, api_name)
            .compare_and_set_many_hash(namespace, key, expected, pairs)
            .await
            .map_err(KeyValueStorageError::from)
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, KeyValueStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        match Self::use_hash(&namespace) {
            Some(ns) => {
                let result: bool = self
                    .redis
                    .with(svc_name, api_name)
                    .hsetnx(ns, key, value)
                    .await
                    .map_err(KeyValueStorageError::from)?;

                Ok(result)
            }
            None => {
                let result: Option<String> = self
                    .redis
                    .with(svc_name, api_name)
                    .set(key, value, None, Some(SetOptions::NX), false)
                    .await
                    .map_err(KeyValueStorageError::from)?;

                Ok(result == Some("OK".to_string()))
            }
        }
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, KeyValueStorageError> {
        let serialized: Option<Bytes> = match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hget(ns, key)
                .await
                .map_err(KeyValueStorageError::from)?,
            None => self
                .redis
                .with(svc_name, api_name)
                .get(key)
                .await
                .map_err(KeyValueStorageError::from)?,
        };

        if let Some(serialized) = serialized {
            record_redis_deserialized_size(svc_name, entity_name, serialized.len());
            Ok(Some(serialized))
        } else {
            Ok(None)
        }
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Arc<[String]>,
    ) -> Result<Vec<Option<Bytes>>, KeyValueStorageError> {
        // fred takes `Into<MultipleKeys>`, which `Arc<[String]>` does not implement, so the batch
        // is materialised once here. The retry decorator no longer repeats it per attempt.
        let keys = keys.to_vec();
        let serialized: Vec<Option<Bytes>> = match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hmget(ns, keys)
                .await
                .map_err(KeyValueStorageError::from)?,
            None => self
                .redis
                .with(svc_name, api_name)
                .mget(keys)
                .await
                .map_err(KeyValueStorageError::from)?,
        };

        for s in serialized.iter().flatten() {
            record_redis_deserialized_size(svc_name, entity_name, s.len());
        }

        Ok(serialized)
    }

    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError> {
        let pairs: Vec<(String, Bytes)> = match Self::use_hash(&namespace) {
            // `HGETALL` returns every field/value of the hash in a single atomic command.
            Some(ns) => {
                let map: HashMap<String, Bytes> = self
                    .redis
                    .with(svc_name, api_name)
                    .hgetall(ns)
                    .await
                    .map_err(KeyValueStorageError::from)?;
                map.into_iter().collect()
            }
            None => {
                return Err(KeyValueStorageError::other(
                    "get_all is only supported for Redis hash namespaces",
                ));
            }
        };

        for (_, value) in &pairs {
            record_redis_deserialized_size(svc_name, entity_name, value.len());
        }

        Ok(pairs)
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), KeyValueStorageError> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hdel(ns, key)
                .await
                .map_err(KeyValueStorageError::from),
            None => self
                .redis
                .with(svc_name, api_name)
                .del(key)
                .await
                .map_err(KeyValueStorageError::from),
        }
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Arc<[String]>,
    ) -> Result<(), KeyValueStorageError> {
        // See `get_many`: fred needs an owned collection it can convert into `MultipleKeys`.
        let keys = keys.to_vec();
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hdel(ns, keys)
                .await
                .map_err(KeyValueStorageError::from),
            None => self
                .redis
                .with(svc_name, api_name)
                .del_many(keys)
                .await
                .map_err(KeyValueStorageError::from),
        }
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, KeyValueStorageError> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hexists(ns, key)
                .await
                .map_err(KeyValueStorageError::from),
            None => self
                .redis
                .with(svc_name, api_name)
                .exists(key)
                .await
                .map_err(KeyValueStorageError::from),
        }
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, KeyValueStorageError> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hkeys(ns)
                .await
                .map_err(KeyValueStorageError::from),
            None => self
                .redis
                .with(svc_name, api_name)
                .keys("*".to_string())
                .await
                .map_err(KeyValueStorageError::from),
        }
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .sadd(&key, value)
            .await
            .map_err(KeyValueStorageError::from)
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .srem(&key, value)
            .await
            .map_err(KeyValueStorageError::from)
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, KeyValueStorageError> {
        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        let members: Vec<Bytes> = self
            .redis
            .with(svc_name, api_name)
            .smembers(&key)
            .await
            .map_err(KeyValueStorageError::from)?;

        for member in &members {
            record_redis_deserialized_size(svc_name, entity_name, member.len());
        }

        Ok(members)
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .zadd(&key, None, None, false, false, (score, value))
            .await
            .map_err(KeyValueStorageError::from)
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .zrem(&key, value)
            .await
            .map_err(KeyValueStorageError::from)
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        let pairs: Vec<(Bytes, f64)> = self
            .redis
            .with(svc_name, api_name)
            .zrange(&key, 0, -1, None, false, None, true)
            .await
            .map_err(KeyValueStorageError::from)?;

        for (data, _score) in &pairs {
            record_redis_deserialized_size(svc_name, entity_name, data.len());
        }

        Ok(pairs
            .into_iter()
            .map(|(data, score)| (score, data))
            .collect())
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{ns}:{key}"),
            None => key.to_string(),
        };
        let pairs: Vec<(Bytes, f64)> = self
            .redis
            .with(svc_name, api_name)
            .zrangebyscore(&key, min, max, true, None)
            .await
            .map_err(KeyValueStorageError::from)?;

        for (data, _score) in &pairs {
            record_redis_deserialized_size(svc_name, entity_name, data.len());
        }

        Ok(pairs
            .into_iter()
            .map(|(data, score)| (score, data))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// Backpressure means the client refused to queue the command, so it never left the process.
    #[test]
    fn backpressure_is_classified_as_not_attempted() {
        let error =
            KeyValueStorageError::from(RedisError::new(ErrorKind::Backpressure, "too many"));
        assert!(
            matches!(error, KeyValueStorageError::NotAttempted(_)),
            "{error:?}"
        );
    }

    /// Everything that can fail with the command already on the wire stays `Transient`.
    #[test]
    fn connection_failures_are_classified_as_transient() {
        for kind in [
            ErrorKind::IO,
            ErrorKind::Timeout,
            ErrorKind::Canceled,
            ErrorKind::Cluster,
            ErrorKind::Routing,
        ] {
            let error = KeyValueStorageError::from(RedisError::new(kind.clone(), "boom"));
            assert!(
                matches!(error, KeyValueStorageError::Transient(_)),
                "{kind:?}: {error:?}"
            );
        }
    }

    /// The replies a node sends while a failover is in progress, exactly as Redis formats them.
    /// fred gives all of these `ErrorKind::Unknown` with the reply as the details (its `protocol`
    /// module is private, so the mapping is reproduced here rather than called), and each one is
    /// a refusal: the command was not executed, so retrying it cannot duplicate a write.
    #[test]
    fn failover_refusals_are_classified_as_not_attempted() {
        for reply in [
            "READONLY You can't write against a read only replica.",
            "LOADING Redis is loading the dataset in memory",
            "MASTERDOWN Link with MASTER is down and replica-serve-stale-data is set to 'no'.",
            "TRYAGAIN Multiple keys request during rehashing of slot",
        ] {
            let error = KeyValueStorageError::from(RedisError::new(ErrorKind::Unknown, reply));
            assert!(
                matches!(error, KeyValueStorageError::NotAttempted(_)),
                "{reply}: {error:?}"
            );
        }
    }

    /// Only those prefixes are retried. Other `Unknown` errors - a reply the client did not
    /// parse, or a client-internal channel failure - are not a failover and stay permanent.
    #[test]
    fn unknown_errors_that_are_not_failover_refusals_are_not_retried() {
        for details in [
            "ERR unknown command 'FOO'",
            "NOSCRIPT No matching script. Please use EVAL.",
            "EXECABORT Transaction discarded because of previous errors.",
            "channel closed",
            "",
            // The prefix must be the whole first token, not a substring of one.
            "READONLYISH",
            "not READONLY",
        ] {
            let error = KeyValueStorageError::from(RedisError::new(ErrorKind::Unknown, details));
            assert!(
                matches!(error, KeyValueStorageError::Other(_)),
                "{details:?}: {error:?}"
            );
        }
    }

    /// Everything else is permanent, so the retry policy leaves it alone.
    #[test]
    fn other_redis_errors_are_not_retried() {
        for kind in [
            ErrorKind::Auth,
            ErrorKind::Config,
            ErrorKind::InvalidArgument,
            ErrorKind::InvalidCommand,
            ErrorKind::NotFound,
            ErrorKind::Parse,
            ErrorKind::Protocol,
            ErrorKind::Sentinel,
            ErrorKind::Tls,
            ErrorKind::Unknown,
            ErrorKind::Url,
        ] {
            let error = KeyValueStorageError::from(RedisError::new(kind.clone(), "boom"));
            assert!(
                matches!(error, KeyValueStorageError::Other(_)),
                "{kind:?}: {error:?}"
            );
        }
    }
}
