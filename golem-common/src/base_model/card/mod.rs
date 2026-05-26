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

mod algebra;
mod class;
mod owner;
mod parsing;
mod recipient;
mod types;

#[cfg(test)]
mod parsing_tests;

#[cfg(test)]
mod subsumption_tests;

#[cfg(test)]
mod tests;

pub use algebra::*;
pub use class::*;
pub use parsing::*;
pub use types::*;

use serde::{Deserialize, Serialize};

macro_rules! card_permission_classes {
    ($macro:ident) => {
        $macro! {
            Filesystem: FilesystemClass,
            Network: NetworkClass,
            Env: EnvClass,
            Oplog: OplogClass,
            Config: ConfigClass,
            Secret: SecretClass,
            Agent: AgentClass,
            Tool: ToolClass,
            Kv: KvClass,
            Blob: BlobClass,
            Rdbms: RdbmsClass,
            Card: CardClass,
            System: SystemClass,
            Plan: PlanClass,
            Account: AccountClass,
            AccountUsage: AccountUsageClass,
            AccountToken: AccountTokenClass,
            AccountPlugin: AccountPluginClass,
            Application: ApplicationClass,
            Environment: EnvironmentClass,
            EnvironmentShare: EnvironmentShareClass,
            EnvironmentPluginGrant: EnvironmentPluginGrantClass,
            EnvironmentDomainRegistration: EnvironmentDomainRegistrationClass,
            EnvironmentSecurityScheme: EnvironmentSecuritySchemeClass,
            EnvironmentHttpApiDeployment: EnvironmentHttpApiDeploymentClass,
            EnvironmentMcpDeployment: EnvironmentMcpDeploymentClass,
            EnvironmentAgentSecret: EnvironmentAgentSecretClass,
            EnvironmentResourceDefinition: EnvironmentResourceDefinitionClass,
            EnvironmentRetryPolicy: EnvironmentRetryPolicyClass,
            Component: ComponentClass,
            AccountOauth2Identity: AccountOauth2IdentityClass,
            EnvironmentInitialFiles: EnvironmentInitialFilesClass,
            EnvironmentKvBucket: EnvironmentKvBucketClass,
            EnvironmentBlobBucket: EnvironmentBlobBucketClass,
        }
    };
}

macro_rules! define_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionPattern {
            $($variant(ClassPermissionPattern<$class>),)+
        }
    };
}

macro_rules! define_polymorphic_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicPermissionPattern {
            $($variant(PolymorphicClassPermissionPattern<$class>),)+
        }
    };
}

macro_rules! define_polymorphic_manifest_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicManifestPermissionPattern {
            $($variant(PolymorphicManifestClassPermissionPattern<$class>),)+
        }
    };
}

card_permission_classes!(define_permission_pattern);
card_permission_classes!(define_polymorphic_permission_pattern);
card_permission_classes!(define_polymorphic_manifest_permission_pattern);

macro_rules! define_class_name_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! class_name_match {
            ($self:expr) => {
                match $self {
                    $(Self::$variant(_) => <$class as PermissionClass>::NAME,)+
                }
            };
        }
    };
}

card_permission_classes!(define_class_name_match);

macro_rules! define_same_variant_subsumes_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_subsumes_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (Self::$variant(a), Self::$variant(b)) => a.subsumes(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_subsumes_match);

macro_rules! define_matches_recipient_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! matches_recipient_match {
            ($self:expr, $holder:expr) => {
                match $self {
                    $(
                        Self::$variant(pattern) => pattern.matches_holder($holder),
                    )+
                }
            };
        }
    };
}

card_permission_classes!(define_matches_recipient_match);

impl PermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        same_variant_subsumes_match!(self, other)
    }

    pub fn matches_recipient(&self, holder: &str) -> bool {
        matches_recipient_match!(self, holder)
    }
}

impl PolymorphicPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}

impl PolymorphicManifestPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}
