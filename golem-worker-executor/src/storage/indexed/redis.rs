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

use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
    ScanResume, WriterId,
};
use async_trait::async_trait;
use bytes::Bytes;
use fred::error::ErrorKind;
use fred::prelude::{Key, Value};
use fred::types::config::Options;
use fred::types::streams::XCapKind;
use golem_common::metrics::redis::{record_redis_deserialized_size, record_redis_serialized_size};
use golem_common::model::ShardEpoch;
use golem_common::redis::{RedisError, RedisPool};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Redis checks a key's epoch in a Lua script, which it runs atomically with the `XADD`s the
/// script guards. Two limits follow from Redis itself rather than from this code: an epoch is
/// only as durable as the last write the server kept, so a failover that loses the tail can lose
/// a raised epoch and let the previous writer back in; and the script names two keys, which a
/// Redis Cluster would need in one slot - this backend does not support Cluster.
#[derive(Debug)]
pub struct RedisIndexedStorage {
    redis: RedisPool,
    writer_id: WriterId,
}

impl RedisIndexedStorage {
    pub fn new(redis: RedisPool) -> Self {
        Self {
            redis,
            writer_id: WriterId::process(),
        }
    }

    /// The same store written as `writer_id`. See [`WriterId`] for why a process uses one value.
    pub fn for_writer(mut self, writer_id: WriterId) -> Self {
        self.writer_id = writer_id;
        self
    }

    /// `ARGV`: the asserted epoch, the writer, then `id, value` pairs.
    ///
    /// Numbers stay decimal strings throughout, because a Lua number is a double and loses
    /// precision above 2^53; with no leading zeros they order by length and then lexically. The
    /// ids are checked against the stream before the first `XADD` because a script is atomic but
    /// not transactional: an `XADD` failing half way would leave the ones before it behind.
    const FENCED_APPEND_SCRIPT: &'static str = r#"
local stored = redis.call('HMGET', KEYS[2], 'epoch', 'writer')
if stored[1] == false then
  return redis.error_reply('FENCED - 0')
end
if stored[1] ~= ARGV[1] then
  return redis.error_reply('FENCED ' .. stored[1] .. ' 0')
end
if stored[2] ~= ARGV[2] then
  return redis.error_reply('FENCED ' .. stored[1] .. ' 1')
end
local top = nil
if redis.call('EXISTS', KEYS[1]) == 1 then
  local info = redis.call('XINFO', 'STREAM', KEYS[1])
  for i = 1, #info, 2 do
    if info[i] == 'last-generated-id' then
      top = string.match(info[i + 1], '^(%d+)')
    end
  end
end
for i = 3, #ARGV, 2 do
  local id = ARGV[i]
  if top and not (#id > #top or (#id == #top and id > top)) then
    return redis.error_reply('ERR The ID specified in XADD is equal or smaller than the target stream top item')
  end
  top = id
end
for i = 3, #ARGV, 2 do
  redis.call('XADD', KEYS[1], ARGV[i], 'key', ARGV[i + 1])
end
return redis.status_reply('OK')
"#;

    /// `ARGV`: the epoch and the writer, compared the way [`Self::FENCED_APPEND_SCRIPT`] does.
    const SET_KEY_EPOCH_SCRIPT: &'static str = r#"
local stored = redis.call('HMGET', KEYS[1], 'epoch', 'writer')
local epoch = stored[1]
local higher = epoch ~= false and (#ARGV[1] > #epoch or (#ARGV[1] == #epoch and ARGV[1] > epoch))
if epoch == false or higher or (epoch == ARGV[1] and stored[2] == ARGV[2]) then
  redis.call('HSET', KEYS[1], 'epoch', ARGV[1], 'writer', ARGV[2])
  return redis.status_reply('OK')
end
if epoch == ARGV[1] then
  return redis.error_reply('FENCED ' .. epoch .. ' 1')
end
return redis.error_reply('FENCED ' .. epoch .. ' 0')
"#;

    /// `KEYS`: the stream, then its epoch record. `ARGV`: the epoch and the writer the delete
    /// asserts, compared the way [`Self::FENCED_APPEND_SCRIPT`] does, or nothing for an
    /// unconditional delete. Both keys go in the one `DEL`, so a refused delete removes neither.
    const DELETE_WITH_EPOCH_SCRIPT: &'static str = r#"
if #ARGV > 0 then
  local stored = redis.call('HMGET', KEYS[2], 'epoch', 'writer')
  if stored[1] == false then
    if redis.call('EXISTS', KEYS[1]) == 0 then
      return redis.status_reply('OK')
    end
    return redis.error_reply('FENCED - 0')
  end
  if stored[1] ~= ARGV[1] then
    return redis.error_reply('FENCED ' .. stored[1] .. ' 0')
  end
  if stored[2] ~= ARGV[2] then
    return redis.error_reply('FENCED ' .. stored[1] .. ' 1')
  end
end
redis.call('DEL', KEYS[1], KEYS[2])
return redis.status_reply('OK')
"#;

    /// Where a key's epoch lives. Not under the key's own name: `scan` matches `...oplog:*`, and
    /// an `...oplog:<key>:epoch` sibling would come back from it as a key of its own.
    fn epoch_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog {
                agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:oplog-epoch:{key}")
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_id: _,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:c{level}-oplog-epoch:{key}")
            }
            // A stage is hidden and has one writer, so it never asserts an epoch; the key exists
            // only to keep this total.
            IndexedStorageNamespace::StagedOpLog {
                agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:staged-oplog-epoch:{key}")
            }
        }
    }

