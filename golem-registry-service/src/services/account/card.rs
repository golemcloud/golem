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
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::AccountRole;
use golem_common::model::card::owner::{
    AccountOwnerPattern, AgentOwnerPattern, ApplicationOwnerPattern, ComponentOwnerPattern,
    EmptyOwnerPattern, EnvironmentOwnerPattern, ToolOwnerPattern,
};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AccountOauth2IdentityResourcePattern, AccountPermissionShareResourcePattern, AccountPluginResourcePattern, AccountResourcePattern, AccountTokenResourcePattern, AccountUsageResourcePattern, AgentResourcePattern, ApplicationResourcePattern, BlobResourcePattern, CardId, CardManagedBy, CardManagedByAccountRoot, CardResourcePattern, ClassPermissionPattern, ComponentResourcePattern, ConfigResourcePattern, EnvResourcePattern, EnvironmentAgentSecretResourcePattern, EnvironmentBlobBucketResourcePattern, EnvironmentDomainRegistrationResourcePattern, EnvironmentHttpApiDeploymentResourcePattern, EnvironmentInitialFilesResourcePattern, EnvironmentKvBucketResourcePattern, EnvironmentMcpDeploymentResourcePattern, EnvironmentPluginGrantResourcePattern, EnvironmentResourceDefinitionResourcePattern, EnvironmentResourcePattern, EnvironmentRetryPolicyResourcePattern, EnvironmentSecuritySchemeResourcePattern, FilesystemResourcePattern, KvResourcePattern, NetworkResourcePattern, OplogResourcePattern, PermissionPattern, PlanResourcePattern, RdbmsResourcePattern, SecretResourcePattern, SystemResourcePattern, SystemVerb, ToolResourcePattern
};

pub(super) fn account_root_card_record(
    account_id: AccountId,
    account_email: AccountEmail,
    roles: &[AccountRole],
) -> CardRecord {
    let mut grants = Vec::new();

    add_own_account_grants(&mut grants, &account_email);

    if roles.contains(&AccountRole::Admin) {
        add_admin_grants(&mut grants);
    }

    if roles.contains(&AccountRole::MarketingAdmin) {
        add_marketing_admin_grants(&mut grants);
    }

    CardRecord::creation(
        CardId::new(),
        Vec::new(),
        grants,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        true,
        Some(CardManagedBy::AccountRoot(CardManagedByAccountRoot { account_id })),
    )
}

fn add_own_account_grants(grants: &mut Vec<PermissionPattern>, account_email: &AccountEmail) {
    add_account_grants(
        grants,
        AccountOwnerPattern::Account {
            account: account_email.clone(),
        },
        ApplicationOwnerPattern::AccountApplications {
            account: account_email.clone(),
        },
        AgentOwnerPattern::AccountAgents {
            account: account_email.clone(),
        },
        EnvironmentOwnerPattern::AccountEnvironments {
            account: account_email.clone(),
        },
        ComponentOwnerPattern::AccountComponents {
            account: account_email.clone(),
        },
        ToolOwnerPattern::AccountTools {
            account: account_email.clone(),
        },
    );
}

fn add_admin_grants(grants: &mut Vec<PermissionPattern>) {
    grants.extend([
        PermissionPattern::System(ClassPermissionPattern {
            verb: None,
            owner: EmptyOwnerPattern,
            recipient: RecipientPattern::Any,
            resource: SystemResourcePattern,
        }),
        PermissionPattern::Plan(ClassPermissionPattern {
            verb: None,
            owner: EmptyOwnerPattern,
            recipient: RecipientPattern::Any,
            resource: PlanResourcePattern::Any,
        }),
    ]);

    add_account_grants(
        grants,
        AccountOwnerPattern::Any,
        ApplicationOwnerPattern::AnyApplications,
        AgentOwnerPattern::AnyAgents,
        EnvironmentOwnerPattern::AnyEnvironments,
        ComponentOwnerPattern::AnyComponents,
        ToolOwnerPattern::AnyTools,
    );
}

