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

use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::worker::OplogEntryWithIndex;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::public_oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::{AccountId, PluginInstallationId};
use golem_common::model::{
    ComponentFilePermissions, ComponentFileSystemNode, ComponentFileSystemNodeDetails, ComponentId,
    ComponentType, ComponentVersion, InitialComponentFile, PromiseId, ScanCursor, ShardId,
    Timestamp, WorkerFilter, WorkerId, WorkerStatus,
};
use golem_common::SafeDisplay;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use poem_openapi::{Enum, NewType, Object, Union};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::SystemTime;
use std::{collections::HashMap, fmt::Display, fmt::Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct WorkerCreationRequest {
    pub name: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerCreationResponse {
    pub worker_id: WorkerId,
    pub component_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, NewType)]
pub struct ComponentName(pub String);

impl Display for ComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    Object,
    Encode,
    Decode,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct VersionedComponentId {
    pub component_id: ComponentId,
    pub version: ComponentVersion,
}

impl TryFrom<golem_api_grpc::proto::golem::component::VersionedComponentId>
    for VersionedComponentId
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::VersionedComponentId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing component_id")?
                .try_into()?,
            version: value.version,
        })
    }
}

impl From<VersionedComponentId> for golem_api_grpc::proto::golem::component::VersionedComponentId {
    fn from(value: VersionedComponentId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            version: value.version,
        }
    }
}

impl std::fmt::Display for VersionedComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.component_id, self.version)
    }
}

