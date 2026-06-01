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
    AccountOauth2IdentityResourcePattern, AccountPluginResourcePattern, AccountResourcePattern,
    AccountTokenResourcePattern, AccountUsageResourcePattern, AgentResourcePattern,
    ApplicationResourcePattern, BlobResourcePattern, CardId, CardManagedBy, CardResourcePattern,
    ClassPermissionPattern, ComponentResourcePattern, ConfigResourcePattern, EnvResourcePattern,
    EnvironmentAgentSecretResourcePattern, EnvironmentBlobBucketResourcePattern,
    EnvironmentDomainRegistrationResourcePattern, EnvironmentHttpApiDeploymentResourcePattern,
    EnvironmentInitialFilesResourcePattern, EnvironmentKvBucketResourcePattern,
    EnvironmentMcpDeploymentResourcePattern, EnvironmentPluginGrantResourcePattern,
    EnvironmentResourceDefinitionResourcePattern, EnvironmentResourcePattern,
    EnvironmentRetryPolicyResourcePattern, EnvironmentSecuritySchemeResourcePattern,
    EnvironmentShareResourcePattern, FilesystemResourcePattern, KvResourcePattern,
    OplogResourcePattern, PermissionPattern, PlanResourcePattern, RdbmsResourcePattern,
    SecretResourcePattern, SystemResourcePattern, SystemVerb, ToolResourcePattern,
};

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
        CardId::new(),
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

fn add_own_account_grants(
    grants: &mut Vec<PermissionPattern>,
    account: &str,
    recipient: &RecipientPattern,
) {
    grants.extend([
        PermissionPattern::Account(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: AccountResourcePattern,
        }),
        PermissionPattern::AccountToken(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: AccountTokenResourcePattern::Any,
        }),
        PermissionPattern::AccountUsage(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: AccountUsageResourcePattern,
        }),
        PermissionPattern::AccountPlugin(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: AccountPluginResourcePattern::Any,
        }),
        PermissionPattern::AccountOauth2Identity(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: AccountOauth2IdentityResourcePattern::Any,
        }),
        PermissionPattern::Application(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: ApplicationResourcePattern::Any,
        }),
        PermissionPattern::Environment(ClassPermissionPattern {
            verb: None,
            owner: ApplicationOwnerPattern::AccountApplications {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: EnvironmentResourcePattern::Any,
        }),
        PermissionPattern::Agent(ClassPermissionPattern {
            verb: None,
            owner: AgentOwnerPattern::AccountAgents {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: AgentResourcePattern::Any,
        }),
        PermissionPattern::Card(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Account {
                account: account.to_string(),
            },
            recipient: recipient.clone(),
            resource: CardResourcePattern::Any,
        }),
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
        PermissionPattern::System(ClassPermissionPattern {
            verb: None,
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: SystemResourcePattern,
        }),
        PermissionPattern::Plan(ClassPermissionPattern {
            verb: None,
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: PlanResourcePattern::Any,
        }),
        PermissionPattern::Network(ClassPermissionPattern {
            verb: None,
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: golem_common::model::card::NetworkResourcePattern::Any,
        }),
        PermissionPattern::Account(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: AccountResourcePattern,
        }),
        PermissionPattern::AccountToken(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: AccountTokenResourcePattern::Any,
        }),
        PermissionPattern::AccountUsage(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: AccountUsageResourcePattern,
        }),
        PermissionPattern::AccountPlugin(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: AccountPluginResourcePattern::Any,
        }),
        PermissionPattern::AccountOauth2Identity(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: AccountOauth2IdentityResourcePattern::Any,
        }),
        PermissionPattern::Application(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: ApplicationResourcePattern::Any,
        }),
        PermissionPattern::Environment(ClassPermissionPattern {
            verb: None,
            owner: ApplicationOwnerPattern::AnyApplications,
            recipient: recipient.clone(),
            resource: EnvironmentResourcePattern::Any,
        }),
        PermissionPattern::Agent(ClassPermissionPattern {
            verb: None,
            owner: AgentOwnerPattern::AnyAgents,
            recipient: recipient.clone(),
            resource: AgentResourcePattern::Any,
        }),
        PermissionPattern::Card(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: recipient.clone(),
            resource: CardResourcePattern::Any,
        }),
    ]);

    add_account_environment_grants(grants, EnvironmentOwnerPattern::AnyEnvironments, recipient);
    add_account_agent_grants(grants, AgentOwnerPattern::AnyAgents, recipient);
    add_account_component_grants(grants, ComponentOwnerPattern::AnyComponents, recipient);
    add_account_tool_grants(grants, ToolOwnerPattern::AnyTools, recipient);
}

fn add_marketing_admin_grants(grants: &mut Vec<PermissionPattern>, recipient: &RecipientPattern) {
    grants.extend([
        PermissionPattern::System(ClassPermissionPattern {
            verb: Some(SystemVerb::ViewAccountSummariesReport),
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: SystemResourcePattern,
        }),
        PermissionPattern::System(ClassPermissionPattern {
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
        PermissionPattern::Component(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: ComponentResourcePattern::Any,
        }),
        PermissionPattern::Blob(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: BlobResourcePattern::any(),
        }),
        PermissionPattern::Kv(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: KvResourcePattern::any(),
        }),
        PermissionPattern::Rdbms(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: RdbmsResourcePattern::any(),
        }),
        PermissionPattern::Secret(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: SecretResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentShare(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentShareResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentPluginGrant(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentPluginGrantResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentDomainRegistration(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentDomainRegistrationResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentSecurityScheme(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentSecuritySchemeResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentHttpApiDeployment(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentHttpApiDeploymentResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentMcpDeployment(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentMcpDeploymentResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentAgentSecret(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentAgentSecretResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentResourceDefinition(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentResourceDefinitionResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentRetryPolicy(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentRetryPolicyResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentKvBucket(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvironmentKvBucketResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentBlobBucket(ClassPermissionPattern {
            verb: None,
            owner,
            recipient: recipient.clone(),
            resource: EnvironmentBlobBucketResourcePattern::Any,
        }),
    ]);
}

fn add_account_agent_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: AgentOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.extend([
        PermissionPattern::Filesystem(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: FilesystemResourcePattern::any(),
        }),
        PermissionPattern::Env(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: EnvResourcePattern::Any,
        }),
        PermissionPattern::Config(ClassPermissionPattern {
            verb: None,
            owner: owner.clone(),
            recipient: recipient.clone(),
            resource: ConfigResourcePattern::Any,
        }),
        PermissionPattern::Oplog(ClassPermissionPattern {
            verb: None,
            owner,
            recipient: recipient.clone(),
            resource: OplogResourcePattern::Any,
        }),
    ]);
}

fn add_account_component_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: ComponentOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.push(PermissionPattern::EnvironmentInitialFiles(
        ClassPermissionPattern {
            verb: None,
            owner,
            recipient: recipient.clone(),
            resource: EnvironmentInitialFilesResourcePattern::any(),
        },
    ));
}

fn add_account_tool_grants(
    grants: &mut Vec<PermissionPattern>,
    owner: ToolOwnerPattern,
    recipient: &RecipientPattern,
) {
    grants.push(PermissionPattern::Tool(ClassPermissionPattern {
        verb: None,
        owner,
        recipient: recipient.clone(),
        resource: ToolResourcePattern::any(),
    }));
}
