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

use std::collections::{BTreeMap, HashMap};

use crate::base_model::OplogIndex;
use crate::model::component::PluginPriority;
use crate::model::oplog::{
    HostStreamKind, LogLevel, OplogCursor, OplogErrorKind, PluginInstallationDescription,
    PublicTypedAgentConfigEntry,
};

fn decode_attachment_observation(
    value: &crate::schema::TypedSchemaValue,
) -> Result<crate::base_model::oplog::attachment::ToolAttachmentReadObservation, String> {
    use crate::schema::FromSchema as _;
    crate::base_model::oplog::attachment::ToolAttachmentReadObservation::from_value(value.value())
        .map_err(|error| error.to_string())
}

fn decode_attachment_control(
    attachment: &crate::schema::TypedSchemaValue,
    transition: &crate::schema::TypedSchemaValue,
) -> Result<
    (
        crate::base_model::oplog::attachment::ToolAttachmentId,
        crate::base_model::oplog::attachment::ToolAttachmentControl,
    ),
    String,
> {
    use crate::base_model::oplog::attachment::{ToolAttachmentControl, ToolAttachmentId};
    use crate::schema::FromSchema as _;
    Ok((
        ToolAttachmentId::from_value(attachment.value()).map_err(|error| error.to_string())?,
        ToolAttachmentControl::from_value(transition.value()).map_err(|error| error.to_string())?,
    ))
}

impl TryFrom<PublicTypedAgentConfigEntry>
    for golem_api_grpc::proto::golem::worker::PublicTypedAgentConfigEntry
{
    type Error = String;

    fn try_from(value: PublicTypedAgentConfigEntry) -> Result<Self, Self::Error> {
        Ok(Self {
            path: value.path,
            value: Some(value.value.try_into()?),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicTypedAgentConfigEntry>
    for PublicTypedAgentConfigEntry
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicTypedAgentConfigEntry,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            path: value.path,
            value: value
                .value
                .ok_or("Missing value field in PublicTypedAgentConfigEntry")?
                .try_into()?,
        })
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogCursor> for OplogCursor {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_revision: value.current_component_revision,
        }
    }
}

impl From<OplogCursor> for golem_api_grpc::proto::golem::worker::OplogCursor {
    fn from(value: OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_revision: value.current_component_revision,
        }
    }
}

impl From<PluginInstallationDescription>
    for golem_api_grpc::proto::golem::worker::PluginInstallationDescription
{
    fn from(plugin_installation_description: PluginInstallationDescription) -> Self {
        golem_api_grpc::proto::golem::worker::PluginInstallationDescription {
            environment_plugin_grant_id: Some(
                plugin_installation_description
                    .environment_plugin_grant_id
                    .into(),
            ),
            plugin_priority: plugin_installation_description.plugin_priority.0,
            plugin_name: plugin_installation_description.plugin_name,
            plugin_version: plugin_installation_description.plugin_version,
            parameters: HashMap::from_iter(plugin_installation_description.parameters),
            registered: false,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PluginInstallationDescription>
    for PluginInstallationDescription
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PluginInstallationDescription,
    ) -> Result<Self, Self::Error> {
        Ok(PluginInstallationDescription {
            environment_plugin_grant_id: value
                .environment_plugin_grant_id
                .ok_or("Missing environment_plugin_grant_id field")?
                .try_into()?,
            plugin_priority: PluginPriority(value.plugin_priority),
            plugin_name: value.plugin_name,
            plugin_version: value.plugin_version,
            parameters: BTreeMap::from_iter(value.parameters),
        })
    }
}

impl From<crate::base_model::oplog::DurableInvocationId>
    for golem_api_grpc::proto::golem::worker::DurableInvocationId
{
    fn from(value: crate::base_model::oplog::DurableInvocationId) -> Self {
        Self {
            anchor: value.anchor.as_u64(),
            ordinal: value.ordinal,
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::DurableInvocationId>
    for crate::base_model::oplog::DurableInvocationId
{
    fn from(value: golem_api_grpc::proto::golem::worker::DurableInvocationId) -> Self {
        Self::new(OplogIndex::from_u64(value.anchor), value.ordinal)
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogLogLevel> for LogLevel {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogLogLevel) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogTrace => LogLevel::Trace,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogDebug => LogLevel::Debug,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogInfo => LogLevel::Info,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogWarn => LogLevel::Warn,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogError => LogLevel::Error,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogCritical => {
                LogLevel::Critical
            }
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStderr => LogLevel::Stderr,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStdout => LogLevel::Stdout,
        }
    }
}

impl From<LogLevel> for golem_api_grpc::proto::golem::worker::OplogLogLevel {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Trace => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogTrace,
            LogLevel::Debug => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogDebug,
            LogLevel::Info => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogInfo,
            LogLevel::Warn => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogWarn,
            LogLevel::Error => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogError,
            LogLevel::Critical => {
                golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogCritical
            }
            LogLevel::Stderr => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStderr,
            LogLevel::Stdout => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStdout,
        }
    }
}

// =============================================================================
// Raw OplogEntry ↔ proto RawOplogEntry conversions
// =============================================================================

fn host_stream_kind_to_proto(
    kind: HostStreamKind,
) -> golem_api_grpc::proto::golem::worker::HostStreamKind {
    match kind {
        HostStreamKind::P3HttpRequestBody => {
            golem_api_grpc::proto::golem::worker::HostStreamKind::P3HttpRequestBody
        }
    }
}

fn host_stream_kind_from_proto(kind: i32) -> Result<HostStreamKind, String> {
    match golem_api_grpc::proto::golem::worker::HostStreamKind::try_from(kind) {
        Ok(golem_api_grpc::proto::golem::worker::HostStreamKind::P3HttpRequestBody) => {
            Ok(HostStreamKind::P3HttpRequestBody)
        }
        Err(_) => Err(format!("Invalid host stream kind: {kind}")),
    }
}

fn oplog_error_kind_to_proto(
    kind: OplogErrorKind,
) -> golem_api_grpc::proto::golem::worker::OplogErrorKind {
    match kind {
        OplogErrorKind::Invocation => {
            golem_api_grpc::proto::golem::worker::OplogErrorKind::Invocation
        }
        OplogErrorKind::Recovery => golem_api_grpc::proto::golem::worker::OplogErrorKind::Recovery,
    }
}

fn oplog_error_kind_from_proto(kind: i32) -> Result<OplogErrorKind, String> {
    match golem_api_grpc::proto::golem::worker::OplogErrorKind::try_from(kind) {
        Ok(golem_api_grpc::proto::golem::worker::OplogErrorKind::Invocation) => {
            Ok(OplogErrorKind::Invocation)
        }
        Ok(golem_api_grpc::proto::golem::worker::OplogErrorKind::Recovery) => {
            Ok(OplogErrorKind::Recovery)
        }
        Err(_) => Err(format!("Unknown oplog error kind: {kind}")),
    }
}

#[cfg(test)]
mod tests;

mod cards;

mod invocations;

mod spans;

mod raw;

mod public;
