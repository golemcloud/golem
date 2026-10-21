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

use crate::base_model::component_metadata::{
    AgentTypeProvisionConfig, ComponentProvisionConfig, KnownExports,
};
use crate::base_model::json::NormalizedJsonValue;
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::AgentTypeName;
use crate::model::agent_config::CanonicalAgentConfigPath;
use crate::model::agent_secret::CanonicalAgentSecretPath;
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::component_metadata::{
    AgentMethodStreamMetadata, ComponentMetadata, ComponentMetadataInnerData, LinearMemory,
    ProducerField, Producers, VersionedName,
};
use crate::model::deployment::DeploymentRevision;
use crate::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolBindingInput, ToolBindingOwner,
    ToolDeploymentMetadata, ToolDeploymentState, ToolName, ToolProvisionConfig, ToolSource,
};
use crate::model::tool_middleware::{ToolMiddlewareDeploymentMetadata, ToolMiddlewareName};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

impl From<golem_api_grpc::proto::golem::component::VersionedName> for VersionedName {
    fn from(value: golem_api_grpc::proto::golem::component::VersionedName) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

impl From<VersionedName> for golem_api_grpc::proto::golem::component::VersionedName {
    fn from(value: VersionedName) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::ProducerField> for ProducerField {
    fn from(value: golem_api_grpc::proto::golem::component::ProducerField) -> Self {
        Self {
            name: value.name,
            values: value.values.into_iter().map(|value| value.into()).collect(),
        }
    }
}

impl From<ProducerField> for golem_api_grpc::proto::golem::component::ProducerField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value.values.into_iter().map(|value| value.into()).collect(),
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::Producers> for Producers {
    fn from(value: golem_api_grpc::proto::golem::component::Producers) -> Self {
        Self {
            fields: value.fields.into_iter().map(|field| field.into()).collect(),
        }
    }
}

impl From<Producers> for golem_api_grpc::proto::golem::component::Producers {
    fn from(value: Producers) -> Self {
        Self {
            fields: value.fields.into_iter().map(|field| field.into()).collect(),
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::LinearMemory> for LinearMemory {
    fn from(value: golem_api_grpc::proto::golem::component::LinearMemory) -> Self {
        Self {
            initial: value.initial,
            maximum: value.maximum,
            shared: value.shared,
        }
    }
}

impl From<LinearMemory> for golem_api_grpc::proto::golem::component::LinearMemory {
    fn from(value: LinearMemory) -> Self {
        Self {
            initial: value.initial,
            maximum: value.maximum,
            shared: value.shared,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata> for ComponentMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ComponentMetadata,
    ) -> Result<Self, Self::Error> {
        let inner_data = ComponentMetadataInnerData::try_from(value)?;
        Ok(Self {
            data: Arc::new(inner_data),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata>
    for ComponentMetadataInnerData
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ComponentMetadata,
    ) -> Result<Self, Self::Error> {
        let known_exports = value
            .known_exports
            .map(KnownExports::from)
            .unwrap_or_default();
        Ok(Self {
            known_exports,
            producers: value
                .producers
                .into_iter()
                .map(|producer| producer.into())
                .collect(),
            memories: value
                .memories
                .into_iter()
                .map(|memory| memory.into())
                .collect(),
            root_package_name: value.root_package_name,
            root_package_version: value.root_package_version,
            config_schema: value
                .config_schema
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
            component_provision_config: value
                .component_provision_config
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
            agent_types: value
                .agent_types
                .into_iter()
                .map(|at| at.try_into())
                .collect::<Result<_, _>>()?,
            agent_method_streams: value
                .agent_method_streams
                .into_iter()
                .map(|(agent, methods)| {
                    let methods = methods
                        .methods
                        .into_iter()
                        .map(|(name, metadata)| {
                            (
                                name,
                                AgentMethodStreamMetadata {
                                    input: metadata.input,
                                    output: metadata.output,
                                },
                            )
                        })
                        .collect();
                    (AgentTypeName(agent), methods)
                })
                .collect(),
            agent_type_provision_configs: value
                .agent_type_provision_configs
                .into_iter()
                .map(|(k, v)| {
                    AgentTypeProvisionConfig::try_from(v).map(|config| (AgentTypeName(k), config))
                })
                .collect::<Result<_, _>>()?,
            tools: value
                .tools
                .into_iter()
                .map(|(name, metadata)| Ok((ToolName::try_from(name)?, metadata.try_into()?)))
                .collect::<Result<_, String>>()?,
            tool_middlewares: value
                .tool_middlewares
                .into_iter()
                .map(|(name, metadata)| {
                    Ok((ToolMiddlewareName::try_from(name)?, metadata.try_into()?))
                })
                .collect::<Result<_, String>>()?,
        })
    }
}

impl From<golem_api_grpc::proto::golem::component::KnownExports> for KnownExports {
    fn from(value: golem_api_grpc::proto::golem::component::KnownExports) -> Self {
        Self {
            agent_guest_interface: value.agent_guest_interface,
            save_snapshot_interface: value.save_snapshot_interface,
            load_snapshot_interface: value.load_snapshot_interface,
            oplog_processor_interface: value.oplog_processor_interface,
            tool_guest_interface: value.tool_guest_interface,
            tool_middleware_guest_interface: value.tool_middleware_guest_interface,
        }
    }
}

impl From<KnownExports> for golem_api_grpc::proto::golem::component::KnownExports {
    fn from(value: KnownExports) -> Self {
        Self {
            agent_guest_interface: value.agent_guest_interface,
            save_snapshot_interface: value.save_snapshot_interface,
            load_snapshot_interface: value.load_snapshot_interface,
            oplog_processor_interface: value.oplog_processor_interface,
            tool_guest_interface: value.tool_guest_interface,
            tool_middleware_guest_interface: value.tool_middleware_guest_interface,
        }
    }
}

impl TryFrom<ComponentMetadata> for golem_api_grpc::proto::golem::component::ComponentMetadata {
    type Error = String;

    fn try_from(value: ComponentMetadata) -> Result<Self, Self::Error> {
        value.data.as_ref().clone().try_into()
    }
}

impl TryFrom<ComponentMetadataInnerData>
    for golem_api_grpc::proto::golem::component::ComponentMetadata
{
    type Error = String;

    fn try_from(value: ComponentMetadataInnerData) -> Result<Self, Self::Error> {
        Ok(Self {
            known_exports: Some(value.known_exports.into()),
            producers: value
                .producers
                .into_iter()
                .map(|producer| producer.into())
                .collect(),
            memories: value
                .memories
                .into_iter()
                .map(|memory| memory.into())
                .collect(),
            root_package_name: value.root_package_name,
            root_package_version: value.root_package_version,
            config_schema: Some(value.config_schema.into()),
            component_provision_config: Some(value.component_provision_config.try_into()?),
            agent_types: value.agent_types.into_iter().map(|at| at.into()).collect(),
            agent_method_streams: value
                .agent_method_streams
                .into_iter()
                .map(|(agent, methods)| {
                    let methods = methods
                        .into_iter()
                        .map(|(name, metadata)| {
                            (name, golem_api_grpc::proto::golem::component::AgentMethodStreamMetadata {
                                input: metadata.input,
                                output: metadata.output,
                            })
                        })
                        .collect();
                    (agent.0, golem_api_grpc::proto::golem::component::AgentMethodStreams { methods })
                })
                .collect(),
            agent_type_provision_configs: value
                .agent_type_provision_configs
                .into_iter()
                .map(|(k, v)| v.try_into().map(|config| (k.0, config)))
                .collect::<Result<_, _>>()?,
            tools: value
                .tools
                .into_iter()
                .map(|(name, metadata)| (name.into_inner(), metadata.into()))
                .collect(),
            tool_middlewares: value
                .tool_middlewares
                .into_iter()
                .map(|(name, metadata)| (name.into_inner(), metadata.into()))
                .collect(),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ToolMiddlewareDeploymentMetadata>
    for ToolMiddlewareDeploymentMetadata
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ToolMiddlewareDeploymentMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            definition: value
                .definition
                .ok_or("Missing ToolMiddlewareDeploymentMetadata.definition")?
                .try_into()?,
            provision: value
                .provision
                .ok_or("Missing ToolMiddlewareDeploymentMetadata.provision")?
                .try_into()?,
        })
    }
}

impl From<ToolMiddlewareDeploymentMetadata>
    for golem_api_grpc::proto::golem::component::ToolMiddlewareDeploymentMetadata
{
    fn from(value: ToolMiddlewareDeploymentMetadata) -> Self {
        Self {
            definition: Some(value.definition.into()),
            provision: Some(value.provision.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ToolDeploymentMetadata>
    for ToolDeploymentMetadata
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ToolDeploymentMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            definition: value
                .definition
                .ok_or_else(|| "Missing ToolDeploymentMetadata.definition".to_string())?
                .try_into()?,
            provision: value
                .provision
                .ok_or_else(|| "Missing ToolDeploymentMetadata.provision".to_string())?
                .try_into()?,
            environment_binding: value
                .environment_binding
                .map(TryInto::try_into)
                .transpose()?,
            component_bindings: value
                .component_bindings
                .into_iter()
                .map(|(name, binding)| {
                    ToolBindingInput::try_from(binding)
                        .map(|binding| (ComponentName(name), binding))
                })
                .collect::<Result<_, _>>()?,
            agent_bindings: value
                .agent_bindings
                .into_iter()
                .map(|(name, binding)| {
                    ToolBindingInput::try_from(binding)
                        .map(|binding| (AgentTypeName(name), binding))
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ToolDeploymentMetadata>
    for golem_api_grpc::proto::golem::component::ToolDeploymentMetadata
{
    fn from(value: ToolDeploymentMetadata) -> Self {
        Self {
            definition: Some(value.definition.into()),
            provision: Some(value.provision.into()),
            environment_binding: value.environment_binding.map(Into::into),
            component_bindings: value
                .component_bindings
                .into_iter()
                .map(|(name, binding)| (name.0, binding.into()))
                .collect(),
            agent_bindings: value
                .agent_bindings
                .into_iter()
                .map(|(name, binding)| (name.0, binding.into()))
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ToolProvisionConfig> for ToolProvisionConfig {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ToolProvisionConfig,
    ) -> Result<Self, Self::Error> {
        let config = serde_json::from_str(&value.config_json)
            .map(NormalizedJsonValue::new)
            .map_err(|error| format!("Invalid ToolProvisionConfig.config_json: {error}"))?;
        Ok(Self {
            config,
            env: value.env.into_iter().collect(),
            plugins: value
                .plugins
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            files: value
                .files
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ToolProvisionConfig> for golem_api_grpc::proto::golem::component::ToolProvisionConfig {
    fn from(value: ToolProvisionConfig) -> Self {
        Self {
            config_json: value.config.to_string(),
            env: value.env.into_iter().collect(),
            plugins: value.plugins.into_iter().map(Into::into).collect(),
            files: value.files.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ToolBindingInput> for ToolBindingInput {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ToolBindingInput,
    ) -> Result<Self, Self::Error> {
        let filesystem_access = value.filesystem_access();
        let parameters = serde_json::from_str(&value.parameters_json)
            .map(NormalizedJsonValue::new)
            .map_err(|error| format!("Invalid ToolBindingInput.parameters_json: {error}"))?;
        Ok(Self {
            version: value.version,
            parameters,
            account: value.account.map(AccountEmail::new),
            config_keys_readable: value
                .config_keys_readable
                .ok_or_else(|| "Missing ToolBindingInput.config_keys_readable".to_string())?
                .try_into()?,
            secret_keys_readable: value
                .secret_keys_readable
                .ok_or_else(|| "Missing ToolBindingInput.secret_keys_readable".to_string())?
                .try_into()?,
            secret_keys_revealable: value
                .secret_keys_revealable
                .ok_or_else(|| "Missing ToolBindingInput.secret_keys_revealable".to_string())?
                .try_into()?,
            filesystem_access: filesystem_access.into(),
            middleware: value
                .middleware
                .map(|installations| {
                    installations
                        .values
                        .into_iter()
                        .map(|installation| {
                            let filesystem_access = installation.filesystem_access();
                            Ok(crate::model::tool_middleware::ToolMiddlewareInstallation {
                                name: ToolMiddlewareName::try_from(installation.name)?,
                                version: installation.version,
                                parameters: NormalizedJsonValue::new(
                                    serde_json::from_str(&installation.parameters_json)
                                        .map_err(|error| format!("Invalid ToolMiddlewareInstallation.parameters_json: {error}"))?,
                                ),
                                account: installation.account.map(AccountEmail::new),
                                secret_keys_readable: installation
                                    .secret_keys_readable
                                    .map(TryInto::try_into)
                                    .transpose()?,
                                secret_keys_revealable: installation
                                    .secret_keys_revealable
                                    .map(TryInto::try_into)
                                    .transpose()?,
                                filesystem_access: filesystem_access.into(),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
                .transpose()?,
            middleware_merge_mode: value
                .middleware_merge_mode
                .map(|mode| {
                    golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::try_from(mode)
                        .map_err(|_| format!("Invalid ToolBindingInput.middleware_merge_mode: {mode}"))
                        .map(|mode| match mode {
                            golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::Prepend => crate::model::tool_middleware::ToolMiddlewareMergeMode::Prepend,
                            golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::Append => crate::model::tool_middleware::ToolMiddlewareMergeMode::Append,
                            golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::Replace => crate::model::tool_middleware::ToolMiddlewareMergeMode::Replace,
                        })
                })
                .transpose()?,
        })
    }
}

impl From<ToolBindingInput> for golem_api_grpc::proto::golem::component::ToolBindingInput {
    fn from(value: ToolBindingInput) -> Self {
        Self {
            version: value.version,
            parameters_json: value.parameters.to_string(),
            account: value.account.map(AccountEmail::into_inner),
            config_keys_readable: Some(value.config_keys_readable.into()),
            secret_keys_readable: Some(value.secret_keys_readable.into()),
            secret_keys_revealable: Some(value.secret_keys_revealable.into()),
            middleware: value.middleware.map(|values| {
                golem_api_grpc::proto::golem::component::ToolMiddlewareInstallations {
                    values: values.into_iter().map(|installation| {
                        golem_api_grpc::proto::golem::component::ToolMiddlewareInstallation {
                            name: installation.name.into_inner(),
                            version: installation.version,
                            parameters_json: installation.parameters.to_string(),
                            account: installation.account.map(AccountEmail::into_inner),
                            filesystem_access: golem_api_grpc::proto::golem::component::ToolFilesystemAccess::from(installation.filesystem_access) as i32,
                            secret_keys_readable: installation
                                .secret_keys_readable
                                .map(Into::into),
                            secret_keys_revealable: installation
                                .secret_keys_revealable
                                .map(Into::into),
                        }
                    }).collect(),
                }
            }),
            middleware_merge_mode: value.middleware_merge_mode.map(|mode| match mode {
                crate::model::tool_middleware::ToolMiddlewareMergeMode::Prepend => {
                    golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::Prepend
                        as i32
                }
                crate::model::tool_middleware::ToolMiddlewareMergeMode::Append => {
                    golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::Append
                        as i32
                }
                crate::model::tool_middleware::ToolMiddlewareMergeMode::Replace => {
                    golem_api_grpc::proto::golem::component::ToolMiddlewareMergeMode::Replace
                        as i32
                }
            }),
            filesystem_access: golem_api_grpc::proto::golem::component::ToolFilesystemAccess::from(value.filesystem_access) as i32,
        }
    }
}

impl From<crate::model::tool::ToolFilesystemAccess>
    for golem_api_grpc::proto::golem::component::ToolFilesystemAccess
{
    fn from(value: crate::model::tool::ToolFilesystemAccess) -> Self {
        match value {
            crate::model::tool::ToolFilesystemAccess::Unset => Self::Unset,
            crate::model::tool::ToolFilesystemAccess::Allowed => Self::Allowed,
            crate::model::tool::ToolFilesystemAccess::Denied => Self::Denied,
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::ToolFilesystemAccess>
    for crate::model::tool::ToolFilesystemAccess
{
    fn from(value: golem_api_grpc::proto::golem::component::ToolFilesystemAccess) -> Self {
        match value {
            golem_api_grpc::proto::golem::component::ToolFilesystemAccess::Unset => Self::Unset,
            golem_api_grpc::proto::golem::component::ToolFilesystemAccess::Allowed => Self::Allowed,
            golem_api_grpc::proto::golem::component::ToolFilesystemAccess::Denied => Self::Denied,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ConfigKeyScope>
    for crate::model::tool::ConfigKeyScope
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ConfigKeyScope,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::config_key_scope::Value;
        match value
            .value
            .ok_or_else(|| "Missing ConfigKeyScope.value".to_string())?
        {
            Value::All(_) => Ok(Self::All),
            Value::Keys(keys) => Ok(Self::Keys(
                keys.paths
                    .into_iter()
                    .map(|path| CanonicalAgentConfigPath(path.segments))
                    .collect(),
            )),
        }
    }
}

impl From<crate::model::tool::ConfigKeyScope>
    for golem_api_grpc::proto::golem::component::ConfigKeyScope
{
    fn from(value: crate::model::tool::ConfigKeyScope) -> Self {
        use golem_api_grpc::proto::golem::component::config_key_scope::Value;
        let value = match value {
            crate::model::tool::ConfigKeyScope::All => {
                Value::All(golem_schema::proto::golem::common::Empty {})
            }
            crate::model::tool::ConfigKeyScope::Keys(keys) => {
                Value::Keys(golem_api_grpc::proto::golem::component::ConfigKeyPaths {
                    paths: keys
                        .into_iter()
                        .map(
                            |path| golem_api_grpc::proto::golem::component::ConfigKeyPath {
                                segments: path.0,
                            },
                        )
                        .collect(),
                })
            }
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SecretKeyScope> for SecretKeyScope {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SecretKeyScope,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::secret_key_scope::Value;
        match value
            .value
            .ok_or_else(|| "Missing SecretKeyScope.value".to_string())?
        {
            Value::All(_) => Ok(Self::All),
            Value::Keys(keys) => Ok(Self::Keys(
                keys.paths
                    .into_iter()
                    .map(|path| CanonicalAgentSecretPath(path.segments))
                    .collect::<BTreeSet<_>>(),
            )),
        }
    }
}

impl From<SecretKeyScope> for golem_api_grpc::proto::golem::component::SecretKeyScope {
    fn from(value: SecretKeyScope) -> Self {
        use golem_api_grpc::proto::golem::component::secret_key_scope::Value;
        let value = match value {
            SecretKeyScope::All => Value::All(golem_schema::proto::golem::common::Empty {}),
            SecretKeyScope::Keys(keys) => {
                Value::Keys(golem_api_grpc::proto::golem::component::SecretKeyPaths {
                    paths: keys
                        .into_iter()
                        .map(
                            |path| golem_api_grpc::proto::golem::component::SecretKeyPath {
                                segments: path.0,
                            },
                        )
                        .collect(),
                })
            }
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig>
    for AgentTypeProvisionConfig
{
    type Error = String;

    fn try_from(
        proto: golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig,
    ) -> Result<Self, Self::Error> {
        use crate::base_model::component::{InitialAgentFile, InstalledPlugin};
        use crate::base_model::worker::TypedAgentConfigEntry;

        let config = proto
            .config
            .into_iter()
            .map(TypedAgentConfigEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        let plugins = proto
            .plugins
            .into_iter()
            .map(InstalledPlugin::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        let files = proto
            .files
            .into_iter()
            .map(InitialAgentFile::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let initial_permission = crate::serialization::deserialize(&proto.initial_permissions)?;

        Ok(AgentTypeProvisionConfig {
            initial_permissions: initial_permission,
            env: proto.env.into_iter().collect(),
            config,
            plugins,
            files,
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ComponentProvisionConfig>
    for ComponentProvisionConfig
{
    type Error = String;
    fn try_from(
        proto: golem_api_grpc::proto::golem::component::ComponentProvisionConfig,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            initial_permissions: crate::serialization::deserialize(&proto.initial_permissions)?,
            env: proto.env.into_iter().collect(),
            config: proto
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plugins: proto
                .plugins
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            files: proto
                .files
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl TryFrom<ComponentProvisionConfig>
    for golem_api_grpc::proto::golem::component::ComponentProvisionConfig
{
    type Error = String;
    fn try_from(config: ComponentProvisionConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            initial_permissions: crate::serialization::serialize(&config.initial_permissions)
                .expect("failed to serialize component initial permission card"),
            env: config.env.into_iter().collect(),
            config: config
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plugins: config.plugins.into_iter().map(Into::into).collect(),
            files: config.files.into_iter().map(Into::into).collect(),
        })
    }
}

impl TryFrom<AgentTypeProvisionConfig>
    for golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig
{
    type Error = String;

    fn try_from(config: AgentTypeProvisionConfig) -> Result<Self, Self::Error> {
        use crate::base_model::component::{InitialAgentFile, InstalledPlugin};

        Ok(Self {
            initial_permissions: crate::serialization::serialize(&config.initial_permissions)
                .expect("failed to serialize agent initial permission card"),
            env: config.env.into_iter().collect(),
            config: config
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            plugins: config
                .plugins
                .into_iter()
                .map(|p: InstalledPlugin| {
                    golem_api_grpc::proto::golem::component::PluginInstallation::from(p)
                })
                .collect(),
            files: config
                .files
                .into_iter()
                .map(|f: InitialAgentFile| {
                    golem_api_grpc::proto::golem::component::InitialAgentFile::from(f)
                })
                .collect(),
        })
    }
}

fn legacy_component_tool_source(
    source: &ToolSource,
) -> Option<golem_api_grpc::proto::golem::registry::ComponentToolSource> {
    match source {
        ToolSource::Component {
            component_id,
            component_revision,
            component_name,
        } => Some(
            golem_api_grpc::proto::golem::registry::ComponentToolSource {
                component_id: Some((*component_id).into()),
                component_revision: (*component_revision).into(),
                component_name: component_name.0.clone(),
            },
        ),
        ToolSource::Host { .. } => None,
    }
}

impl From<ToolSource> for golem_api_grpc::proto::golem::registry::ToolSource {
    fn from(value: ToolSource) -> Self {
        Self {
            source: Some(match value {
                ToolSource::Component {
                    component_id,
                    component_revision,
                    component_name,
                } => golem_api_grpc::proto::golem::registry::tool_source::Source::Component(
                    golem_api_grpc::proto::golem::registry::ComponentToolSource {
                        component_id: Some(component_id.into()),
                        component_revision: component_revision.into(),
                        component_name: component_name.0,
                    },
                ),
                ToolSource::Host {
                    host_tool_id,
                    implementation_version,
                } => golem_api_grpc::proto::golem::registry::tool_source::Source::Host(
                    golem_api_grpc::proto::golem::registry::HostToolSource {
                        host_tool_id: host_tool_id.as_str().to_string(),
                        implementation_version,
                    },
                ),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::ComponentToolSource> for ToolSource {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::ComponentToolSource,
    ) -> Result<Self, Self::Error> {
        Ok(Self::Component {
            component_id: ComponentId::try_from(
                value
                    .component_id
                    .ok_or("missing ComponentToolSource.component_id")?,
            )?,
            component_revision: ComponentRevision::try_from(value.component_revision)?,
            component_name: ComponentName(value.component_name),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::ToolSource> for ToolSource {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::ToolSource,
    ) -> Result<Self, Self::Error> {
        match value.source.ok_or("missing ToolSource.source")? {
            golem_api_grpc::proto::golem::registry::tool_source::Source::Component(value) => {
                value.try_into()
            }
            golem_api_grpc::proto::golem::registry::tool_source::Source::Host(value) => {
                Ok(Self::Host {
                    host_tool_id: crate::model::tool::HostToolId::try_from(value.host_tool_id)?,
                    implementation_version: value.implementation_version,
                })
            }
        }
    }
}

fn tool_source_from_proto(
    tagged_source: Option<golem_api_grpc::proto::golem::registry::ToolSource>,
    legacy_source: Option<golem_api_grpc::proto::golem::registry::ComponentToolSource>,
) -> Result<ToolSource, String> {
    if let Some(source) = tagged_source {
        source.try_into()
    } else {
        legacy_source.ok_or("missing tool source")?.try_into()
    }
}

impl From<RegisteredTool> for golem_api_grpc::proto::golem::registry::RegisteredTool {
    fn from(value: RegisteredTool) -> Self {
        let source = legacy_component_tool_source(&value.source);
        let tagged_source = Some(value.source.into());
        Self {
            deployment_revision: value.deployment_revision.into(),
            definition: Some(value.definition.into()),
            provision: Some(value.provision.into()),
            source,
            owner_account_id: Some(value.owner_account_id.into()),
            owner_account_email: value.owner_account_email.into_inner(),
            metadata_version: value.metadata_version,
            tool_release_id: value.release_id.map(|id| id.0.into()),
            metadata_digest: Some(value.metadata_digest.into()),
            tagged_source,
            component_bindings: value
                .component_bindings
                .into_iter()
                .map(|(name, binding)| (name.0, binding.into()))
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredTool> for RegisteredTool {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredTool,
    ) -> Result<Self, Self::Error> {
        let definition: crate::schema::tool::Tool = value
            .definition
            .ok_or("missing RegisteredTool.definition")?
            .try_into()?;
        let metadata_version = value.metadata_version;
        let metadata_digest = match value.metadata_digest {
            Some(metadata_digest) => metadata_digest.try_into()?,
            None => {
                crate::model::tool_release::tool_metadata_digest(&metadata_version, &definition)
                    .map_err(|error| format!("failed to derive tool metadata digest: {error}"))?
            }
        };
        Ok(Self {
            deployment_revision: DeploymentRevision::try_from(value.deployment_revision)?,
            definition,
            provision: value
                .provision
                .ok_or("missing RegisteredTool.provision")?
                .try_into()?,
            component_bindings: value
                .component_bindings
                .into_iter()
                .map(|(name, binding)| {
                    Ok((ComponentName(name), ToolBindingInput::try_from(binding)?))
                })
                .collect::<Result<_, String>>()?,
            source: tool_source_from_proto(value.tagged_source, value.source)?,
            owner_account_id: AccountId::try_from(
                value
                    .owner_account_id
                    .ok_or("missing RegisteredTool.owner_account_id")?,
            )?,
            owner_account_email: AccountEmail::new(value.owner_account_email),
            metadata_version,
            release_id: value
                .tool_release_id
                .map(uuid::Uuid::from)
                .map(crate::model::tool_release::ToolReleaseId),
            metadata_digest,
        })
    }
}

impl From<CompiledToolBinding> for golem_api_grpc::proto::golem::registry::CompiledToolBinding {
    fn from(value: CompiledToolBinding) -> Self {
        let source = legacy_component_tool_source(&value.source);
        let tagged_source = Some(value.source.into());
        Self {
            deployment_revision: value.deployment_revision.into(),
            owner: Some(match value.owner {
                ToolBindingOwner::AgentType { agent_type_name } =>
                    golem_api_grpc::proto::golem::registry::compiled_tool_binding::Owner::AgentTypeName(agent_type_name.0),
                ToolBindingOwner::ComponentBaseline { component_id } =>
                    golem_api_grpc::proto::golem::registry::compiled_tool_binding::Owner::ComponentBaselineId(component_id.into()),
            }),
            tool_name: value.tool_name.into_inner(),
            version: value.version,
            metadata_version: value.metadata_version,
            account_id: Some(value.account_id.into()),
            account_email: value.account_email.into_inner(),
            parameters_json: value.parameters.to_string(),
            config_keys_readable: Some(value.config_keys_readable.into()),
            secret_keys_readable: Some(value.secret_keys_readable.into()),
            secret_keys_revealable: Some(value.secret_keys_revealable.into()),
            source,
            filesystem_access:
                golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::from(
                    value.filesystem_access,
                ) as i32,
            tool_release_id: value.release_id.map(|id| id.0.into()),
            metadata_digest: Some(value.metadata_digest.into()),
            tagged_source,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::CompiledToolBinding> for CompiledToolBinding {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::CompiledToolBinding,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            deployment_revision: DeploymentRevision::try_from(value.deployment_revision)?,
            owner: match value.owner.ok_or("missing CompiledToolBinding.owner")? {
                golem_api_grpc::proto::golem::registry::compiled_tool_binding::Owner::AgentTypeName(agent_type_name) =>
                    ToolBindingOwner::AgentType { agent_type_name: AgentTypeName(agent_type_name) },
                golem_api_grpc::proto::golem::registry::compiled_tool_binding::Owner::ComponentBaselineId(component_id) =>
                    ToolBindingOwner::ComponentBaseline { component_id: component_id.try_into()? },
            },
            tool_name: ToolName::try_from(value.tool_name)?,
            version: value.version,
            metadata_version: value.metadata_version,
            release_id: value
                .tool_release_id
                .map(uuid::Uuid::from)
                .map(crate::model::tool_release::ToolReleaseId),
            metadata_digest: value
                .metadata_digest
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
            account_id: AccountId::try_from(
                value
                    .account_id
                    .ok_or("missing CompiledToolBinding.account_id")?,
            )?,
            account_email: AccountEmail::new(value.account_email),
            parameters: NormalizedJsonValue::new(
                serde_json::from_str(&value.parameters_json)
                    .map_err(|error| format!("invalid tool binding parameters: {error}"))?,
            ),
            config_keys_readable: value
                .config_keys_readable
                .ok_or("missing CompiledToolBinding.config_keys_readable")?
                .try_into()?,
            secret_keys_readable: value
                .secret_keys_readable
                .ok_or("missing CompiledToolBinding.secret_keys_readable")?
                .try_into()?,
            secret_keys_revealable: value
                .secret_keys_revealable
                .ok_or("missing CompiledToolBinding.secret_keys_revealable")?
                .try_into()?,
            filesystem_access:
                golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::try_from(
                    value.filesystem_access,
                )
                .map_err(|error| error.to_string())?
                .into(),
            source: tool_source_from_proto(value.tagged_source, value.source)?,
        })
    }
}

impl From<crate::model::tool::ToolFilesystemAccess>
    for golem_api_grpc::proto::golem::registry::ToolFilesystemAccess
{
    fn from(value: crate::model::tool::ToolFilesystemAccess) -> Self {
        match value {
            crate::model::tool::ToolFilesystemAccess::Unset => Self::Unset,
            crate::model::tool::ToolFilesystemAccess::Allowed => Self::Allowed,
            crate::model::tool::ToolFilesystemAccess::Denied => Self::Denied,
        }
    }
}

impl From<golem_api_grpc::proto::golem::registry::ToolFilesystemAccess>
    for crate::model::tool::ToolFilesystemAccess
{
    fn from(value: golem_api_grpc::proto::golem::registry::ToolFilesystemAccess) -> Self {
        match value {
            golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::Unset => Self::Unset,
            golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::Allowed => Self::Allowed,
            golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::Denied => Self::Denied,
        }
    }
}

impl From<ToolDeploymentState> for golem_api_grpc::proto::golem::registry::ToolDeploymentState {
    fn from(value: ToolDeploymentState) -> Self {
        Self {
            deployment_revision: value.deployment_revision.into(),
            registered_tools: value
                .registered_tools
                .into_values()
                .map(Into::into)
                .collect(),
            tool_bindings: value
                .tool_bindings
                .into_values()
                .flat_map(BTreeMap::into_values)
                .map(Into::into)
                .collect(),
            mcp_imports: value
                .mcp_imports
                .into_iter()
                .map(|import| golem_api_grpc::proto::golem::registry::McpImport {
                    url: import.url,
                    auth: import.auth.map(|auth| {
                        golem_api_grpc::proto::golem::registry::McpImportAuth {
                            kind: match auth.kind {
                                crate::model::mcp_import::McpInlineCredentialKind::Bearer => golem_api_grpc::proto::golem::registry::McpInlineCredentialKind::Bearer.into(),
                                crate::model::mcp_import::McpInlineCredentialKind::Basic => golem_api_grpc::proto::golem::registry::McpInlineCredentialKind::Basic.into(),
                            },
                            credential_digest: Some(auth.credential_digest.into()),
                        }
                    }),
                    security_scheme: import.security_scheme.map(|name| name.0),
                    prefix: import.prefix,
                    has_include: import.include.is_some(),
                    include: import.include.unwrap_or_default(),
                    has_exclude: import.exclude.is_some(),
                    exclude: import.exclude.unwrap_or_default(),
                    version: import.version,
                })
                .collect(),
            registered_tool_middlewares: value
                .registered_tool_middlewares
                .into_values()
                .map(|middleware| {
                    desert_rust::serialize_to_byte_vec(&middleware)
                        .expect("middleware snapshot serialization must succeed")
                })
                .collect(),
            tool_middleware_chains: value
                .tool_middleware_chains
                .into_values()
                .flat_map(BTreeMap::into_values)
                .map(|chain| {
                    desert_rust::serialize_to_byte_vec(&chain)
                        .expect("middleware chain snapshot serialization must succeed")
                })
                .collect(),
            tool_middleware_configuration: desert_rust::serialize_to_byte_vec(
                &value.tool_middleware_configuration,
            )
            .expect("middleware configuration snapshot serialization must succeed"),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::ToolDeploymentState> for ToolDeploymentState {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::ToolDeploymentState,
    ) -> Result<Self, Self::Error> {
        let deployment_revision = DeploymentRevision::try_from(value.deployment_revision)?;
        let tool_middleware_configuration =
            desert_rust::deserialize(&value.tool_middleware_configuration)
                .map_err(|error| format!("invalid middleware configuration snapshot: {error}"))?;
        let mut registered_tools = BTreeMap::new();
        for proto in value.registered_tools {
            let registered: RegisteredTool = proto.try_into()?;
            if registered.deployment_revision != deployment_revision {
                return Err(format!(
                    "registered tool deployment revision {} does not match snapshot revision {}",
                    registered.deployment_revision, deployment_revision
                ));
            }
            let name = ToolName::try_from(
                registered
                    .definition
                    .name()
                    .ok_or("registered tool definition has no name")?,
            )?;
            if registered_tools.insert(name.clone(), registered).is_some() {
                return Err(format!("duplicate registered tool {name}"));
            }
        }
        let mut tool_bindings = BTreeMap::new();
        for proto in value.tool_bindings {
            let metadata_digest_missing = proto.metadata_digest.is_none();
            let tool_name = ToolName::try_from(proto.tool_name.as_str())?;
            let registered = registered_tools.get(&tool_name).ok_or_else(|| {
                format!("compiled binding references unregistered tool {tool_name}")
            })?;
            let mut binding: CompiledToolBinding = proto.try_into()?;
            if metadata_digest_missing {
                binding.metadata_digest = registered.metadata_digest;
            }
            if binding.deployment_revision != deployment_revision {
                return Err(format!(
                    "compiled tool binding deployment revision {} does not match snapshot revision {}",
                    binding.deployment_revision, deployment_revision
                ));
            }
            if binding.source != registered.source
                || binding.version != registered.definition.version
                || binding.metadata_version != registered.metadata_version
                || binding.release_id != registered.release_id
                || binding.metadata_digest != registered.metadata_digest
                || binding.account_id != registered.owner_account_id
                || binding.account_email != registered.owner_account_email
            {
                return Err(format!(
                    "compiled binding for tool {} does not match the registered implementation",
                    binding.tool_name
                ));
            }
            if !binding
                .secret_keys_revealable
                .is_subset_of(&binding.secret_keys_readable)
            {
                return Err(format!(
                    "compiled binding for tool {} has revealable secret keys outside its readable scope",
                    binding.tool_name
                ));
            }
            if tool_bindings
                .entry(binding.owner.clone())
                .or_insert_with(BTreeMap::new)
                .insert(binding.tool_name.clone(), binding)
                .is_some()
            {
                return Err("duplicate compiled agent tool binding".to_string());
            }
        }
        let mut registered_tool_middlewares = BTreeMap::new();
        for bytes in value.registered_tool_middlewares {
            let middleware: crate::model::tool_middleware::RegisteredToolMiddleware =
                desert_rust::deserialize(&bytes)
                    .map_err(|error| format!("invalid registered middleware snapshot: {error}"))?;
            if middleware.deployment_revision != deployment_revision {
                return Err(
                    "registered middleware belongs to another deployment revision".to_string(),
                );
            }
            let name = crate::model::tool_middleware::ToolMiddlewareName::try_from(
                middleware.definition.name.clone(),
            )?;
            if middleware.metadata_digest
                != crate::model::tool_middleware_release::tool_middleware_metadata_digest(
                    &middleware.metadata_version,
                    &middleware.definition,
                )
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "registered middleware {name} has an invalid metadata digest"
                ));
            }
            if registered_tool_middlewares
                .insert(name.clone(), middleware)
                .is_some()
            {
                return Err(format!("duplicate registered middleware {name}"));
            }
        }
        let mut tool_middleware_chains = BTreeMap::new();
        for bytes in value.tool_middleware_chains {
            let chain: crate::model::tool_middleware::CompiledToolMiddlewareChain =
                desert_rust::deserialize(&bytes)
                    .map_err(|error| format!("invalid middleware chain snapshot: {error}"))?;
            if chain.deployment_revision != deployment_revision {
                return Err("middleware chain belongs to another deployment revision".to_string());
            }
            let binding = tool_bindings
                .get(&chain.owner)
                .and_then(|bindings| bindings.get(&chain.tool_name))
                .ok_or_else(|| {
                    format!(
                        "middleware chain for tool {} has no matching compiled binding",
                        chain.tool_name
                    )
                })?;
            for occurrence in &chain.occurrences {
                let name = crate::model::tool_middleware::ToolMiddlewareName::try_from(
                    occurrence.middleware.definition.name.clone(),
                )?;
                let registered = registered_tool_middlewares.get(&name).ok_or_else(|| {
                    format!("middleware chain references unregistered middleware {name}")
                })?;
                if registered != &occurrence.middleware
                    || !occurrence
                        .secret_keys_revealable
                        .is_subset_of(&occurrence.secret_keys_readable)
                    || !occurrence
                        .secret_keys_readable
                        .is_subset_of(&binding.secret_keys_readable)
                    || !occurrence
                        .secret_keys_revealable
                        .is_subset_of(&binding.secret_keys_revealable)
                {
                    return Err(format!(
                        "middleware occurrence {name} does not match its registration or policy"
                    ));
                }
            }
            if tool_middleware_chains
                .entry(chain.owner.clone())
                .or_insert_with(BTreeMap::new)
                .insert(chain.tool_name.clone(), chain)
                .is_some()
            {
                return Err("duplicate compiled middleware chain".to_string());
            }
        }
        Ok(Self {
            deployment_revision,
            registered_tools,
            tool_bindings,
            mcp_imports: value
                .mcp_imports
                .into_iter()
                .map(|import| {
                    Ok(crate::model::mcp_import::McpImport {
                        url: import.url,
                        auth: import.auth.map(|auth| {
                            let kind = match golem_api_grpc::proto::golem::registry::McpInlineCredentialKind::try_from(auth.kind).map_err(|_| "invalid MCP inline credential kind")? {
                                golem_api_grpc::proto::golem::registry::McpInlineCredentialKind::Bearer => crate::model::mcp_import::McpInlineCredentialKind::Bearer,
                                golem_api_grpc::proto::golem::registry::McpInlineCredentialKind::Basic => crate::model::mcp_import::McpInlineCredentialKind::Basic,
                                golem_api_grpc::proto::golem::registry::McpInlineCredentialKind::Unspecified => return Err("missing MCP inline credential kind".to_string()),
                            };
                            Ok(crate::model::mcp_import::McpImportAuth {
                                kind,
                                credential_digest: auth.credential_digest.ok_or("missing MCP credential digest")?.try_into()?,
                            })
                        }).transpose()?,
                        security_scheme: import.security_scheme.map(crate::model::security_scheme::SecuritySchemeName::try_from).transpose()?,
                        prefix: import.prefix,
                        include: import.has_include.then_some(import.include),
                        exclude: import.has_exclude.then_some(import.exclude),
                        version: import.version,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            tool_middleware_configuration,
            registered_tool_middlewares,
            tool_middleware_chains,
        })
    }
}
