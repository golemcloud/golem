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

use crate::repo::model::card::CardRecord;
use golem_common::model::account::AccountId;
use golem_common::model::auth::AccountRole;
use golem_common::model::card::owner::{
    AccountOwnerPattern, AgentOwnerPattern, ApplicationOwnerPattern, ComponentOwnerPattern,
    EmptyOwnerPattern, EnvironmentOwnerPattern, ToolOwnerPattern,
};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AccountClass, AccountOauth2IdentityClass, AccountOauth2IdentityResourcePattern,
    AccountPluginClass, AccountPluginResourcePattern, AccountResourcePattern, AccountTokenClass,
    AccountTokenResourcePattern, AccountUsageClass, AccountUsageResourcePattern, AgentClass,
    AgentResourcePattern, ApplicationClass, ApplicationResourcePattern, BlobClass,
    BlobResourcePattern, CardClass, CardId, CardManagedBy, CardResourcePattern,
    ClassPermissionPattern, ComponentClass, ComponentResourcePattern, ConfigClass,
    ConfigResourcePattern, EnvClass, EnvResourcePattern, EnvironmentAgentSecretClass,
    EnvironmentAgentSecretResourcePattern, EnvironmentBlobBucketClass,
    EnvironmentBlobBucketResourcePattern, EnvironmentClass, EnvironmentDomainRegistrationClass,
    EnvironmentDomainRegistrationResourcePattern, EnvironmentHttpApiDeploymentClass,
    EnvironmentHttpApiDeploymentResourcePattern, EnvironmentInitialFilesClass,
    EnvironmentInitialFilesResourcePattern, EnvironmentKvBucketClass,
    EnvironmentKvBucketResourcePattern, EnvironmentMcpDeploymentClass,
    EnvironmentMcpDeploymentResourcePattern, EnvironmentPluginGrantClass,
    EnvironmentPluginGrantResourcePattern, EnvironmentResourceDefinitionClass,
    EnvironmentResourceDefinitionResourcePattern, EnvironmentResourcePattern,
    EnvironmentRetryPolicyClass, EnvironmentRetryPolicyResourcePattern,
    EnvironmentSecuritySchemeClass, EnvironmentSecuritySchemeResourcePattern,
    EnvironmentShareClass, EnvironmentShareResourcePattern, FilesystemClass,
    FilesystemResourcePattern, KvClass, KvResourcePattern, OplogClass, OplogResourcePattern,
    PermissionPattern, PlanClass, PlanResourcePattern, RdbmsClass, RdbmsResourcePattern,
    SecretClass, SecretResourcePattern, SystemClass, SystemResourcePattern, SystemVerb, ToolClass,
    ToolResourcePattern,
};
use uuid::Uuid;

pub(super) fn account_root_card_record(
    account_id: AccountId,
    account_email: &str,
    roles: &[AccountRole],
) -> CardRecord {
    let account = account_id.to_string();
    let recipient = RecipientPattern::Account {
        account: account_email.to_string(),
    };
    let mut grants = Vec::new();

    add_own_account_grants(&mut grants, &account, &recipient);

    if roles.contains(&AccountRole::Admin) {
        add_admin_grants(&mut grants, &recipient);
    }

    if roles.contains(&AccountRole::MarketingAdmin) {
        add_marketing_admin_grants(&mut grants, &recipient);
    }

    CardRecord::creation(
        CardId(Uuid::now_v7()),
        Vec::new(),
        grants,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        true,
        Some(CardManagedBy::AccountRoot { account_id }),
    )
}

macro_rules! grant {
    ($variant:ident, $class:ty, $owner:expr, $resource:expr, $recipient:expr) => {
        PermissionPattern::$variant(ClassPermissionPattern::<$class> {
            verb: None,
            owner: $owner,
            recipient: $recipient.clone(),
            resource: $resource,
        })
    };
}