    /// The `FENCED <stored epoch or -> <writer conflict 0|1>` reply of the scripts above.
    fn parse_fenced(
        error: &RedisError,
        key: &str,
        expected: ShardEpoch,
    ) -> Option<IndexedStorageError> {
        let mut parts = error
            .details()
            .split_whitespace()
            .skip_while(|part| *part != "FENCED");
        parts.next()?;
        let actual = match parts.next()? {
            "-" => None,
            epoch => Some(ShardEpoch(epoch.parse::<u64>().ok()?)),
        };
        let writer_conflict = parts.next()? == "1";
        Some(IndexedStorageError::Fenced {
            key: key.to_string(),
            expected,
            actual,
            writer_conflict,
        })
    }

    /// The error of [`IndexedStorage::set_key_epoch`] or [`IndexedStorage::delete_with_epoch`]:
    /// the scripts' fence, or else classified as a read is. A lost connection or a timeout is
    /// `Transient` even if the script ran, because both repeat safely for the same writer: the
    /// epoch it already holds is accepted again, and a deletion that already happened finds
    /// nothing left and succeeds.
    fn classify_epoch_error(
        error: RedisError,
        key: &str,
        expected: Option<ShardEpoch>,
    ) -> IndexedStorageError {
        expected
            .and_then(|expected| Self::parse_fenced(&error, key, expected))
            .unwrap_or_else(|| Self::classify_read_error(error))
    }

