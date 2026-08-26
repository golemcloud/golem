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

use golem_common::model::card::KvVerb;
use golem_common::model::oplog::host_functions::{
    KeyvalueEventualDelete, KeyvalueEventualExists, KeyvalueEventualGet, KeyvalueEventualSet,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestKVBucketAndKey, HostRequestKVBucketKeyAndSize,
    HostResponseKVDelete, HostResponseKVGet, HostResponseKVUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::durable_host::authorization::targets::kv_target;
use crate::durable_host::concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::keyvalue::{denial, environment_owner};
use crate::durable_host::{DurableWorkerCtx, HostFailureKind, InternalRetryResult};
use crate::metrics::storage::{
    STORAGE_TYPE_KV, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::preview2::wasi::keyvalue::eventual::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<Option<Resource<IncomingValue>>, Resource<Error>>> {
        let begun = DurableCallSession::<KeyvalueEventualGet, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            let (mut handle, environment_id, bucket, denied) = if begun.is_live() {
                let environment_id = self.owned_agent_id.environment_id();
                let bucket = self
                    .as_wasi_view()
                    .table()
                    .get::<BucketEntry>(&bucket)?
                    .name
                    .clone();
                let target = kv_target(environment_owner(self), KvVerb::Read, &bucket, &key);
                let denied = match target {
                    Ok(target) => self
                        .authorize_live_permission(&target)
                        .await?
                        .err()
                        .map(denial),
                    Err(error) => Some(denial(error)),
                };
                let request = HostRequestKVBucketAndKey {
                    bucket: bucket.clone(),
                    key: key.clone(),
                };
                (
                    begun.start_live(self, request).await?,
                    environment_id,
                    bucket,
                    denied,
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
                (handle, environment_id, bucket, None)
            };

            if let Some(error) = denied {
                break 'resp handle
                    .complete(self, HostResponseKVGet { result: Err(error) })
                    .await?;
            }

            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .get(environment_id, bucket.clone(), key.clone())
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
            handle.complete(self, HostResponseKVGet { result }).await?
        };

        match result.result {
            Ok(Some(value)) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table()
                    .push(IncomingValueEntry::new(value))?;
                Ok(Ok(Some(incoming_value)))
            }
            Ok(None) => Ok(Ok(None)),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
        outgoing_value: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let begun = DurableCallSession::<KeyvalueEventualSet, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            let (mut handle, environment_id, bucket, denied, outgoing_value, length) =
                if begun.is_live() {
                    let environment_id = self.owned_agent_id.environment_id();
                    let bucket = self
                        .as_wasi_view()
                        .table()
                        .get::<BucketEntry>(&bucket)?
                        .name
                        .clone();
                    let target = kv_target(environment_owner(self), KvVerb::Write, &bucket, &key);
                    let denied = match target {
                        Ok(target) => self
                            .authorize_live_permission(&target)
                            .await?
                            .err()
                            .map(denial),
                        Err(error) => Some(denial(error)),
                    };
                    let outgoing_value = if denied.is_none() {
                        self.as_wasi_view()
                            .table()
                            .get::<OutgoingValueEntry>(&outgoing_value)?
                            .body
                            .read()
                            .unwrap()
                            .clone()
                    } else {
                        Vec::new()
                    };
                    let length = outgoing_value.len() as u64;
                    let request = HostRequestKVBucketKeyAndSize {
                        bucket: bucket.clone(),
                        key: key.clone(),
                        length: outgoing_value.len(),
                    };
                    (
                        begun.start_live(self, request).await?,
                        environment_id,
                        bucket,
                        denied,
                        outgoing_value,
                        length,
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
                    let outgoing_value = self
                        .as_wasi_view()
                        .table()
                        .get::<OutgoingValueEntry>(&outgoing_value)?
                        .body
                        .read()
                        .unwrap()
                        .clone();
                    let length = outgoing_value.len() as u64;
                    (
                        handle,
                        environment_id,
                        bucket,
                        denied,
                        outgoing_value,
                        length,
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
                    .set(
                        environment_id,
                        bucket.clone(),
                        key.clone(),
                        outgoing_value.clone(),
                    )
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
                    length,
                );
                record_storage_objects_written(
                    STORAGE_TYPE_KV,
                    &account_id,
                    &environment_id_str,
                    1,
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

    async fn delete(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let begun = DurableCallSession::<KeyvalueEventualDelete, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            let (mut handle, environment_id, bucket, denied) = if begun.is_live() {
                let environment_id = self.owned_agent_id.environment_id();
                let bucket = self
                    .as_wasi_view()
                    .table()
                    .get::<BucketEntry>(&bucket)?
                    .name
                    .clone();
                let target = kv_target(environment_owner(self), KvVerb::Delete, &bucket, &key);
                let denied = match target {
                    Ok(target) => self
                        .authorize_live_permission(&target)
                        .await?
                        .err()
                        .map(denial),
                    Err(error) => Some(denial(error)),
                };
                let request = HostRequestKVBucketAndKey {
                    bucket: bucket.clone(),
                    key: key.clone(),
                };
                (
                    begun.start_live(self, request).await?,
                    environment_id,
                    bucket,
                    denied,
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
                (handle, environment_id, bucket, None)
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
                    .delete(environment_id, bucket.clone(), key.clone())
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
                    1,
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

    async fn exists(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        let begun = DurableCallSession::<KeyvalueEventualExists, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            let (mut handle, environment_id, bucket, denied) = if begun.is_live() {
                let environment_id = self.owned_agent_id.environment_id();
                let bucket = self
                    .as_wasi_view()
                    .table()
                    .get::<BucketEntry>(&bucket)?
                    .name
                    .clone();
                let target = kv_target(environment_owner(self), KvVerb::Read, &bucket, &key);
                let denied = match target {
                    Ok(target) => self
                        .authorize_live_permission(&target)
                        .await?
                        .err()
                        .map(denial),
                    Err(error) => Some(denial(error)),
                };
                let request = HostRequestKVBucketAndKey {
                    bucket: bucket.clone(),
                    key: key.clone(),
                };
                (
                    begun.start_live(self, request).await?,
                    environment_id,
                    bucket,
                    denied,
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
                (handle, environment_id, bucket, None)
            };

            if let Some(error) = denied {
                break 'resp handle
                    .complete(self, HostResponseKVDelete { result: Err(error) })
                    .await?;
            }

            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .exists(environment_id, bucket.clone(), key.clone())
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
                .complete(self, HostResponseKVDelete { result })
                .await?
        };

        match result.result {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table()
                    .push(ErrorEntry::new(format!("{e:?}")))?;
                Ok(Err(error))
            }
        }
    }
}
