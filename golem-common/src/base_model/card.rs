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

use chrono::{DateTime, Utc};
use desert_rust::{
    BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct OwnerPathPattern(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct RecipientPathPattern(pub String);

macro_rules! define_permission_patterns {
    ($(
        $variant:ident($pattern:ident, $verb:ident, $resource:ident) => $class_name:literal
    ),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
            pub enum $verb {}

            #[cfg(feature = "full")]
            impl BinarySerializer for $verb {
                fn serialize<Output: BinaryOutput>(
                    &self,
                    _context: &mut SerializationContext<Output>,
                ) -> desert_rust::Result<()> {
                    match *self {}
                }
            }

            #[cfg(feature = "full")]
            impl BinaryDeserializer for $verb {
                fn deserialize(
                    _context: &mut DeserializationContext<'_>,
                ) -> desert_rust::Result<Self> {
                    Err(desert_rust::Error::DeserializationFailure(format!(
                        "{} has no variants yet",
                        stringify!($verb)
                    )))
                }
            }

            #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
            pub enum $resource {}

            impl BinarySerializer for $resource {
                fn serialize<Output: BinaryOutput>(
                    &self,
                    _context: &mut SerializationContext<Output>,
                ) -> desert_rust::Result<()> {
                    match *self {}
                }
            }

            impl BinaryDeserializer for $resource {
                fn deserialize(
                    _context: &mut DeserializationContext<'_>,
                ) -> desert_rust::Result<Self> {
                    Err(desert_rust::Error::DeserializationFailure(format!(
                        "{} has no variants yet",
                        stringify!($resource)
                    )))
                }
            }

            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
            pub struct $pattern {
                pub verb: $verb,
                pub resource: $resource,
            }
        )+

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionPattern {
            $($variant($pattern)),+
        }

        impl PermissionPattern {
            pub fn class_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $class_name),+
                }
            }
        }
    };
}

define_permission_patterns! {
    Filesystem(FilesystemPermissionPattern, FilesystemVerb, FilesystemResourcePattern) => "filesystem",
    Network(NetworkPermissionPattern, NetworkVerb, NetworkResourcePattern) => "network",
    Env(EnvPermissionPattern, EnvVerb, EnvResourcePattern) => "env",
    Oplog(OplogPermissionPattern, OplogVerb, OplogResourcePattern) => "oplog",
    Config(ConfigPermissionPattern, ConfigVerb, ConfigResourcePattern) => "config",
    Secret(SecretPermissionPattern, SecretVerb, SecretResourcePattern) => "secret",
    Agent(AgentPermissionPattern, AgentVerb, AgentResourcePattern) => "agent",
    Tool(ToolPermissionPattern, ToolVerb, ToolResourcePattern) => "tool",
    Kv(KvPermissionPattern, KvVerb, KvResourcePattern) => "kv",
    Blob(BlobPermissionPattern, BlobVerb, BlobResourcePattern) => "blob",
    Rdbms(RdbmsPermissionPattern, RdbmsVerb, RdbmsResourcePattern) => "rdbms",
    Card(CardPermissionPattern, CardVerb, CardResourcePattern) => "card",
    System(SystemPermissionPattern, SystemVerb, SystemResourcePattern) => "system",
    Plan(PlanPermissionPattern, PlanVerb, PlanResourcePattern) => "plan",
    Account(AccountPermissionPattern, AccountVerb, AccountResourcePattern) => "account",
    AccountUsage(AccountUsagePermissionPattern, AccountUsageVerb, AccountUsageResourcePattern) => "account.usage",
    AccountToken(AccountTokenPermissionPattern, AccountTokenVerb, AccountTokenResourcePattern) => "account.token",
    AccountPlugin(AccountPluginPermissionPattern, AccountPluginVerb, AccountPluginResourcePattern) => "account.plugin",
    Application(ApplicationPermissionPattern, ApplicationVerb, ApplicationResourcePattern) => "application",
    Environment(EnvironmentPermissionPattern, EnvironmentVerb, EnvironmentResourcePattern) => "environment",
    EnvironmentShare(EnvironmentSharePermissionPattern, EnvironmentShareVerb, EnvironmentShareResourcePattern) => "environment.share",
    EnvironmentPluginGrant(EnvironmentPluginGrantPermissionPattern, EnvironmentPluginGrantVerb, EnvironmentPluginGrantResourcePattern) => "environment.plugin-grant",
    EnvironmentDomainRegistration(EnvironmentDomainRegistrationPermissionPattern, EnvironmentDomainRegistrationVerb, EnvironmentDomainRegistrationResourcePattern) => "environment.domain-registration",
    EnvironmentSecurityScheme(EnvironmentSecuritySchemePermissionPattern, EnvironmentSecuritySchemeVerb, EnvironmentSecuritySchemeResourcePattern) => "environment.security-scheme",
    EnvironmentHttpApiDeployment(EnvironmentHttpApiDeploymentPermissionPattern, EnvironmentHttpApiDeploymentVerb, EnvironmentHttpApiDeploymentResourcePattern) => "environment.http-api-deployment",
    EnvironmentMcpDeployment(EnvironmentMcpDeploymentPermissionPattern, EnvironmentMcpDeploymentVerb, EnvironmentMcpDeploymentResourcePattern) => "environment.mcp-deployment",
    EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern, EnvironmentAgentSecretVerb, EnvironmentAgentSecretResourcePattern) => "environment.agent-secret",
    EnvironmentResourceDefinition(EnvironmentResourceDefinitionPermissionPattern, EnvironmentResourceDefinitionVerb, EnvironmentResourceDefinitionResourcePattern) => "environment.resource-definition",
    EnvironmentRetryPolicy(EnvironmentRetryPolicyPermissionPattern, EnvironmentRetryPolicyVerb, EnvironmentRetryPolicyResourcePattern) => "environment.retry-policy",
    Component(ComponentPermissionPattern, ComponentVerb, ComponentResourcePattern) => "component",
    AccountOauth2Identity(AccountOauth2IdentityPermissionPattern, AccountOauth2IdentityVerb, AccountOauth2IdentityResourcePattern) => "account.oauth2-identity",
    EnvironmentInitialFiles(EnvironmentInitialFilesPermissionPattern, EnvironmentInitialFilesVerb, EnvironmentInitialFilesResourcePattern) => "environment.initial-files",
    EnvironmentKvBucket(EnvironmentKvBucketPermissionPattern, EnvironmentKvBucketVerb, EnvironmentKvBucketResourcePattern) => "environment.kv-bucket",
    EnvironmentBlobBucket(EnvironmentBlobBucketPermissionPattern, EnvironmentBlobBucketVerb, EnvironmentBlobBucketResourcePattern) => "environment.blob-bucket",
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PatternGrant {
    pub owner: OwnerPathPattern,
    pub recipient: RecipientPathPattern,
    pub permission: PermissionPattern,
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
