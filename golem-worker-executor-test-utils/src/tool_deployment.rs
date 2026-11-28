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

use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentDto, ComponentId, ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::tool::{
    CompiledToolBinding, ConfigKeyScope, RegisteredTool, SecretKeyScope, ToolBindingOwner,
    ToolDeploymentState, ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolRegistration,
    ToolSource,
};
use golem_common::model::type_id::TypeId;
use golem_common::schema::tool::Tool;
use std::collections::{BTreeMap, BTreeSet};

/// Builds the `ToolDeploymentState` of one provider component: it registers each tool
/// definition of the provider and binds every registered tool to one agent type.
///
/// ```ignore
/// let deployment = ToolDeployment::new(context.account_id, &provider_component)
///     .tools(metadata.tools)
///     .bound_to("ToolStreamingCaller")
///     .build();
/// ```
pub struct ToolDeployment {
    account_id: AccountId,
    account_email: AccountEmail,
    provider_id: ComponentId,
    provider_revision: ComponentRevision,
    provider_name: ComponentName,
    deployment_revision: u64,
    definitions: Vec<Tool>,
    agent_type: Option<String>,
    provision_env: BTreeMap<String, String>,
    config_keys_readable: ConfigKeyScope,
    filesystem_access: BTreeMap<String, ToolFilesystemAccess>,
    shell: BTreeSet<String>,
}

impl ToolDeployment {
    /// A deployment of `provider`'s tools, owned by `account_id` (`test@golem`), at deployment
    /// revision 1.
    pub fn new(account_id: AccountId, provider: &ComponentDto) -> Self {
        Self {
            account_id,
            account_email: AccountEmail::new("test@golem"),
            provider_id: provider.id,
            provider_revision: provider.revision,
            provider_name: provider.component_name.clone(),
            deployment_revision: 1,
            definitions: Vec::new(),
            agent_type: None,
            provision_env: BTreeMap::new(),
            config_keys_readable: ConfigKeyScope::default(),
            filesystem_access: BTreeMap::new(),
            shell: BTreeSet::new(),
        }
    }

    /// Registers each definition, named by its root command, with type IDs 1, 2, … in order.
    pub fn tools(mut self, definitions: Vec<Tool>) -> Self {
        self.definitions = definitions;
        self
    }

    /// Binds every registered tool to `agent_type`.
    pub fn bound_to(mut self, agent_type: &str) -> Self {
        self.agent_type = Some(agent_type.to_string());
        self
    }

    /// The provider name that the registrations record, when it differs from the stored
    /// component's name.
    pub fn provider_name(mut self, name: &str) -> Self {
        self.provider_name = ComponentName(name.to_string());
        self
    }

    pub fn account_email(mut self, email: &str) -> Self {
        self.account_email = AccountEmail::new(email);
        self
    }

    pub fn deployment_revision(mut self, revision: u64) -> Self {
        self.deployment_revision = revision;
        self
    }

    /// The provision environment of every registered tool.
    pub fn provision_env<'a>(mut self, env: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        self.provision_env = env
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        self
    }

    /// The config keys every binding may read.
    pub fn config_keys_readable(mut self, scope: ConfigKeyScope) -> Self {
        self.config_keys_readable = scope;
        self
    }

    /// The filesystem access of `tool`'s binding, when the deployment registers `tool`.
    pub fn filesystem_access(mut self, tool: &str, access: ToolFilesystemAccess) -> Self {
        self.filesystem_access.insert(tool.to_string(), access);
        self
    }

    /// Marks `tool`'s binding as the shell, when the deployment registers `tool`.
    pub fn shell(mut self, tool: &str) -> Self {
        self.shell.insert(tool.to_string());
        self
    }

    pub fn build(self) -> ToolDeploymentState {
        let deployment_revision = DeploymentRevision::try_from(self.deployment_revision)
            .expect("valid deployment revision");
        let agent_type = AgentTypeName(
            self.agent_type
                .expect("ToolDeployment::bound_to names the agent type"),
        );
        let provision = ToolProvisionConfig {
            env: self.provision_env,
            ..Default::default()
        };
        let registered_tools = self
            .definitions
            .into_iter()
            .enumerate()
            .map(|(index, definition)| {
                let root_name = definition
                    .commands
                    .nodes
                    .first()
                    .expect("tool definition has a root command")
                    .name
                    .clone();
                let name = ToolName::try_from(root_name).expect("valid discovered tool name");
                let tool = RegisteredTool {
                    type_id: TypeId::try_from(u32::try_from(index + 1).unwrap()).unwrap(),
                    registration: ToolRegistration {
                        deployment_revision,
                        release_id: None,
                        metadata_digest: Default::default(),
                        definition,
                        provision: provision.clone(),
                        component_bindings: Default::default(),
                        source: ToolSource::Component {
                            component_id: self.provider_id,
                            component_revision: self.provider_revision,
                            component_name: self.provider_name.clone(),
                        },
                        owner_account_id: self.account_id,
                        owner_account_email: self.account_email.clone(),
                        metadata_version: "0.1.0".to_string(),
                    },
                };
                (name, tool)
            })
            .collect::<BTreeMap<_, _>>();
        let owner = ToolBindingOwner::AgentType {
            agent_type_name: agent_type,
        };
        let bindings = registered_tools
            .iter()
            .map(|(name, tool)| {
                let binding = CompiledToolBinding {
                    deployment_revision,
                    release_id: tool.registration.release_id,
                    metadata_digest: tool.registration.metadata_digest,
                    owner: owner.clone(),
                    tool_name: name.clone(),
                    version: tool.registration.definition.version.clone(),
                    metadata_version: tool.registration.metadata_version.clone(),
                    account_id: self.account_id,
                    account_email: self.account_email.clone(),
                    parameters: NormalizedJsonValue::new(serde_json::json!({})),
                    config_keys_readable: self.config_keys_readable.clone(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: self
                        .filesystem_access
                        .get(name.as_str())
                        .cloned()
                        .unwrap_or(ToolFilesystemAccess::Unset),
                    source: tool.registration.source.clone(),
                    shell: self.shell.contains(name.as_str()),
                };
                (name.clone(), binding)
            })
            .collect();
        ToolDeploymentState {
            deployment_revision,
            registered_tools,
            tool_bindings: BTreeMap::from([(owner, bindings)]),
            mcp_imports: Vec::new(),
            tool_middleware_configuration: Default::default(),
            registered_tool_middlewares: BTreeMap::new(),
            tool_middleware_chains: BTreeMap::new(),
        }
    }
}
