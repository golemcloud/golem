// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::durable_host::DurableWorkerCtx;
use crate::error::GolemError;
use crate::metrics::wasm::record_host_function_call;
use crate::model::PersistenceLevel;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::workerctx::WorkerCtx;
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, WrappedFunctionType};
use golem_common::serialization::try_deserialize;
use std::fmt::Debug;
use std::marker::PhantomData;
use tracing::error;

pub enum OplogEntryVersion {
    V1,
    V2,
}

pub struct Durability<Ctx, SOk, SErr> {
    package: &'static str,
    function: &'static str,
    function_type: WrappedFunctionType,
    begin_index: OplogIndex,
    is_live: bool,
    persistence_level: PersistenceLevel,
    _ctx: PhantomData<Ctx>,
    _sok: PhantomData<SOk>,
    _serr: PhantomData<SErr>,
}

impl<Ctx: WorkerCtx, SOk, SErr> Durability<Ctx, SOk, SErr> {
    pub async fn new(
        ctx: &mut DurableWorkerCtx<Ctx>,
        package: &'static str,
        function: &'static str,
        function_type: WrappedFunctionType,
    ) -> Result<Self, GolemError> {
        record_host_function_call(package, function);

        let begin_index = ctx.state.begin_function(&function_type).await?;

        Ok(Self {
            package,
            function,
            function_type,
            begin_index,
            is_live: ctx.state.is_live(),
            persistence_level: ctx.state.persistence_level.clone(),
            _ctx: PhantomData,
            _sok: PhantomData,
            _serr: PhantomData,
        })
    }

    pub fn is_live(&self) -> bool {
        self.is_live || self.persistence_level == PersistenceLevel::PersistNothing
    }

    pub async fn persist<SIn, Ok, Err>(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        input: SIn,
        result: Result<Ok, Err>,
    ) -> Result<Ok, Err>
    where
        Ok: Clone,
        Err: From<SErr> + From<GolemError> + Send + Sync,
        SIn: Debug + Encode + Send + Sync,
        SErr: Debug + Encode + for<'a> From<&'a Err> + From<GolemError> + Send + Sync,
        SOk: Debug + Encode + From<Ok> + Send + Sync,
    {
        let serializable_result: Result<SOk, SErr> = result
            .as_ref()
            .map(|result| result.clone().into())
            .map_err(|err| err.into());

        self.persist_serializable(ctx, input, serializable_result)
            .await
            .map_err(|err| {
                let err: SErr = err.into();
                let err: Err = err.into();
                err
            })?;
        result
    }

    pub async fn persist_serializable<SIn>(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        input: SIn,
        serializable_result: Result<SOk, SErr>,
    ) -> Result<(), GolemError>
    where
        SIn: Debug + Encode + Send + Sync,
        SOk: Debug + Encode + Send + Sync,
        SErr: Debug + Encode + Send + Sync,
    {
        let function_name = self.function_name();
        if ctx.state.persistence_level != PersistenceLevel::PersistNothing {
            ctx.state
                .oplog
                .add_imported_function_invoked(
                    function_name.to_string(),
                    &input,
                    &serializable_result,
                    self.function_type.clone(),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to serialize and store function request ({:?}) and response ({:?}): {err}",
                        input,
                        serializable_result
                    )
                });
            ctx.state
                .end_function(&self.function_type, self.begin_index)
                .await?;
            if self.function_type == WrappedFunctionType::WriteRemote
                || matches!(
                    self.function_type,
                    WrappedFunctionType::WriteRemoteBatched(_)
                )
            {
                ctx.state.oplog.commit(CommitLevel::DurableOnly).await;
            }
        }
        Ok(())
    }

    pub async fn replay_raw(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(Bytes, OplogEntryVersion), GolemError> {
        let (_, oplog_entry) = crate::get_oplog_entry!(
            ctx.state.replay_state,
            OplogEntry::ImportedFunctionInvoked,
            OplogEntry::ImportedFunctionInvokedV1
        )?;

        let version = if matches!(oplog_entry, OplogEntry::ImportedFunctionInvokedV1 { .. }) {
            OplogEntryVersion::V1
        } else {
            OplogEntryVersion::V2
        };

        let function_name = self.function_name();
        Self::validate_oplog_entry(&oplog_entry, &function_name)?;

        let bytes = ctx
            .state
            .oplog
            .get_raw_payload_of_entry(&oplog_entry)
            .await
            .map_err(|err| {
                GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
            })?
            .unwrap();

        ctx.state
            .end_function(&self.function_type, self.begin_index)
            .await?;

        Ok((bytes, version))
    }

    pub async fn replay_serializable(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Result<SOk, SErr>, GolemError>
    where
        SOk: Decode,
        SErr: Decode,
    {
        let (bytes, _) = self.replay_raw(ctx).await?;
        let result: Result<SOk, SErr> = try_deserialize(&bytes)
            .map_err(|err| {
                GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
            })?
            .expect("Payload is empty");
        Ok(result)
    }

    pub async fn replay<Ok, Err>(&self, ctx: &mut DurableWorkerCtx<Ctx>) -> Result<Ok, Err>
    where
        Ok: From<SOk>,
        Err: From<SErr> + From<GolemError>,
        SErr: Debug + Encode + Decode + From<GolemError> + Send + Sync,
        SOk: Debug + Encode + Decode + Send + Sync,
    {
        Self::replay_serializable(self, ctx)
            .await?
            .map(|sok| sok.into())
            .map_err(|serr| serr.into())
    }

    fn function_name(&self) -> String {
        if self.package.is_empty() {
            // For backward compatibility - some of the recorded function names were not following the pattern
            self.function.to_string()
        } else {
            format!("{}::{}", self.package, self.function)
        }
    }

    fn validate_oplog_entry(
        oplog_entry: &OplogEntry,
        expected_function_name: &str,
    ) -> Result<(), GolemError> {
        if let OplogEntry::ImportedFunctionInvoked { function_name, .. } = oplog_entry {
            if function_name != expected_function_name {
                error!(
                    "Unexpected imported function call entry in oplog: expected {}, got {}",
                    expected_function_name, function_name
                );
                Err(GolemError::unexpected_oplog_entry(
                    expected_function_name,
                    function_name,
                ))
            } else {
                Ok(())
            }
        } else if let OplogEntry::ImportedFunctionInvokedV1 { function_name, .. } = oplog_entry {
            if function_name != expected_function_name {
                error!(
                    "Unexpected imported function call entry in oplog: expected {}, got {}",
                    expected_function_name, function_name
                );
                Err(GolemError::unexpected_oplog_entry(
                    expected_function_name,
                    function_name,
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}