    async fn append_fenced(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: impl Iterator<Item = (u64, Bytes)>,
        expected_epoch: ShardEpoch,
        options: Option<&Options>,
        primary_oplog_insert: bool,
    ) -> Result<(), IndexedStorageError> {
        let mut args = vec![
            Value::from(expected_epoch.0.to_string()),
            Value::from(self.writer_id.to_string()),
        ];
        for (id, value) in pairs {
            args.push(Value::from(id.to_string()));
            args.push(Value::Bytes(value));
        }
        self.redis
            .with(svc_name, api_name)
            .eval(
                Self::FENCED_APPEND_SCRIPT,
                &[
                    Self::composite_key(namespace.clone(), key),
                    Self::epoch_key(namespace.clone(), key),
                ],
                args,
                options,
            )
            .await
            .map(|_| ())
            .map_err(|error| {
                Self::parse_fenced(&error, key, expected_epoch)
                    .unwrap_or_else(|| Self::classify_append_error(error, primary_oplog_insert))
            })
    }

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog {
                agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:oplog:{key}")
            }
            IndexedStorageNamespace::StagedOpLog {
                agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:staged-oplog:{key}")
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_id: _,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:c{level}-oplog:{key}")
            }
        }
    }

    fn composite_meta_key(namespace: IndexedStorageMetaNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:oplog:{key}")
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("worker:{mode}:c{level}-oplog:{key}")
            }
        }
    }

    fn parse_composite_meta_key(namespace: IndexedStorageMetaNamespace, key: &str) -> String {
        let prefix = Self::composite_meta_key(namespace, "");
        if key.starts_with(&prefix) {
            key[prefix.len()..].to_string()
        } else {
            key.to_string()
        }
    }

    fn to_scan_pattern(prefix: Option<&str>) -> String {
        match prefix {
            None => "*".to_string(),
            Some(prefix) => {
                let mut result = String::with_capacity(prefix.len() + 1);
                for ch in prefix.chars() {
                    match ch {
                        '*' | '?' | '[' | ']' | '\\' => {
                            result.push('\\');
                            result.push(ch);
                        }
                        _ => result.push(ch),
                    }
                }
                result.push('*');
                result
            }
        }
    }

    const KEY: &'static str = "key";

    fn parse_entry_id(id: &str) -> Result<u64, IndexedStorageError> {
        if let Some((id, _)) = id.split_once('-') {
            id.parse::<u64>().map_err(|e| {
                IndexedStorageError::Other(format!("Failed to parse {id} as u64: {e}"))
            })
        } else {
            id.parse::<u64>().map_err(|e| {
                IndexedStorageError::Other(format!("Failed to parse {id} as u64: {e}"))
            })
        }
    }

    fn classify_append_error(error: RedisError, primary_oplog_insert: bool) -> IndexedStorageError {
        if primary_oplog_insert
            && error
                .details()
                .contains("ID specified in XADD is equal or smaller than")
        {
            IndexedStorageError::Conflict(error.to_string())
        } else if primary_oplog_insert
            && matches!(
                error.kind(),
                ErrorKind::IO | ErrorKind::Timeout | ErrorKind::Canceled
            )
        {
            IndexedStorageError::Indeterminate(error.to_string())
        } else {
            IndexedStorageError::Other(error.to_string())
        }
    }

    fn classify_read_error(error: RedisError) -> IndexedStorageError {
        if matches!(
            error.kind(),
            ErrorKind::IO | ErrorKind::Timeout | ErrorKind::Canceled
        ) {
            IndexedStorageError::Transient(error.to_string())
        } else {
            IndexedStorageError::Other(error.to_string())
        }
    }

    fn process_stream(
        &self,
        svc_name: &'static str,
        entity_name: &'static str,
        items: Vec<HashMap<String, HashMap<String, Bytes>>>,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        let mut result = Vec::new();
        for item in items {
            for (id, value) in item {
                let id = Self::parse_entry_id(&id)?;
                for (key, value) in value {
                    if key == Self::KEY {
                        record_redis_deserialized_size(svc_name, entity_name, value.len());
                        result.push((id, value.to_vec()));
                    }
                }
            }
        }
        Ok(result)
    }
}

