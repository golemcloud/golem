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

use crate::base_model::card::{
    AccountOwnerPattern, AgentOwnerPattern, AgentPermissionPattern, AgentRecipientPattern,
    AgentResourcePattern, BlobPermissionPattern, CardPermissionPattern, CardResourcePattern,
    ConfigPermissionPattern, EmptyOwnerPattern, EnvPermissionPattern, EnvironmentOwnerPattern,
    FilesystemPermissionPattern, GlobResourcePattern, IdentifierResourcePattern,
    KvPermissionPattern, NetworkPermissionPattern, NetworkResourcePattern, OplogPermissionPattern,
    OplogResourcePattern, PermissionPattern, PolymorphicPermissionPattern, PortPattern,
    RdbmsPermissionPattern, RecipientPathPattern, SecretPermissionPattern, ToolOwnerPattern,
    ToolPermissionPattern, ToolResourcePattern,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct SlotVariable(pub String);

impl SlotVariable {
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some(name) = value.strip_prefix('?') else {
            return Err(value.to_string());
        };
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(value.to_string());
        }
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PatternGrant {
    pub permission: PermissionPattern,
}

impl PatternGrant {
    pub fn new(permission: PermissionPattern) -> Self {
        Self { permission }
    }

    pub fn filesystem_read(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        path: impl Into<String>,
    ) -> Self {
        Self::filesystem_read_pattern(owner, recipient, GlobResourcePattern::exact(path))
    }

    pub fn filesystem_read_pattern(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        resource: GlobResourcePattern,
    ) -> Self {
        Self::new(PermissionPattern::Filesystem(
            FilesystemPermissionPattern::Read {
                owner: owner.into(),
                recipient: recipient.into(),
                resource,
            },
        ))
    }

    pub fn filesystem_write(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        path: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Filesystem(
            FilesystemPermissionPattern::Write {
                owner: owner.into(),
                recipient: recipient.into(),
                resource: GlobResourcePattern::exact(path),
            },
        ))
    }

    pub fn network_connect(
        recipient: impl Into<AgentRecipientPattern>,
        host: impl Into<String>,
        ports: PortPattern,
    ) -> Self {
        Self::new(PermissionPattern::Network(
            NetworkPermissionPattern::Connect {
                owner: EmptyOwnerPattern,
                recipient: recipient.into(),
                resource: NetworkResourcePattern::host_port(host, ports),
            },
        ))
    }

    pub fn env_read(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        name: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Env(EnvPermissionPattern::Read {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: IdentifierResourcePattern::exact(name),
        }))
    }

    pub fn oplog_read(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        resource: OplogResourcePattern,
    ) -> Self {
        Self::new(PermissionPattern::Oplog(OplogPermissionPattern::Read {
            owner: owner.into(),
            recipient: recipient.into(),
            resource,
        }))
    }

    pub fn config_read(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Config(ConfigPermissionPattern::Read {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: GlobResourcePattern::exact(key),
        }))
    }

    pub fn secret_reveal(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Secret(SecretPermissionPattern::Reveal {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: GlobResourcePattern::exact(key),
        }))
    }

    pub fn agent_invoke(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        method: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Agent(AgentPermissionPattern::Invoke {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: AgentResourcePattern::method(method),
        }))
    }

    pub fn tool_invoke(
        owner: impl Into<ToolOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        command: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Tool(ToolPermissionPattern::Invoke {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: ToolResourcePattern::command(command),
        }))
    }

    pub fn kv_read(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Kv(KvPermissionPattern::Read {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: GlobResourcePattern::exact(key),
        }))
    }

    pub fn blob_read(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Blob(BlobPermissionPattern::Read {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: GlobResourcePattern::exact(key),
        }))
    }

    pub fn rdbms_query(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        query_target: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Rdbms(RdbmsPermissionPattern::Query {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: GlobResourcePattern::exact(query_target),
        }))
    }

    pub fn card_install(
        owner: impl Into<AccountOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        target: RecipientPathPattern,
    ) -> Self {
        Self::new(PermissionPattern::Card(CardPermissionPattern::Install {
            owner: owner.into(),
            recipient: recipient.into(),
            resource: CardResourcePattern::InstallTarget(target),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicRecipientPathPattern {
    Concrete(RecipientPathPattern),
    Slot(SlotVariable),
    Template(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PolymorphicPatternGrant {
    pub permission: PolymorphicPermissionPattern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
    pub polymorphic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolymorphicCard {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PolymorphicPatternGrant>,
    pub lower_negative: Vec<PolymorphicPatternGrant>,
    pub upper_positive: Vec<PolymorphicPatternGrant>,
    pub upper_negative: Vec<PolymorphicPatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
}