fn add_own_account_grants(
    grants: &mut Vec<PermissionPattern>,
    account: &str,
    recipient: &RecipientPattern,
) {
    grants.extend([
        grant!(
            Account,
            AccountClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            AccountResourcePattern,
            recipient
        ),
        grant!(
            AccountToken,
            AccountTokenClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            AccountTokenResourcePattern::Any,
            recipient
        ),
        grant!(
            AccountUsage,
            AccountUsageClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            AccountUsageResourcePattern,
            recipient
        ),
        grant!(
            AccountPlugin,
            AccountPluginClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            AccountPluginResourcePattern::Any,
            recipient
        ),
        grant!(
            AccountOauth2Identity,
            AccountOauth2IdentityClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            AccountOauth2IdentityResourcePattern::Any,
            recipient
        ),
        grant!(
            Application,
            ApplicationClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            ApplicationResourcePattern::Any,
            recipient
        ),
        grant!(
            Environment,
            EnvironmentClass,
            ApplicationOwnerPattern::AccountApplications {
                account: account.to_string(),
            },
            EnvironmentResourcePattern::Any,
            recipient
        ),
        grant!(
            Agent,
            AgentClass,
            AgentOwnerPattern::AccountAgents {
                account: account.to_string(),
            },
            AgentResourcePattern::Any,
            recipient
        ),
        grant!(
            Card,
            CardClass,
            AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            CardResourcePattern::Any,
            recipient
        ),
    ]);

    add_account_environment_grants(
        grants,
        EnvironmentOwnerPattern::AccountEnvironments {
            account: account.to_string(),
        },
        recipient,
    );
    add_account_agent_grants(
        grants,
        AgentOwnerPattern::AccountAgents {
            account: account.to_string(),
        },
        recipient,
    );
    add_account_component_grants(
        grants,
        ComponentOwnerPattern::AccountComponents {
            account: account.to_string(),
        },
        recipient,
    );
    add_account_tool_grants(
        grants,
        ToolOwnerPattern::AccountTools {
            account: account.to_string(),
        },
        recipient,
    );
}

fn add_admin_grants(grants: &mut Vec<PermissionPattern>, recipient: &RecipientPattern) {
    grants.extend([
        grant!(
            System,
            SystemClass,
            EmptyOwnerPattern,
            SystemResourcePattern,
            recipient
        ),
        grant!(
            Plan,
            PlanClass,
            EmptyOwnerPattern,
            PlanResourcePattern::Any,
            recipient
        ),
        grant!(
            Network,
            golem_common::model::card::NetworkClass,
            EmptyOwnerPattern,
            golem_common::model::card::NetworkResourcePattern::Any,
            recipient
        ),
        grant!(
            Account,
            AccountClass,
            AccountOwnerPattern::Any,
            AccountResourcePattern,
            recipient
        ),
        grant!(
            AccountToken,
            AccountTokenClass,
            AccountOwnerPattern::Any,
            AccountTokenResourcePattern::Any,
            recipient
        ),
        grant!(
            AccountUsage,
            AccountUsageClass,
            AccountOwnerPattern::Any,
            AccountUsageResourcePattern,
            recipient
        ),
        grant!(
            AccountPlugin,
            AccountPluginClass,
            AccountOwnerPattern::Any,
            AccountPluginResourcePattern::Any,
            recipient
        ),
        grant!(
            AccountOauth2Identity,
            AccountOauth2IdentityClass,
            AccountOwnerPattern::Any,
            AccountOauth2IdentityResourcePattern::Any,
            recipient
        ),
        grant!(
            Application,
            ApplicationClass,
            AccountOwnerPattern::Any,
            ApplicationResourcePattern::Any,
            recipient
        ),
        grant!(
            Environment,
            EnvironmentClass,
            ApplicationOwnerPattern::AnyApplications,
            EnvironmentResourcePattern::Any,
            recipient
        ),
        grant!(
            Agent,
            AgentClass,
            AgentOwnerPattern::AnyAgents,
            AgentResourcePattern::Any,
            recipient
        ),
        grant!(
            Card,
            CardClass,
            AccountOwnerPattern::Any,
            CardResourcePattern::Any,
            recipient
        ),
    ]);

    add_account_environment_grants(grants, EnvironmentOwnerPattern::AnyEnvironments, recipient);
    add_account_agent_grants(grants, AgentOwnerPattern::AnyAgents, recipient);
    add_account_component_grants(grants, ComponentOwnerPattern::AnyComponents, recipient);
    add_account_tool_grants(grants, ToolOwnerPattern::AnyTools, recipient);
}

