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
    AgentResourcePattern, AgentVerb, BlobPermissionPattern, BlobResourcePattern, BlobVerb,
    CardPermissionPattern, CardResourcePattern, CardVerb, ConfigPermissionPattern,
    ConfigResourcePattern, ConfigVerb, EmptyOwnerPattern, EnvPermissionPattern, EnvResourcePattern,
    EnvVerb, EnvironmentOwnerPattern, FilesystemPermissionPattern, FilesystemResourcePattern,
    FilesystemVerb, KvPermissionPattern, KvResourcePattern, KvVerb, NetworkPermissionPattern,
    NetworkResourcePattern, NetworkVerb, OplogPermissionPattern, OplogResourcePattern, OplogVerb,
    PermissionPattern, PolymorphicPermissionPattern, PortPattern, RdbmsPermissionPattern,
    RdbmsResourcePattern, RdbmsVerb, SecretPermissionPattern, SecretResourcePattern, SecretVerb,
    ToolOwnerPattern, ToolPermissionPattern, ToolResourcePattern, ToolVerb,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
        Self::filesystem_read_pattern(owner, recipient, FilesystemResourcePattern::exact(path))
    }

    pub fn filesystem_read_pattern(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        resource: FilesystemResourcePattern,
    ) -> Self {
        Self::new(PermissionPattern::Filesystem(
            FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Read,
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
            FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Write,
                owner: owner.into(),
                recipient: recipient.into(),
                resource: FilesystemResourcePattern::exact(path),
            },
        ))
    }

    pub fn network_connect(
        recipient: impl Into<AgentRecipientPattern>,
        host: impl Into<String>,
        ports: PortPattern,
    ) -> Self {
        Self::new(PermissionPattern::Network(NetworkPermissionPattern::Verb {
            verb: NetworkVerb::Connect,
            owner: EmptyOwnerPattern,
            recipient: recipient.into(),
            resource: NetworkResourcePattern::host_port(host, ports),
        }))
    }

    pub fn env_read(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        name: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Env(EnvPermissionPattern::Verb {
            verb: EnvVerb::Read,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: EnvResourcePattern::exact(name),
        }))
    }

    pub fn oplog_read(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        resource: OplogResourcePattern,
    ) -> Self {
        Self::new(PermissionPattern::Oplog(OplogPermissionPattern::Verb {
            verb: OplogVerb::Read,
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
        Self::new(PermissionPattern::Config(ConfigPermissionPattern::Verb {
            verb: ConfigVerb::Read,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: ConfigResourcePattern::exact(key),
        }))
    }

    pub fn secret_reveal(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Secret(SecretPermissionPattern::Verb {
            verb: SecretVerb::Reveal,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: SecretResourcePattern::exact(key),
        }))
    }

    pub fn agent_invoke(
        owner: impl Into<AgentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        method: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Agent(AgentPermissionPattern::Verb {
            verb: AgentVerb::Invoke,
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
        Self::new(PermissionPattern::Tool(ToolPermissionPattern::Verb {
            verb: ToolVerb::Invoke,
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
        Self::new(PermissionPattern::Kv(KvPermissionPattern::Verb {
            verb: KvVerb::Read,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: KvResourcePattern::exact(key),
        }))
    }

    pub fn blob_read(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Blob(BlobPermissionPattern::Verb {
            verb: BlobVerb::Read,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: BlobResourcePattern::exact(key),
        }))
    }

    pub fn rdbms_query(
        owner: impl Into<EnvironmentOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        query_target: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Rdbms(RdbmsPermissionPattern::Verb {
            verb: RdbmsVerb::Query,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: RdbmsResourcePattern::exact(query_target),
        }))
    }

    pub fn card_install(
        owner: impl Into<AccountOwnerPattern>,
        recipient: impl Into<AgentRecipientPattern>,
        target: AgentRecipientPattern,
    ) -> Self {
        Self::new(PermissionPattern::Card(CardPermissionPattern::Verb {
            verb: CardVerb::Install,
            owner: owner.into(),
            recipient: recipient.into(),
            resource: CardResourcePattern::InstallTarget(target),
        }))
    }
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
