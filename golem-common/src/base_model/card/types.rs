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
    AgentPermissionPattern, AgentResourcePattern, BlobPermissionPattern, CardPermissionPattern,
    CardResourcePattern, ConfigPermissionPattern, EnvPermissionPattern,
    FilesystemPermissionPattern, GlobResourcePattern, IdentifierResourcePattern,
    KvPermissionPattern, NetworkPermissionPattern, NetworkResourcePattern, OplogPermissionPattern,
    OplogResourcePattern, PermissionPattern, PolymorphicPermissionPattern, PortPattern,
    RdbmsPermissionPattern, RecipientPathPattern, SecretPermissionPattern, ToolPermissionPattern,
    ToolResourcePattern,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct OwnerPathPattern(pub String);

impl OwnerPathPattern {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }
}

impl From<String> for OwnerPathPattern {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for OwnerPathPattern {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

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
    pub owner: OwnerPathPattern,
    pub recipient: RecipientPathPattern,
    pub permission: PermissionPattern,
}

impl PatternGrant {
    pub fn new(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        permission: PermissionPattern,
    ) -> Self {
        Self {
            owner: owner.into(),
            recipient,
            permission,
        }
    }

    pub fn filesystem_read(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        path: impl Into<String>,
    ) -> Self {
        Self::filesystem_read_pattern(owner, recipient, GlobResourcePattern::exact(path))
    }

    pub fn filesystem_read_pattern(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        resource: GlobResourcePattern,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(resource)),
        )
    }

    pub fn filesystem_write(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        path: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Filesystem(FilesystemPermissionPattern::Write(
                GlobResourcePattern::exact(path),
            )),
        )
    }

    pub fn network_connect(
        recipient: RecipientPathPattern,
        host: impl Into<String>,
        ports: PortPattern,
    ) -> Self {
        Self::new(
            OwnerPathPattern::new(String::new()),
            recipient,
            PermissionPattern::Network(NetworkPermissionPattern::Connect(
                NetworkResourcePattern::host_port(host, ports),
            )),
        )
    }

    pub fn env_read(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        name: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Env(EnvPermissionPattern::Read(
                IdentifierResourcePattern::exact(name),
            )),
        )
    }

    pub fn oplog_read(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        resource: OplogResourcePattern,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Oplog(OplogPermissionPattern::Read(resource)),
        )
    }

    pub fn config_read(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Config(ConfigPermissionPattern::Read(GlobResourcePattern::exact(
                key,
            ))),
        )
    }

    pub fn secret_reveal(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Secret(SecretPermissionPattern::Reveal(GlobResourcePattern::exact(
                key,
            ))),
        )
    }

    pub fn agent_invoke(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        method: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Agent(AgentPermissionPattern::Invoke(
                AgentResourcePattern::method(method),
            )),
        )
    }

    pub fn tool_invoke(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        command: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Tool(ToolPermissionPattern::Invoke(ToolResourcePattern::command(
                command,
            ))),
        )
    }

    pub fn kv_read(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Kv(KvPermissionPattern::Read(GlobResourcePattern::exact(key))),
        )
    }

    pub fn blob_read(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Blob(BlobPermissionPattern::Read(GlobResourcePattern::exact(key))),
        )
    }

    pub fn rdbms_query(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        query_target: impl Into<String>,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Rdbms(RdbmsPermissionPattern::Query(GlobResourcePattern::exact(
                query_target,
            ))),
        )
    }

    pub fn card_install(
        owner: impl Into<OwnerPathPattern>,
        recipient: RecipientPathPattern,
        target: RecipientPathPattern,
    ) -> Self {
        Self::new(
            owner,
            recipient,
            PermissionPattern::Card(CardPermissionPattern::Install(
                CardResourcePattern::InstallTarget(target),
            )),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicOwnerPathPattern {
    Concrete(OwnerPathPattern),
    Slot(SlotVariable),
    Template(String),
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
    pub owner: PolymorphicOwnerPathPattern,
    pub recipient: PolymorphicRecipientPathPattern,
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