fn add_marketing_admin_grants(grants: &mut Vec<PermissionPattern>, recipient: &RecipientPattern) {
    grants.extend([
        PermissionPattern::System(ClassPermissionPattern::<SystemClass> {
            verb: Some(SystemVerb::ViewAccountSummariesReport),
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: SystemResourcePattern,
        }),
        PermissionPattern::System(ClassPermissionPattern::<SystemClass> {
            verb: Some(SystemVerb::ViewAccountCountsReport),
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: SystemResourcePattern,
        }),
    ]);
}

fn add_account_environment_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: EnvironmentOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.extend([
        grant!(
            Component,
            ComponentClass,
            owner.clone(),
            ComponentResourcePattern::Any,
            recipient
        ),
        grant!(
            Blob,
            BlobClass,
            owner.clone(),
            BlobResourcePattern::any(),
            recipient
        ),
        grant!(
            Kv,
            KvClass,
            owner.clone(),
            KvResourcePattern::any(),
            recipient
        ),
        grant!(
            Rdbms,
            RdbmsClass,
            owner.clone(),
            RdbmsResourcePattern::any(),
            recipient
        ),
        grant!(
            Secret,
            SecretClass,
            owner.clone(),
            SecretResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentShare,
            EnvironmentShareClass,
            owner.clone(),
            EnvironmentShareResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentPluginGrant,
            EnvironmentPluginGrantClass,
            owner.clone(),
            EnvironmentPluginGrantResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentDomainRegistration,
            EnvironmentDomainRegistrationClass,
            owner.clone(),
            EnvironmentDomainRegistrationResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentSecurityScheme,
            EnvironmentSecuritySchemeClass,
            owner.clone(),
            EnvironmentSecuritySchemeResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentHttpApiDeployment,
            EnvironmentHttpApiDeploymentClass,
            owner.clone(),
            EnvironmentHttpApiDeploymentResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentMcpDeployment,
            EnvironmentMcpDeploymentClass,
            owner.clone(),
            EnvironmentMcpDeploymentResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentAgentSecret,
            EnvironmentAgentSecretClass,
            owner.clone(),
            EnvironmentAgentSecretResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentResourceDefinition,
            EnvironmentResourceDefinitionClass,
            owner.clone(),
            EnvironmentResourceDefinitionResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentRetryPolicy,
            EnvironmentRetryPolicyClass,
            owner.clone(),
            EnvironmentRetryPolicyResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentKvBucket,
            EnvironmentKvBucketClass,
            owner.clone(),
            EnvironmentKvBucketResourcePattern::Any,
            recipient
        ),
        grant!(
            EnvironmentBlobBucket,
            EnvironmentBlobBucketClass,
            owner,
            EnvironmentBlobBucketResourcePattern::Any,
            recipient
        ),
    ]);
}

fn add_account_agent_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: AgentOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.extend([
        grant!(
            Filesystem,
            FilesystemClass,
            owner.clone(),
            FilesystemResourcePattern::any(),
            recipient
        ),
        grant!(
            Env,
            EnvClass,
            owner.clone(),
            EnvResourcePattern::Any,
            recipient
        ),
        grant!(
            Config,
            ConfigClass,
            owner.clone(),
            ConfigResourcePattern::Any,
            recipient
        ),
        grant!(
            Oplog,
            OplogClass,
            owner,
            OplogResourcePattern::Any,
            recipient
        ),
    ]);
}

fn add_account_component_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: ComponentOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.push(grant!(
        EnvironmentInitialFiles,
        EnvironmentInitialFilesClass,
        owner,
        EnvironmentInitialFilesResourcePattern::any(),
        recipient
    ));
}

fn add_account_tool_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: ToolOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.push(grant!(
        Tool,
        ToolClass,
        owner,
        ToolResourcePattern::any(),
        recipient
    ));
}
