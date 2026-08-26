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

use crate::durable_host::authorization::targets::{kv_bucket_target, kv_target};
use crate::durable_host::concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::keyvalue::{denial, environment_owner};
use crate::durable_host::{DurableWorkerCtx, HostFailureKind, InternalRetryResult};
use crate::metrics::storage::{
    STORAGE_TYPE_KV, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::preview2::wasi::keyvalue::eventual_batch::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::card::KvVerb;
use golem_common::model::oplog::host_functions::{
    KeyvalueEventualBatchDeleteMany, KeyvalueEventualBatchGetKeys, KeyvalueEventualBatchGetMany,
    KeyvalueEventualBatchSetMany,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestKVBucket, HostRequestKVBucketAndKeySizePairs,
    HostRequestKVBucketAndKeys, HostResponseKVGetMany, HostResponseKVKeys, HostResponseKVUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::{IoView, ResourceTableError};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Vec<Key>,
    ) -> anyhow::Result<Result<Vec<Option<Resource<IncomingValue>>>, Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let is_live = self.state.is_live();
        let denied = if is_live {
            let owner = environment_owner(self);
            let targets = keys
                .iter()
                .map(|key| kv_target(owner.clone(), KvVerb::Read, &bucket, key))
                .collect::<Result<Vec<_>, _>>();
            match targets {
                Ok(targets) => self
                    .authorize_live_permissions(&targets)
                    .await?
                    .err()
                    .map(denial),
                Err(error) => Some(denial(error)),
            }
        } else {
            None
        };
        let begun = DurableCallSession::<KeyvalueEventualBatchGetMany, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let mut handle = if begun.is_live() {
            begun
                .start_live(
                    self,
                    HostRequestKVBucketAndKeys {
                        bucket: bucket.clone(),
                        keys: keys.clone(),
                    },
                )
                .await?
        } else {
            begun.start_replay(self).await?
        };
        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            if let Some(error) = denied {
                break 'resp handle
                    .complete(self, HostResponseKVGetMany { result: Err(error) })
                    .await?;
            }

            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .get_many(environment_id, bucket.clone(), keys.clone())
                    .await
                    .map_err(|err| err.to_string());
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle
                .complete(self, HostResponseKVGetMany { result })
                .await?
        };

        match result.result {
            Ok(values) => {
                let mut result = Vec::new();
                for maybe_incoming_value in values {
                    match maybe_incoming_value {
                        Some(incoming_value) => {
                            let value = self
                                .as_wasi_view()
                                .table()
                                .push(IncomingValueEntry::new(incoming_value))?;
                            result.push(Some(value));
                        }
                        None => {
                            result.push(None);
                        }
                    }
                }
                Ok(Ok(result))
            }
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn keys(
        &mut self,
        bucket: Resource<Bucket>,
    ) -> anyhow::Result<Result<Vec<Key>, Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let is_live = self.state.is_live();
        let denied = if is_live {
            let target = kv_bucket_target(environment_owner(self), KvVerb::List, &bucket);
            match target {
                Ok(target) => self
                    .authorize_live_permission(&target)
                    .await?
                    .err()
                    .map(denial),
                Err(error) => Some(denial(error)),
            }
        } else {
            None
        };
        let begun = DurableCallSession::<KeyvalueEventualBatchGetKeys, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let mut handle = if begun.is_live() {
            begun
                .start_live(
                    self,
                    HostRequestKVBucket {
                        bucket: bucket.clone(),
                    },
                )
                .await?
        } else {
            begun.start_replay(self).await?
        };
        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            if let Some(error) = denied {
                break 'resp handle
                    .complete(self, HostResponseKVKeys { result: Err(error) })
                    .await?;
            }

            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .get_keys(environment_id, bucket.clone())
                    .await
                    .map_err(|err| err.to_string());
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle.complete(self, HostResponseKVKeys { result }).await?
        };

        match result.result {
            Ok(keys) => Ok(Ok(keys)),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn set_many(
        &mut self,
        bucket: Resource<Bucket>,
        key_values: Vec<(Key, Resource<OutgoingValue>)>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let begun = DurableCallSession::<KeyvalueEventualBatchSetMany, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            let (mut handle, environment_id, bucket, denied, key_values, total_bytes, count) =
                if begun.is_live() {
                    let environment_id = self.owned_agent_id.environment_id();
                    let bucket = self
                        .as_wasi_view()
                        .table()
                        .get::<BucketEntry>(&bucket)?
                        .name
                        .clone();
                    let owner = environment_owner(self);
                    let targets = key_values
                        .iter()
                        .map(|(key, _)| kv_target(owner.clone(), KvVerb::Write, &bucket, key))
                        .collect::<Result<Vec<_>, _>>();
                    let denied = match targets {
                        Ok(targets) => self
                            .authorize_live_permissions(&targets)
                            .await?
                            .err()
                            .map(denial),
                        Err(error) => Some(denial(error)),
                    };
                    let key_values = if denied.is_some() {
                        key_values
                            .into_iter()
                            .map(|(key, _)| (key, Vec::new()))
                            .collect()
                    } else {
                        key_values
                            .into_iter()
                            .map(|(key, outgoing_value)| {
                                let value = self
                                    .as_wasi_view()
                                    .table()
                                    .get::<OutgoingValueEntry>(&outgoing_value)?
                                    .body
                                    .read()
                                    .unwrap()
                                    .clone();
                                Ok((key, value))
                            })
                            .collect::<Result<Vec<(String, Vec<u8>)>, ResourceTableError>>()?
                    };
                    let total_bytes = key_values.iter().map(|(_, v)| v.len() as u64).sum();
                    let count = key_values.len() as u64;
                    let request = HostRequestKVBucketAndKeySizePairs {
                        bucket: bucket.clone(),
                        keys: key_values
                            .iter()
                            .map(|(k, v)| (k.clone(), v.len()))
                            .collect(),
                    };
                    (
                        begun.start_live(self, request).await?,
                        environment_id,
                        bucket,
                        denied,
                        key_values,
                        total_bytes,
                        count,
                    )
                } else {
                    let mut handle = begun.start_replay(self).await?;
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                    let environment_id = self.owned_agent_id.environment_id();
                    let bucket = self
                        .as_wasi_view()
                        .table()
                        .get::<BucketEntry>(&bucket)?
                        .name
                        .clone();
                    let denied = None;
                    let key_values = key_values
                        .into_iter()
                        .map(|(key, outgoing_value)| {
                            let value = self
                                .as_wasi_view()
                                .table()
                                .get::<OutgoingValueEntry>(&outgoing_value)?
                                .body
                                .read()
                                .unwrap()
                                .clone();
                            Ok((key, value))
                        })
                        .collect::<Result<Vec<(String, Vec<u8>)>, ResourceTableError>>()?;
                    let total_bytes = key_values.iter().map(|(_, v)| v.len() as u64).sum();
                    let count = key_values.len() as u64;
                    (
                        handle,
                        environment_id,
                        bucket,
                        denied,
                        key_values,
                        total_bytes,
                        count,
                    )
                };

            if let Some(error) = denied {
                break 'resp handle
                    .complete(self, HostResponseKVUnit { result: Err(error) })
                    .await?;
            }

            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .set_many(environment_id, bucket.clone(), key_values.clone())
                    .await
                    .map_err(|err| err.to_string());
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_bytes_written(
                    STORAGE_TYPE_KV,
                    &account_id,
                    &environment_id_str,
                    total_bytes,
                );
                record_storage_objects_written(
                    STORAGE_TYPE_KV,
                    &account_id,
                    &environment_id_str,
                    count,
                );
            }
            handle.complete(self, HostResponseKVUnit { result }).await?
        };

        match result.result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn delete_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Vec<Key>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let count = keys.len() as u64;
        let is_live = self.state.is_live();
        let denied = if is_live {
            let owner = environment_owner(self);
            let targets = keys
                .iter()
                .map(|key| kv_target(owner.clone(), KvVerb::Delete, &bucket, key))
                .collect::<Result<Vec<_>, _>>();
            match targets {
                Ok(targets) => self
                    .authorize_live_permissions(&targets)
                    .await?
                    .err()
                    .map(denial),
                Err(error) => Some(denial(error)),
            }
        } else {
            None
        };
        let begun = DurableCallSession::<KeyvalueEventualBatchDeleteMany, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let mut handle = if begun.is_live() {
            begun
                .start_live(
                    self,
                    HostRequestKVBucketAndKeys {
                        bucket: bucket.clone(),
                        keys: keys.clone(),
                    },
                )
                .await?
        } else {
            begun.start_replay(self).await?
        };

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            if let Some(error) = denied {
                break 'resp handle
                    .complete(self, HostResponseKVUnit { result: Err(error) })
                    .await?;
            }

            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .delete_many(environment_id, bucket.clone(), keys.clone())
                    .await
                    .map_err(|err| err.to_string());
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_objects_deleted(
                    STORAGE_TYPE_KV,
                    &account_id,
                    &environment_id_str,
                    count,
                );
            }
            handle.complete(self, HostResponseKVUnit { result }).await?
        };

        match result.result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }
}