pub fn validate_worker_name(name: &str) -> Result<(), &'static str> {
    let length = name.len();
    if !(1..=100).contains(&length) {
        Err("Worker name must be between 1 and 100 characters")
    } else if name.contains(' ') {
        Err("Worker name must not contain spaces")
    } else if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        Err("Worker name must contain only alphanumeric characters, underscores, and dashes")
    } else if name.starts_with('-') {
        Err("Worker name must not start with a dash")
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CompleteParameters {
    pub oplog_idx: u64,
    pub data: Vec<u8>,
}

impl From<CompleteParameters> for golem_api_grpc::proto::golem::worker::CompleteParameters {
    fn from(value: CompleteParameters) -> Self {
        Self {
            oplog_idx: value.oplog_idx,
            data: value.data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Invalid request: {details}")]
pub struct GolemErrorInvalidRequest {
    pub details: String,
}

impl SafeDisplay for GolemErrorInvalidRequest {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::InvalidRequest> for GolemErrorInvalidRequest {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::InvalidRequest) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorInvalidRequest> for golem_api_grpc::proto::golem::worker::v1::InvalidRequest {
    fn from(value: GolemErrorInvalidRequest) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker already exists: {worker_id}")]
pub struct GolemErrorWorkerAlreadyExists {
    pub worker_id: WorkerId,
}

impl SafeDisplay for GolemErrorWorkerAlreadyExists {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerAlreadyExists>
    for GolemErrorWorkerAlreadyExists
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::WorkerAlreadyExists,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorWorkerAlreadyExists>
    for golem_api_grpc::proto::golem::worker::v1::WorkerAlreadyExists
{
    fn from(value: GolemErrorWorkerAlreadyExists) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker not found: {worker_id}")]
pub struct GolemErrorWorkerNotFound {
    pub worker_id: WorkerId,
}

impl SafeDisplay for GolemErrorWorkerNotFound {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerNotFound>
    for GolemErrorWorkerNotFound
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::WorkerNotFound,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorWorkerNotFound> for golem_api_grpc::proto::golem::worker::v1::WorkerNotFound {
    fn from(value: GolemErrorWorkerNotFound) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker creation failed {worker_id}: {details}")]
pub struct GolemErrorWorkerCreationFailed {
    pub worker_id: WorkerId,
    pub details: String,
}

impl SafeDisplay for GolemErrorWorkerCreationFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerCreationFailed>
    for GolemErrorWorkerCreationFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::WorkerCreationFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            details: value.details,
        })
    }
}

impl From<GolemErrorWorkerCreationFailed>
    for golem_api_grpc::proto::golem::worker::v1::WorkerCreationFailed
{
    fn from(value: GolemErrorWorkerCreationFailed) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to resume worker: {worker_id}")]
pub struct GolemErrorFailedToResumeWorker {
    pub worker_id: WorkerId,
    pub reason: Box<GolemError>,
}

impl SafeDisplay for GolemErrorFailedToResumeWorker {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::FailedToResumeWorker>
    for GolemErrorFailedToResumeWorker
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::FailedToResumeWorker,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            reason: Box::new((*value.reason.ok_or("Missing field: reason")?).try_into()?),
        })
    }
}

impl From<GolemErrorFailedToResumeWorker>
    for golem_api_grpc::proto::golem::worker::v1::FailedToResumeWorker
{
    fn from(value: GolemErrorFailedToResumeWorker) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            reason: Some(Box::new((*value.reason).into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to download component {component_id}: {reason}")]
pub struct GolemErrorComponentDownloadFailed {
    pub component_id: VersionedComponentId,
    pub reason: String,
}

impl SafeDisplay for GolemErrorComponentDownloadFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::ComponentDownloadFailed>
    for GolemErrorComponentDownloadFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::ComponentDownloadFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: VersionedComponentId {
                component_id: value
                    .component_id
                    .ok_or("Missing field: component_id")?
                    .try_into()?,
                version: value.component_version,
            },
            reason: value.reason,
        })
    }
}

impl From<GolemErrorComponentDownloadFailed>
    for golem_api_grpc::proto::golem::worker::v1::ComponentDownloadFailed
{
    fn from(value: GolemErrorComponentDownloadFailed) -> Self {
        let component_version = value.component_id.version;
        let component_id = golem_api_grpc::proto::golem::component::ComponentId {
            value: Some(value.component_id.component_id.0.into()),
        };
        Self {
            component_id: Some(component_id),
            component_version,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to parse component {component_id}: {reason}")]
pub struct GolemErrorComponentParseFailed {
    pub component_id: VersionedComponentId,
    pub reason: String,
}

impl SafeDisplay for GolemErrorComponentParseFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::ComponentParseFailed>
    for GolemErrorComponentParseFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::ComponentParseFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: VersionedComponentId {
                component_id: value
                    .component_id
                    .ok_or("Missing field: component_id")?
                    .try_into()?,
                version: value.component_version,
            },
            reason: value.reason,
        })
    }
}

impl From<GolemErrorComponentParseFailed>
    for golem_api_grpc::proto::golem::worker::v1::ComponentParseFailed
{
    fn from(value: GolemErrorComponentParseFailed) -> Self {
        let component_version = value.component_id.version;
        let component_id = golem_api_grpc::proto::golem::component::ComponentId {
            value: Some(value.component_id.component_id.0.into()),
        };
        Self {
            component_id: Some(component_id),
            component_version,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to get latest version of component {component_id}: {reason}")]
pub struct GolemErrorGetLatestVersionOfComponentFailed {
    pub component_id: ComponentId,
    pub reason: String,
}

impl SafeDisplay for GolemErrorGetLatestVersionOfComponentFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::GetLatestVersionOfComponentFailed>
    for GolemErrorGetLatestVersionOfComponentFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::GetLatestVersionOfComponentFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing field: component_id")?
                .try_into()?,
            reason: value.reason,
        })
    }
}

impl From<GolemErrorGetLatestVersionOfComponentFailed>
    for golem_api_grpc::proto::golem::worker::v1::GetLatestVersionOfComponentFailed
{
    fn from(value: GolemErrorGetLatestVersionOfComponentFailed) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to find promise: {promise_id}")]
pub struct GolemErrorPromiseNotFound {
    pub promise_id: PromiseId,
}

impl SafeDisplay for GolemErrorPromiseNotFound {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::PromiseNotFound>
    for GolemErrorPromiseNotFound
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::PromiseNotFound,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseNotFound> for golem_api_grpc::proto::golem::worker::v1::PromiseNotFound {
    fn from(value: GolemErrorPromiseNotFound) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Promise dropped: {promise_id}")]
pub struct GolemErrorPromiseDropped {
    pub promise_id: PromiseId,
}

impl SafeDisplay for GolemErrorPromiseDropped {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::PromiseDropped>
    for GolemErrorPromiseDropped
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::PromiseDropped,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseDropped> for golem_api_grpc::proto::golem::worker::v1::PromiseDropped {
    fn from(value: GolemErrorPromiseDropped) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Promise already completed: {promise_id}")]
pub struct GolemErrorPromiseAlreadyCompleted {
    pub promise_id: PromiseId,
}

impl SafeDisplay for GolemErrorPromiseAlreadyCompleted {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::PromiseAlreadyCompleted>
    for GolemErrorPromiseAlreadyCompleted
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::PromiseAlreadyCompleted,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseAlreadyCompleted>
    for golem_api_grpc::proto::golem::worker::v1::PromiseAlreadyCompleted
{
    fn from(value: GolemErrorPromiseAlreadyCompleted) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error(
    "Worker Interrupted: {}", if *.recover_immediately { "recovering immediately" } else { "not recovering immediately" }
)]
pub struct GolemErrorInterrupted {
    pub recover_immediately: bool,
}

impl SafeDisplay for GolemErrorInterrupted {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::Interrupted> for GolemErrorInterrupted {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::Interrupted) -> Self {
        Self {
            recover_immediately: value.recover_immediately,
        }
    }
}

impl From<GolemErrorInterrupted> for golem_api_grpc::proto::golem::worker::v1::Interrupted {
    fn from(value: GolemErrorInterrupted) -> Self {
        Self {
            recover_immediately: value.recover_immediately,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Parameter type mismatch")]
pub struct GolemErrorParamTypeMismatch {
    pub details: String,
}

impl SafeDisplay for GolemErrorParamTypeMismatch {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::ParamTypeMismatch>
    for GolemErrorParamTypeMismatch
{
    fn from(value: golem_api_grpc::proto::golem::worker::v1::ParamTypeMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorParamTypeMismatch>
    for golem_api_grpc::proto::golem::worker::v1::ParamTypeMismatch
{
    fn from(value: GolemErrorParamTypeMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("No value in message")]
pub struct GolemErrorNoValueInMessage {}

impl SafeDisplay for GolemErrorNoValueInMessage {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::NoValueInMessage>
    for GolemErrorNoValueInMessage
{
    fn from(_value: golem_api_grpc::proto::golem::worker::v1::NoValueInMessage) -> Self {
        Self {}
    }
}

impl From<GolemErrorNoValueInMessage>
    for golem_api_grpc::proto::golem::worker::v1::NoValueInMessage
{
    fn from(_value: GolemErrorNoValueInMessage) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Value mismatch: {details}")]
pub struct GolemErrorValueMismatch {
    pub details: String,
}

impl SafeDisplay for GolemErrorValueMismatch {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::ValueMismatch> for GolemErrorValueMismatch {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::ValueMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorValueMismatch> for golem_api_grpc::proto::golem::worker::v1::ValueMismatch {
    fn from(value: GolemErrorValueMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Unexpected oplog entry: expected {expected}, got {got}")]
pub struct GolemErrorUnexpectedOplogEntry {
    pub expected: String,
    pub got: String,
}

impl SafeDisplay for GolemErrorUnexpectedOplogEntry {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::UnexpectedOplogEntry>
    for GolemErrorUnexpectedOplogEntry
{
    fn from(value: golem_api_grpc::proto::golem::worker::v1::UnexpectedOplogEntry) -> Self {
        Self {
            expected: value.expected,
            got: value.got,
        }
    }
}

impl From<GolemErrorUnexpectedOplogEntry>
    for golem_api_grpc::proto::golem::worker::v1::UnexpectedOplogEntry
{
    fn from(value: GolemErrorUnexpectedOplogEntry) -> Self {
        Self {
            expected: value.expected,
            got: value.got,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Runtime error: {details}")]
pub struct GolemErrorRuntimeError {
    pub details: String,
}

impl SafeDisplay for GolemErrorRuntimeError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::RuntimeError> for GolemErrorRuntimeError {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::RuntimeError) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorRuntimeError> for golem_api_grpc::proto::golem::worker::v1::RuntimeError {
    fn from(value: GolemErrorRuntimeError) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Invalid shard id: {shard_id}, valid shard ids: {shard_ids:?}")]
pub struct GolemErrorInvalidShardId {
    pub shard_id: ShardId,
    pub shard_ids: std::collections::HashSet<ShardId>,
}

impl SafeDisplay for GolemErrorInvalidShardId {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::InvalidShardId>
    for GolemErrorInvalidShardId
{
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::InvalidShardId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            shard_id: value.shard_id.ok_or("Missing field: shard_id")?.into(),
            shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
        })
    }
}

impl From<GolemErrorInvalidShardId> for golem_api_grpc::proto::golem::worker::v1::InvalidShardId {
    fn from(value: GolemErrorInvalidShardId) -> Self {
        Self {
            shard_id: Some(value.shard_id.into()),
            shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Previous invocation failed: {details}")]
pub struct GolemErrorPreviousInvocationFailed {
    pub details: String,
}

impl SafeDisplay for GolemErrorPreviousInvocationFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::PreviousInvocationFailed>
    for GolemErrorPreviousInvocationFailed
{
    fn from(value: golem_api_grpc::proto::golem::worker::v1::PreviousInvocationFailed) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorPreviousInvocationFailed>
    for golem_api_grpc::proto::golem::worker::v1::PreviousInvocationFailed
{
    fn from(value: GolemErrorPreviousInvocationFailed) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Previous invocation exited")]
pub struct GolemErrorPreviousInvocationExited {}

impl SafeDisplay for GolemErrorPreviousInvocationExited {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::PreviousInvocationExited>
    for GolemErrorPreviousInvocationExited
{
    fn from(_value: golem_api_grpc::proto::golem::worker::v1::PreviousInvocationExited) -> Self {
        Self {}
    }
}

impl From<GolemErrorPreviousInvocationExited>
    for golem_api_grpc::proto::golem::worker::v1::PreviousInvocationExited
{
    fn from(_value: GolemErrorPreviousInvocationExited) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Unknown error: {details}")]
pub struct GolemErrorUnknown {
    pub details: String,
}

impl SafeDisplay for GolemErrorUnknown {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::UnknownError> for GolemErrorUnknown {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::UnknownError) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorUnknown> for golem_api_grpc::proto::golem::worker::v1::UnknownError {
    fn from(value: GolemErrorUnknown) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Invalid account")]
pub struct GolemErrorInvalidAccount {}

impl SafeDisplay for GolemErrorInvalidAccount {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::InvalidAccount> for GolemErrorInvalidAccount {
    fn from(_value: golem_api_grpc::proto::golem::worker::v1::InvalidAccount) -> Self {
        Self {}
    }
}

impl From<GolemErrorInvalidAccount> for golem_api_grpc::proto::golem::worker::v1::InvalidAccount {
    fn from(_value: GolemErrorInvalidAccount) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Initial file upload failed")]
pub struct GolemErrorInitialComponentFileUploadFailed {
    pub component_id: VersionedComponentId,
    pub path: String,
    pub reason: String,
}

impl SafeDisplay for GolemErrorInitialComponentFileUploadFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Initial file download failed")]
pub struct GolemErrorInitialComponentFileDownloadFailed {
    pub path: String,
    pub reason: String,
}

impl SafeDisplay for GolemErrorInitialComponentFileDownloadFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::InitialComponentFileDownloadFailed>
    for GolemErrorInitialComponentFileDownloadFailed
{
    fn from(
        value: golem_api_grpc::proto::golem::worker::v1::InitialComponentFileDownloadFailed,
    ) -> Self {
        Self {
            path: value.path,
            reason: value.reason,
        }
    }
}

impl From<GolemErrorInitialComponentFileDownloadFailed>
    for golem_api_grpc::proto::golem::worker::v1::InitialComponentFileDownloadFailed
{
    fn from(value: GolemErrorInitialComponentFileDownloadFailed) -> Self {
        Self {
            path: value.path,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Error with accessing worker files")]
pub struct GolemErrorFileSystemError {
    pub path: String,
    pub reason: String,
}

impl SafeDisplay for GolemErrorFileSystemError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::FileSystemError> for GolemErrorFileSystemError {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::FileSystemError) -> Self {
        Self {
            path: value.path,
            reason: value.reason,
        }
    }
}

impl From<GolemErrorFileSystemError> for golem_api_grpc::proto::golem::worker::v1::FileSystemError {
    fn from(value: GolemErrorFileSystemError) -> Self {
        Self {
            path: value.path,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Invalid account")]
pub struct GolemErrorShardingNotReady {}

impl SafeDisplay for GolemErrorShardingNotReady {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl From<golem_api_grpc::proto::golem::worker::v1::ShardingNotReady>
    for crate::model::GolemErrorShardingNotReady
{
    fn from(_value: golem_api_grpc::proto::golem::worker::v1::ShardingNotReady) -> Self {
        Self {}
    }
}

impl From<crate::model::GolemErrorShardingNotReady>
    for golem_api_grpc::proto::golem::worker::v1::ShardingNotReady
{
    fn from(_value: crate::model::GolemErrorShardingNotReady) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct InvokeParameters {
    pub params: Vec<TypeAnnotatedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct DeleteWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct InvokeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct InterruptResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct ResumeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct UpdateWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct ActivatePluginResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct DeactivatePluginResponse {}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetOplogResponse {
    pub entries: Vec<PublicOplogEntryWithIndex>,
    pub next: Option<OplogCursor>,
    pub first_index_in_chunk: u64,
    pub last_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetFilesResponse {
    pub nodes: Vec<FlatComponentFileSystemNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub enum FlatComponentFileSystemNodeKind {
    Directory,
    File,
}

// Flat, worse typed version ComponentFileSystemNode for rest representation
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct FlatComponentFileSystemNode {
    pub name: String,
    pub last_modified: u64,
    pub kind: FlatComponentFileSystemNodeKind,
    pub permissions: Option<ComponentFilePermissions>, // only for files
    pub size: Option<u64>,                             // only for files
}

impl From<ComponentFileSystemNode> for FlatComponentFileSystemNode {
    fn from(value: ComponentFileSystemNode) -> Self {
        let last_modified = value
            .last_modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        match value.details {
            ComponentFileSystemNodeDetails::Directory => Self {
                name: value.name,
                last_modified,
                kind: FlatComponentFileSystemNodeKind::Directory,
                permissions: None,
                size: None,
            },
            ComponentFileSystemNodeDetails::File { permissions, size } => Self {
                name: value.name,
                last_modified,
                kind: FlatComponentFileSystemNodeKind::File,
                permissions: Some(permissions),
                size: Some(size),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PublicOplogEntryWithIndex {
    pub oplog_index: OplogIndex,
    pub entry: PublicOplogEntry,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OplogEntryWithIndex>
    for PublicOplogEntryWithIndex
{
    type Error = String;

    fn try_from(value: OplogEntryWithIndex) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: OplogIndex::from_u64(value.oplog_index),
            entry: value.entry.ok_or("Missing field: entry")?.try_into()?,
        })
    }
}

impl TryFrom<PublicOplogEntryWithIndex>
    for golem_api_grpc::proto::golem::worker::OplogEntryWithIndex
{
    type Error = String;

    fn try_from(value: PublicOplogEntryWithIndex) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: value.oplog_index.into(),
            entry: Some(value.entry.try_into()?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Enum)]
pub enum WorkerUpdateMode {
    Automatic,
    Manual,
}

impl From<golem_api_grpc::proto::golem::worker::UpdateMode> for WorkerUpdateMode {
    fn from(value: golem_api_grpc::proto::golem::worker::UpdateMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::UpdateMode::Automatic => {
                WorkerUpdateMode::Automatic
            }
            golem_api_grpc::proto::golem::worker::UpdateMode::Manual => WorkerUpdateMode::Manual,
        }
    }
}

impl From<WorkerUpdateMode> for golem_api_grpc::proto::golem::worker::UpdateMode {
    fn from(value: WorkerUpdateMode) -> Self {
        match value {
            WorkerUpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            WorkerUpdateMode::Manual => golem_api_grpc::proto::golem::worker::UpdateMode::Manual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct UpdateWorkerRequest {
    pub mode: WorkerUpdateMode,
    pub target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct WorkersMetadataRequest {
    pub filter: Option<WorkerFilter>,
    pub cursor: Option<ScanCursor>,
    pub count: Option<u64>,
    pub precise: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<WorkerMetadata>,
    pub cursor: Option<ScanCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub component_version: ComponentVersion,
    pub retry_count: u64,
    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<u64, ResourceMetadata>,
    pub active_plugins: HashSet<PluginInstallationId>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerMetadata> for WorkerMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            args: value.args,
            env: value.env,
            status: value.status.try_into()?,
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value
                .updates
                .into_iter()
                .map(|update| update.try_into())
                .collect::<Result<Vec<UpdateRecord>, String>>()?,
            created_at: value.created_at.ok_or("Missing created_at")?.into(),
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value
                .owned_resources
                .into_iter()
                .map(|(k, v)| v.try_into().map(|v| (k, v)))
                .collect::<Result<HashMap<_, _>, _>>()?,
            active_plugins: value
                .active_plugins
                .into_iter()
                .map(|id| id.try_into())
                .collect::<Result<HashSet<_>, _>>()?,
        })
    }
}

impl From<WorkerMetadata> for golem_api_grpc::proto::golem::worker::WorkerMetadata {
    fn from(value: WorkerMetadata) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            account_id: Some(AccountId::placeholder().into()),
            args: value.args,
            env: value.env,
            status: value.status.into(),
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates.iter().cloned().map(|u| u.into()).collect(),
            created_at: Some(value.created_at.into()),
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value
                .owned_resources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            active_plugins: value
                .active_plugins
                .into_iter()
                .map(|id| id.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union)]
#[serde(rename_all = "camelCase")]
#[oai(discriminator_name = "type", one_of = true, rename_all = "camelCase")]
pub enum UpdateRecord {
    PendingUpdate(PendingUpdate),
    SuccessfulUpdate(SuccessfulUpdate),
    FailedUpdate(FailedUpdate),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PendingUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SuccessfulUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct FailedUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
    details: Option<String>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::UpdateRecord> for UpdateRecord {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::UpdateRecord,
    ) -> Result<Self, Self::Error> {
        match value.update.ok_or("Missing update field")? {
            golem_api_grpc::proto::golem::worker::update_record::Update::Failed(failed) => {
                Ok(Self::FailedUpdate(FailedUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                    details: { failed.details },
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Pending(_) => {
                Ok(Self::PendingUpdate(PendingUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Successful(_) => {
                Ok(Self::SuccessfulUpdate(SuccessfulUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                }))
            }
        }
    }
}

impl From<UpdateRecord> for golem_api_grpc::proto::golem::worker::UpdateRecord {
    fn from(value: UpdateRecord) -> Self {
        match value {
            UpdateRecord::FailedUpdate(FailedUpdate {
                timestamp,
                target_version,
                details,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Failed(
                        golem_api_grpc::proto::golem::worker::FailedUpdate { details },
                    ),
                ),
            },
            UpdateRecord::PendingUpdate(PendingUpdate {
                timestamp,
                target_version,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Pending(
                        golem_api_grpc::proto::golem::worker::PendingUpdate {},
                    ),
                ),
            },
            UpdateRecord::SuccessfulUpdate(SuccessfulUpdate {
                timestamp,
                target_version,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Successful(
                        golem_api_grpc::proto::golem::worker::SuccessfulUpdate {},
                    ),
                ),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResourceMetadata {
    pub created_at: Timestamp,
    pub indexed: Option<IndexedWorkerMetadata>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::ResourceMetadata> for ResourceMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::ResourceMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            created_at: value.created_at.ok_or("Missing created_at")?.into(),
            indexed: value.indexed.map(|i| i.into()),
        })
    }
}

impl From<ResourceMetadata> for golem_api_grpc::proto::golem::worker::ResourceMetadata {
    fn from(value: ResourceMetadata) -> Self {
        Self {
            created_at: Some(value.created_at.into()),
            indexed: value.indexed.map(|i| i.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct IndexedWorkerMetadata {
    pub resource_name: String,
    pub resource_params: Vec<String>,
}

impl From<golem_api_grpc::proto::golem::worker::IndexedResourceMetadata> for IndexedWorkerMetadata {
    fn from(value: golem_api_grpc::proto::golem::worker::IndexedResourceMetadata) -> Self {
        Self {
            resource_name: value.resource_name,
            resource_params: value.resource_params,
        }
    }
}

impl From<IndexedWorkerMetadata> for golem_api_grpc::proto::golem::worker::IndexedResourceMetadata {
    fn from(value: IndexedWorkerMetadata) -> Self {
        Self {
            resource_name: value.resource_name,
            resource_params: value.resource_params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct InvokeResult {
    pub result: TypeAnnotatedValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union, thiserror::Error)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum GolemError {
    #[error(transparent)]
    InvalidRequest(GolemErrorInvalidRequest),
    #[error(transparent)]
    WorkerAlreadyExists(GolemErrorWorkerAlreadyExists),
    #[error(transparent)]
    WorkerNotFound(GolemErrorWorkerNotFound),
    #[error(transparent)]
    WorkerCreationFailed(GolemErrorWorkerCreationFailed),
    #[error(transparent)]
    FailedToResumeWorker(GolemErrorFailedToResumeWorker),
    #[error(transparent)]
    ComponentDownloadFailed(GolemErrorComponentDownloadFailed),
    #[error(transparent)]
    ComponentParseFailed(GolemErrorComponentParseFailed),
    #[error(transparent)]
    GetLatestVersionOfComponentFailed(GolemErrorGetLatestVersionOfComponentFailed),
    #[error(transparent)]
    PromiseNotFound(GolemErrorPromiseNotFound),
    #[error(transparent)]
    PromiseDropped(GolemErrorPromiseDropped),
    #[error(transparent)]
    PromiseAlreadyCompleted(GolemErrorPromiseAlreadyCompleted),
    #[error(transparent)]
    Interrupted(GolemErrorInterrupted),
    #[error(transparent)]
    ParamTypeMismatch(GolemErrorParamTypeMismatch),
    #[error(transparent)]
    NoValueInMessage(GolemErrorNoValueInMessage),
    #[error(transparent)]
    ValueMismatch(GolemErrorValueMismatch),
    #[error(transparent)]
    UnexpectedOplogEntry(GolemErrorUnexpectedOplogEntry),
    #[error(transparent)]
    RuntimeError(GolemErrorRuntimeError),
    #[error(transparent)]
    InvalidShardId(GolemErrorInvalidShardId),
    #[error(transparent)]
    PreviousInvocationFailed(GolemErrorPreviousInvocationFailed),
    #[error(transparent)]
    PreviousInvocationExited(GolemErrorPreviousInvocationExited),
    #[error(transparent)]
    Unknown(GolemErrorUnknown),
    #[error(transparent)]
    InvalidAccount(GolemErrorInvalidAccount),
    #[error(transparent)]
    ShardingNotReady(GolemErrorShardingNotReady),
    #[error(transparent)]
    InitialComponentFileDownloadFailed(GolemErrorInitialComponentFileDownloadFailed),
    #[error(transparent)]
    FileSystemError(GolemErrorFileSystemError),
}

impl SafeDisplay for GolemError {
    fn to_safe_string(&self) -> String {
        match self {
            GolemError::InvalidRequest(inner) => inner.to_safe_string(),
            GolemError::WorkerAlreadyExists(inner) => inner.to_safe_string(),
            GolemError::WorkerNotFound(inner) => inner.to_safe_string(),
            GolemError::WorkerCreationFailed(inner) => inner.to_safe_string(),
            GolemError::FailedToResumeWorker(inner) => inner.to_safe_string(),
            GolemError::ComponentDownloadFailed(inner) => inner.to_safe_string(),
            GolemError::ComponentParseFailed(inner) => inner.to_safe_string(),
            GolemError::GetLatestVersionOfComponentFailed(inner) => inner.to_safe_string(),
            GolemError::PromiseNotFound(inner) => inner.to_safe_string(),
            GolemError::PromiseDropped(inner) => inner.to_safe_string(),
            GolemError::PromiseAlreadyCompleted(inner) => inner.to_safe_string(),
            GolemError::Interrupted(inner) => inner.to_safe_string(),
            GolemError::ParamTypeMismatch(inner) => inner.to_safe_string(),
            GolemError::NoValueInMessage(inner) => inner.to_safe_string(),
            GolemError::ValueMismatch(inner) => inner.to_safe_string(),
            GolemError::UnexpectedOplogEntry(inner) => inner.to_safe_string(),
            GolemError::RuntimeError(inner) => inner.to_safe_string(),
            GolemError::InvalidShardId(inner) => inner.to_safe_string(),
            GolemError::PreviousInvocationFailed(inner) => inner.to_safe_string(),
            GolemError::PreviousInvocationExited(inner) => inner.to_safe_string(),
            GolemError::Unknown(inner) => inner.to_safe_string(),
            GolemError::InvalidAccount(inner) => inner.to_safe_string(),
            GolemError::ShardingNotReady(inner) => inner.to_safe_string(),
            GolemError::InitialComponentFileDownloadFailed(inner) => inner.to_safe_string(),
            GolemError::FileSystemError(inner) => inner.to_safe_string(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError> for GolemError {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError,
    ) -> Result<Self, Self::Error> {
        match value.error {
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidRequest(err)) => {
                Ok(GolemError::InvalidRequest(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(err)) => {
                Ok(GolemError::WorkerAlreadyExists(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerNotFound(err)) => {
                Ok(GolemError::WorkerNotFound(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(err)) => {
                Ok(GolemError::WorkerCreationFailed(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(err)) => {
                Ok(GolemError::FailedToResumeWorker((*err).try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(err)) => {
                Ok(GolemError::ComponentDownloadFailed(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(err)) => {
                Ok(GolemError::ComponentParseFailed(err.try_into()?))
            }
            Some(
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                    err,
                ),
            ) => Ok(GolemError::GetLatestVersionOfComponentFailed(
                err.try_into()?,
            )),
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseNotFound(err)) => {
                Ok(GolemError::PromiseNotFound(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseDropped(err)) => {
                Ok(GolemError::PromiseDropped(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(err)) => {
                Ok(GolemError::PromiseAlreadyCompleted(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Interrupted(err)) => {
                Ok(GolemError::Interrupted(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(err)) => {
                Ok(GolemError::ParamTypeMismatch(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::NoValueInMessage(err)) => {
                Ok(GolemError::NoValueInMessage(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ValueMismatch(err)) => {
                Ok(GolemError::ValueMismatch(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(err)) => {
                Ok(GolemError::UnexpectedOplogEntry(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::RuntimeError(err)) => {
                Ok(GolemError::RuntimeError(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidShardId(err)) => {
                Ok(GolemError::InvalidShardId(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(err)) => {
                Ok(GolemError::PreviousInvocationFailed(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(err)) => {
                Ok(GolemError::PreviousInvocationExited(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Unknown(err)) => {
                Ok(GolemError::Unknown(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidAccount(err)) => {
                Ok(GolemError::InvalidAccount(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ShardingNotReady(err)) => {
                Ok(GolemError::ShardingNotReady(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(err)) => {
                Ok(GolemError::InitialComponentFileDownloadFailed(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FileSystemError(err)) => {
                Ok(GolemError::FileSystemError(err.into()))
            }
            None => Err("Missing field: error".to_string()),
        }
    }
}

impl From<GolemError> for golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError {
    fn from(error: GolemError) -> Self {
        golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError {
            error: Some(error.into()),
        }
    }
}

impl From<GolemError> for golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error {
    fn from(error: GolemError) -> Self {
        match error {
            GolemError::InvalidRequest(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidRequest(err.into())
            }
            GolemError::WorkerAlreadyExists(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(err.into())
            }
            GolemError::WorkerNotFound(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerNotFound(err.into())
            }
            GolemError::WorkerCreationFailed(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(err.into())
            }
            GolemError::FailedToResumeWorker(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(Box::new(err.into()))
            }
            GolemError::ComponentDownloadFailed(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(err.into())
            }
            GolemError::ComponentParseFailed(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(err.into())
            }
            GolemError::GetLatestVersionOfComponentFailed(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(err.into())
            }
            GolemError::PromiseNotFound(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseNotFound(err.into())
            }
            GolemError::PromiseDropped(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseDropped(err.into())
            }
            GolemError::PromiseAlreadyCompleted(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(err.into())
            }
            GolemError::Interrupted(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Interrupted(err.into())
            }
            GolemError::ParamTypeMismatch(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(err.into())
            }
            GolemError::NoValueInMessage(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::NoValueInMessage(err.into())
            }
            GolemError::ValueMismatch(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ValueMismatch(err.into())
            }
            GolemError::UnexpectedOplogEntry(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(err.into())
            }
            GolemError::RuntimeError(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::RuntimeError(err.into())
            }
            GolemError::InvalidShardId(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidShardId(err.into())
            }
            GolemError::PreviousInvocationFailed(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(err.into())
            }
            GolemError::PreviousInvocationExited(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(err.into())
            }
            GolemError::Unknown(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Unknown(err.into())
            }
            GolemError::InvalidAccount(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidAccount(err.into())
            }
            GolemError::ShardingNotReady(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ShardingNotReady(err.into())
            }
            GolemError::InitialComponentFileDownloadFailed(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(err.into())
            }
            GolemError::FileSystemError(err) => {
                golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FileSystemError(err.into())
            }
        }
    }
}

#[derive(Object, Clone, Debug)]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorBody {
    pub golem_error: GolemError,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError> for GolemErrorBody {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            golem_error: value.try_into()?,
        })
    }
}

#[derive(Debug, Clone, Object, Serialize)]
pub struct ErrorsBody {
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Object, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

impl From<golem_api_grpc::proto::golem::common::ErrorBody> for ErrorBody {
    fn from(value: golem_api_grpc::proto::golem::common::ErrorBody) -> Self {
        Self { error: value.error }
    }
}

impl From<golem_api_grpc::proto::golem::common::ErrorsBody> for ErrorsBody {
    fn from(value: golem_api_grpc::proto::golem::common::ErrorsBody) -> Self {
        Self {
            errors: value.errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub component_type: Option<ComponentType>,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        let created_at = match &value.created_at {
            Some(t) => {
                let t = SystemTime::try_from(*t).map_err(|_| "Failed to convert timestamp")?;
                Some(t.into())
            }
            None => None,
        };

        let component_type = if value.component_type.is_some() {
            Some(value.component_type().into())
        } else {
            None
        };

        let files = value
            .files
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        let installed_plugins = value
            .installed_plugins
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
            component_name: ComponentName(value.component_name.clone()),
            component_size: value.component_size,
            metadata: value
                .metadata
                .clone()
                .ok_or("Missing metadata")?
                .try_into()?,
            created_at,
            component_type,
            files,
            installed_plugins,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: None,
            created_at: value
                .created_at
                .map(|t| prost_types::Timestamp::from(SystemTime::from(t))),
            component_type: value.component_type.map(|c| {
                let c: golem_api_grpc::proto::golem::component::ComponentType = c.into();
                c.into()
            }),
            files: value.files.into_iter().map(|f| f.into()).collect(),
            installed_plugins: value
                .installed_plugins
                .into_iter()
                .map(|p| p.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub available_fuel: i64,
    pub max_memory_per_worker: i64,
}

impl From<ResourceLimits> for golem_api_grpc::proto::golem::common::ResourceLimits {
    fn from(value: ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::ResourceLimits> for ResourceLimits {
    fn from(value: golem_api_grpc::proto::golem::common::ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}
