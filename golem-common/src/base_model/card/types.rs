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

use super::class::*;
use super::{
    PermissionPattern, PolymorphicManifestPermissionPattern, PolymorphicPermissionPattern,
};
use crate::model::card::owner::{
    AccountOwnerPattern, AgentOwnerPattern, EmptyOwnerPattern, EnvironmentOwnerPattern,
    ToolOwnerPattern,
};
use crate::model::card::recipient::RecipientPattern;
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
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        path: impl Into<String>,
    ) -> Self {
        Self::filesystem_read_pattern(owner, recipient, FilesystemResourcePattern::exact(path))
    }

    pub fn filesystem_read_pattern(
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        resource: FilesystemResourcePattern,
    ) -> Self {
        Self::new(PermissionPattern::Filesystem(ClassPermissionPattern::<
            FilesystemClass,
        >::Verb {
            verb: FilesystemVerb::Read,
            owner,
            recipient,
            resource,
        }))
    }

    pub fn filesystem_write(
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        path: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Filesystem(ClassPermissionPattern::<
            FilesystemClass,
        >::Verb {
            verb: FilesystemVerb::Write,
            owner,
            recipient,
            resource: FilesystemResourcePattern::exact(path),
        }))
    }

    pub fn network_connect(
        recipient: RecipientPattern,
        host: impl Into<String>,
        ports: PortPattern,
    ) -> Self {
        Self::new(PermissionPattern::Network(ClassPermissionPattern::<
            NetworkClass,
        >::Verb {
            verb: NetworkVerb::Connect,
            owner: EmptyOwnerPattern,
            recipient,
            resource: NetworkResourcePattern::host_port(host, ports),
        }))
    }

    pub fn env_read(
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        name: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Env(
            ClassPermissionPattern::<EnvClass>::Verb {
                verb: EnvVerb::Read,
                owner,
                recipient,
                resource: EnvResourcePattern::exact(name),
            },
        ))
    }

    pub fn oplog_read(
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        resource: OplogResourcePattern,
    ) -> Self {
        Self::new(PermissionPattern::Oplog(ClassPermissionPattern::<
            OplogClass,
        >::Verb {
            verb: OplogVerb::Read,
            owner,
            recipient,
            resource,
        }))
    }

    pub fn config_read(
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Config(ClassPermissionPattern::<
            ConfigClass,
        >::Verb {
            verb: ConfigVerb::Read,
            owner,
            recipient,
            resource: ConfigResourcePattern::exact(key),
        }))
    }

    pub fn secret_reveal(
        owner: EnvironmentOwnerPattern,
        recipient: RecipientPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Secret(ClassPermissionPattern::<
            SecretClass,
        >::Verb {
            verb: SecretVerb::Reveal,
            owner,
            recipient,
            resource: SecretResourcePattern::exact(key),
        }))
    }

    pub fn agent_invoke(
        owner: AgentOwnerPattern,
        recipient: RecipientPattern,
        method: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Agent(ClassPermissionPattern::<
            AgentClass,
        >::Verb {
            verb: AgentVerb::Invoke,
            owner,
            recipient,
            resource: AgentResourcePattern::method(method),
        }))
    }

    pub fn tool_invoke(
        owner: ToolOwnerPattern,
        recipient: RecipientPattern,
        command: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Tool(
            ClassPermissionPattern::<ToolClass>::Verb {
                verb: ToolVerb::Invoke,
                owner,
                recipient,
                resource: ToolResourcePattern::command(command),
            },
        ))
    }

    pub fn kv_read(
        owner: EnvironmentOwnerPattern,
        recipient: RecipientPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Kv(
            ClassPermissionPattern::<KvClass>::Verb {
                verb: KvVerb::Read,
                owner,
                recipient,
                resource: KvResourcePattern::exact(key),
            },
        ))
    }

    pub fn blob_read(
        owner: EnvironmentOwnerPattern,
        recipient: RecipientPattern,
        key: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Blob(
            ClassPermissionPattern::<BlobClass>::Verb {
                verb: BlobVerb::Read,
                owner,
                recipient,
                resource: BlobResourcePattern::exact(key),
            },
        ))
    }

    pub fn rdbms_query(
        owner: EnvironmentOwnerPattern,
        recipient: RecipientPattern,
        query_target: impl Into<String>,
    ) -> Self {
        Self::new(PermissionPattern::Rdbms(ClassPermissionPattern::<
            RdbmsClass,
        >::Verb {
            verb: RdbmsVerb::Query,
            owner,
            recipient,
            resource: RdbmsResourcePattern::exact(query_target),
        }))
    }

    pub fn card_install(
        owner: AccountOwnerPattern,
        recipient: RecipientPattern,
        target: RecipientPattern,
    ) -> Self {
        Self::new(PermissionPattern::Card(
            ClassPermissionPattern::<CardClass>::Verb {
                verb: CardVerb::Install,
                owner,
                recipient,
                resource: CardResourcePattern::InstallTarget(target),
            },
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PolymorphicPatternGrant {
    pub permission: PolymorphicPermissionPattern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PolymorphicManifestPatternGrant {
    pub permission: PolymorphicManifestPermissionPattern,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolymorphicManifestCard {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PolymorphicManifestPatternGrant>,
    pub lower_negative: Vec<PolymorphicManifestPatternGrant>,
    pub upper_positive: Vec<PolymorphicManifestPatternGrant>,
    pub upper_negative: Vec<PolymorphicManifestPatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
}