fn add_marketing_admin_grants(grants: &mut Vec<PermissionPattern>) {
    grants.extend([
        PermissionPattern::System(ClassPermissionPattern {
            verb: Some(SystemVerb::ViewAccountSummariesReport),
            owner: EmptyOwnerPattern,
            recipient: RecipientPattern::Any,
            resource: SystemResourcePattern,
        }),
        PermissionPattern::System(ClassPermissionPattern {
            verb: Some(SystemVerb::ViewAccountCountsReport),
            owner: EmptyOwnerPattern,
            recipient: RecipientPattern::Any,
            resource: SystemResourcePattern,
        }),
    ]);
}

fn add_account_grants(
    grants: &mut Vec<PermissionPattern>,
    account_owner: AccountOwnerPattern,
    application_owner: ApplicationOwnerPattern,
    agent_owner: AgentOwnerPattern,
    environment_owner: EnvironmentOwnerPattern,
    component_owner: ComponentOwnerPattern,
    tool_owner: ToolOwnerPattern,
) {
    grants.extend([
        PermissionPattern::Network(ClassPermissionPattern {
            verb: None,
            owner: EmptyOwnerPattern,
            recipient: RecipientPattern::Any,
            resource: NetworkResourcePattern::Any,
        }),
        PermissionPattern::AccountOauth2Identity(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountOauth2IdentityResourcePattern::Any,
        }),
        PermissionPattern::Account(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountResourcePattern,
        }),
        PermissionPattern::AccountPlugin(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountPluginResourcePattern::Any,
        }),
        PermissionPattern::AccountPermissionShare(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountPermissionShareResourcePattern::Any,
        }),
        PermissionPattern::AccountToken(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountTokenResourcePattern::Any,
        }),
        PermissionPattern::AccountUsage(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountUsageResourcePattern,
        }),
        PermissionPattern::Card(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: CardResourcePattern::Any,
        }),
        PermissionPattern::Application(ClassPermissionPattern {
            verb: None,
            owner: application_owner,
            recipient: RecipientPattern::Any,
            resource: ApplicationResourcePattern,
        }),
        PermissionPattern::Environment(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentResourcePattern::Any,
        }),
        PermissionPattern::Component(ClassPermissionPattern {
            verb: None,
            owner: component_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: ComponentResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentAgentSecret(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentAgentSecretResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentBlobBucket(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentBlobBucketResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentDomainRegistration(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentDomainRegistrationResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentHttpApiDeployment(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentHttpApiDeploymentResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentKvBucket(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentKvBucketResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentMcpDeployment(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentMcpDeploymentResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentPluginGrant(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentPluginGrantResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentResourceDefinition(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentResourceDefinitionResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentRetryPolicy(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentRetryPolicyResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentSecurityScheme(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentSecuritySchemeResourcePattern::Any,
        }),
        PermissionPattern::Blob(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: BlobResourcePattern::any(),
        }),
        PermissionPattern::Kv(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: KvResourcePattern::any(),
        }),
        PermissionPattern::Rdbms(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: RdbmsResourcePattern::any(),
        }),
        PermissionPattern::Secret(ClassPermissionPattern {
            verb: None,
            owner: environment_owner,
            recipient: RecipientPattern::Any,
            resource: SecretResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentInitialFiles(ClassPermissionPattern {
            verb: None,
            owner: component_owner,
            recipient: RecipientPattern::Any,
            resource: EnvironmentInitialFilesResourcePattern::any(),
        }),
        PermissionPattern::Agent(ClassPermissionPattern {
            verb: None,
            owner: agent_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AgentResourcePattern::Any,
        }),
        PermissionPattern::Config(ClassPermissionPattern {
            verb: None,
            owner: agent_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: ConfigResourcePattern::Any,
        }),
        PermissionPattern::Env(ClassPermissionPattern {
            verb: None,
            owner: agent_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvResourcePattern::Any,
        }),
        PermissionPattern::Filesystem(ClassPermissionPattern {
            verb: None,
            owner: agent_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: FilesystemResourcePattern::any(),
        }),
        PermissionPattern::Oplog(ClassPermissionPattern {
            verb: None,
            owner: agent_owner,
            recipient: RecipientPattern::Any,
            resource: OplogResourcePattern::Any,
        }),
        PermissionPattern::Tool(ClassPermissionPattern {
            verb: None,
            owner: tool_owner,
            recipient: RecipientPattern::Any,
            resource: ToolResourcePattern::any(),
        }),
    ]);
}