#[async_trait]
impl IndexedStorage for RedisIndexedStorage {
    async fn number_of_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> Result<u8, IndexedStorageError> {
        self.redis
            .with(svc_name, api_name)
            .info_connected_slaves()
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))
    }

    async fn wait_for_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        self.redis
            .with(svc_name, api_name)
            .wait(replicas as i64, timeout.as_millis() as i64)
            .await
            .map(|r| r as u8)
            .map_err(|e| IndexedStorageError::Other(e.to_string()))
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        self.redis
            .with(svc_name, api_name)
            .exists(Self::composite_key(namespace, key))
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))
    }

    async fn scan_stable(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
        // A Redis SCAN cursor walks the hash space, so deleting keys behind it moves nothing, and
        // a key present for the whole iteration comes back at least once.
        let cursor = resume
            .map(|resume| resume.into_cursor("Redis"))
            .transpose()?
            .unwrap_or(0);
        let pattern = Self::to_scan_pattern(prefix);
        let (next, keys) = self
            .redis
            .with(svc_name, api_name)
            .scan(
                Self::composite_meta_key(namespace.clone(), &pattern),
                cursor,
                count,
            )
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))?;
        let keys = keys
            .into_iter()
            .map(|key| Self::parse_composite_meta_key(namespace.clone(), &key))
            .collect();
        let next = (next != 0).then_some(ScanResume::Cursor(next));
        Ok((next, keys))
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        record_redis_serialized_size(svc_name, entity_name, value.len());
        let primary_oplog_insert = matches!(
            &namespace,
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. }
        );
        let options = primary_oplog_insert.then_some(Options {
            max_attempts: Some(1),
            ..Default::default()
        });

        if let Some(expected_epoch) = expected_epoch {
            return self
                .append_fenced(
                    svc_name,
                    api_name,
                    &namespace,
                    key,
                    std::iter::once((id, Bytes::from(value))),
                    expected_epoch,
                    options.as_ref(),
                    primary_oplog_insert,
                )
                .await;
        }

        let _: String = self
            .redis
            .with(svc_name, api_name)
            .xadd(
                Self::composite_key(namespace, key),
                false,
                None,
                id.to_string(),
                (Key::from(Self::KEY), Value::Bytes(Bytes::from(value))),
                options.as_ref(),
            )
            .await
            .map_err(|error| Self::classify_append_error(error, primary_oplog_insert))?;
        Ok(())
    }

    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        if !pairs.is_empty() {
            let primary_oplog_insert = matches!(
                namespace,
                IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. }
            );
            let options = primary_oplog_insert.then_some(Options {
                max_attempts: Some(1),
                ..Default::default()
            });

            if let Some(expected_epoch) = expected_epoch {
                for (_, value) in pairs.iter() {
                    record_redis_serialized_size(svc_name, entity_name, value.len());
                }
                return self
                    .append_fenced(
                        svc_name,
                        api_name,
                        namespace,
                        key,
                        pairs.iter().cloned(),
                        expected_epoch,
                        options.as_ref(),
                        primary_oplog_insert,
                    )
                    .await;
            }
            let mut redis_pairs = Vec::with_capacity(pairs.len());
            for (id, value) in pairs.iter() {
                record_redis_serialized_size(svc_name, entity_name, value.len());
                redis_pairs.push((
                    id.to_string(),
                    (Key::from(Self::KEY), Value::Bytes(value.clone())),
                ));
            }

            self.redis
                .with(svc_name, api_name)
                .xadd_pipeline(
                    Self::composite_key((*namespace).clone(), key),
                    false,
                    None,
                    redis_pairs,
                    options.as_ref(),
                )
                .await
                .map_err(|error| Self::classify_append_error(error, primary_oplog_insert))?;
        }
        Ok(())
    }

    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        self.redis
            .with(svc_name, api_name)
            .eval(
                Self::SET_KEY_EPOCH_SCRIPT,
                &[Self::epoch_key(namespace, key)],
                vec![
                    Value::from(epoch.0.to_string()),
                    Value::from(self.writer_id.to_string()),
                ],
                None,
            )
            .await
            .map(|_| ())
            .map_err(|error| Self::classify_epoch_error(error, key, Some(epoch)))
    }

    async fn delete_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let args = match expected_epoch {
            Some(expected) => vec![
                Value::from(expected.0.to_string()),
                Value::from(self.writer_id.to_string()),
            ],
            None => vec![],
        };
        self.redis
            .with(svc_name, api_name)
            .eval(
                Self::DELETE_WITH_EPOCH_SCRIPT,
                &[
                    Self::composite_key(namespace.clone(), key),
                    Self::epoch_key(namespace, key),
                ],
                args,
                None,
            )
            .await
            .map(|_| ())
            .map_err(|error| Self::classify_epoch_error(error, key, expected_epoch))
    }

    async fn move_if_absent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        source_namespace: IndexedStorageNamespace,
        source_key: &str,
        target_namespace: IndexedStorageNamespace,
        target_key: &str,
        expected_last_id: u64,
    ) -> Result<bool, IndexedStorageError> {
        let source = Self::composite_key(source_namespace, source_key);
        let target = Self::composite_key(target_namespace, target_key);
        match self
            .redis
            .with(svc_name, api_name)
            .move_stream_if_absent(source, target, expected_last_id)
            .await
            .map_err(|error| Self::classify_append_error(error, true))?
        {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(IndexedStorageError::Other(
                "source index is missing, empty, gapped, or has an unexpected tip".to_string(),
            )),
        }
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        self.redis
            .with(svc_name, api_name)
            .xlen(Self::composite_key(namespace, key))
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        self.redis
            .with(svc_name, api_name)
            .del(Self::composite_key(namespace, key))
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))
    }

    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), start_id, end_id, None)
            .await
            .map_err(Self::classify_read_error)?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result)
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), "-", "+", Some(1))
            .await
            .map_err(Self::classify_read_error)?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result.into_iter().next())
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrevrange(Self::composite_key(namespace, key), "+", "-", Some(1))
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result.into_iter().next())
    }

    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        // Streams have no id-only read, so this reads the payload too.
        Ok(self
            .last(svc_name, api_name, entity_name, namespace, key)
            .await?
            .map(|(id, _)| id))
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), id, "+", Some(1))
            .await
            .map_err(Self::classify_read_error)?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result.into_iter().next())
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), IndexedStorageError> {
        let _: u64 = self
            .redis
            .with(svc_name, api_name)
            .xtrim(
                Self::composite_key(namespace, key),
                (XCapKind::MinID, last_dropped_id + 1),
            )
            .await
            .map_err(|e| IndexedStorageError::Other(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn a_fenced_reply_carries_the_stored_epoch_and_the_writer_conflict() {
        let fenced = |details: &'static str| {
            RedisIndexedStorage::parse_fenced(
                &RedisError::new(ErrorKind::Unknown, details),
                "k",
                ShardEpoch(7),
            )
        };

        assert!(matches!(
            fenced("FENCED 9 0"),
            Some(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(9)),
                writer_conflict: false,
                ..
            })
        ));
        assert!(matches!(
            fenced("FENCED 7 1"),
            Some(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(7)),
                writer_conflict: true,
                ..
            })
        ));
        assert!(matches!(
            fenced("FENCED - 0"),
            Some(IndexedStorageError::Fenced {
                actual: None,
                writer_conflict: false,
                ..
            })
        ));
        // An error raised by a command inside the script is not a fence.
        assert!(
            fenced(
                "ERR The ID specified in XADD is equal or smaller than the target stream top item"
            )
            .is_none()
        );
    }

    #[test]
    fn primary_oplog_xadd_ordering_error_is_a_conflict() {
        let error = RedisError::new(
            ErrorKind::Unknown,
            "ERR The ID specified in XADD is equal or smaller than the target stream top item",
        );

        assert!(matches!(
            RedisIndexedStorage::classify_append_error(error, true),
            IndexedStorageError::Conflict(_)
        ));
    }

    #[test]
    fn compressed_oplog_xadd_ordering_error_remains_permanent() {
        let error = RedisError::new(
            ErrorKind::Unknown,
            "ERR The ID specified in XADD is equal or smaller than the target stream top item",
        );

        assert!(matches!(
            RedisIndexedStorage::classify_append_error(error, false),
            IndexedStorageError::Other(_)
        ));
    }

    #[test]
    fn primary_oplog_xadd_io_error_is_indeterminate() {
        let error = RedisError::new(ErrorKind::IO, "connection lost after sending XADD");

        assert!(matches!(
            RedisIndexedStorage::classify_append_error(error, true),
            IndexedStorageError::Indeterminate(_)
        ));
    }

    #[test]
    fn primary_oplog_xadd_timeout_and_cancellation_are_indeterminate() {
        for kind in [ErrorKind::Timeout, ErrorKind::Canceled] {
            let error = RedisError::new(kind, "outcome is unknown");
            assert!(matches!(
                RedisIndexedStorage::classify_append_error(error, true),
                IndexedStorageError::Indeterminate(_)
            ));
        }
    }

    #[test]
    fn xrange_connection_errors_are_transient() {
        for kind in [ErrorKind::IO, ErrorKind::Timeout, ErrorKind::Canceled] {
            let error = RedisError::new(kind, "read can be retried");
            assert!(matches!(
                RedisIndexedStorage::classify_read_error(error),
                IndexedStorageError::Transient(_)
            ));
        }
    }

    // The oplog retries only `Transient` around these two and panics on anything else but a
    // fence, which SQLite and PostgreSQL never hand it for a lost connection.
    #[test]
    fn epoch_record_connection_errors_are_transient() {
        for expected in [Some(ShardEpoch(7)), None] {
            for kind in [ErrorKind::IO, ErrorKind::Timeout, ErrorKind::Canceled] {
                let error = RedisError::new(kind, "outcome is unknown");
                assert!(matches!(
                    RedisIndexedStorage::classify_epoch_error(error, "k", expected),
                    IndexedStorageError::Transient(_)
                ));
            }
        }
    }

    #[test]
    fn epoch_record_fence_is_a_fence_and_other_errors_stay_permanent() {
        let fenced = RedisError::new(ErrorKind::Unknown, "FENCED 9 0");
        assert!(matches!(
            RedisIndexedStorage::classify_epoch_error(fenced, "k", Some(ShardEpoch(7))),
            IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(9)),
                ..
            }
        ));

        let wrong_type = RedisError::new(
            ErrorKind::Unknown,
            "WRONGTYPE Operation against a key holding the wrong kind of value",
        );
        assert!(matches!(
            RedisIndexedStorage::classify_epoch_error(wrong_type, "k", Some(ShardEpoch(7))),
            IndexedStorageError::Other(_)
        ));
    }
}
