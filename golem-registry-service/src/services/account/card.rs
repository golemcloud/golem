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
    AccountOauth2IdentityResourcePattern, AccountPermissionShareResourcePattern,
    AccountPluginResourcePattern, AccountResourcePattern, AccountTokenResourcePattern,
    AccountToolMiddlewareReleaseResourcePattern, AccountToolReleaseResourcePattern,
    AccountUsageResourcePattern, AccountVerb, AgentResourcePattern, ApplicationResourcePattern,
    BlobResourcePattern, CardId, CardManagedBy, CardManagedByAccountRoot, CardResourcePattern,
    ClassPermissionPattern, ComponentResourcePattern, ConfigResourcePattern, EnvResourcePattern,
    EnvironmentAgentSecretResourcePattern, EnvironmentBlobBucketResourcePattern,
    EnvironmentDomainRegistrationResourcePattern, EnvironmentHttpApiDeploymentResourcePattern,
    EnvironmentInitialFilesResourcePattern, EnvironmentKvBucketResourcePattern,
    EnvironmentMcpDeploymentResourcePattern, EnvironmentPluginGrantResourcePattern,
    EnvironmentResourceDefinitionResourcePattern, EnvironmentResourcePattern,
    EnvironmentRetryPolicyResourcePattern, EnvironmentSecuritySchemeResourcePattern,
    EnvironmentToolGrantResourcePattern, EnvironmentToolMiddlewareGrantResourcePattern,
    FilesystemResourcePattern, KvResourcePattern, NetworkResourcePattern, OplogResourcePattern,
    PermissionPattern, PlanResourcePattern, RdbmsResourcePattern, RdbmsVerb, SecretResourcePattern,
    SystemResourcePattern, SystemVerb, ToolResourcePattern,
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
        Some(CardManagedBy::AccountRoot(CardManagedByAccountRoot {
            account_id,
        })),
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

    // Every account verb except set-plan: the plan decides what the account may use and what
    // it pays, so only an administrator or the holder of a set-plan permission share changes it.
    grants.extend(OWNER_ACCOUNT_VERBS.map(|verb| {
        PermissionPattern::Account(ClassPermissionPattern {
            verb: Some(verb),
            owner: AccountOwnerPattern::Account {
                account: account_email.clone(),
            },
            recipient: RecipientPattern::Any,
            resource: AccountResourcePattern,
        })
    }));
}

const OWNER_ACCOUNT_VERBS: [AccountVerb; 4] = [
    AccountVerb::View,
    AccountVerb::Update,
    AccountVerb::Delete,
    AccountVerb::ViewPlan,
];

fn add_admin_grants(grants: &mut Vec<PermissionPattern>) {
    grants.extend([
        PermissionPattern::Account(ClassPermissionPattern {
            verb: None,
            owner: AccountOwnerPattern::Any,
            recipient: RecipientPattern::Any,
            resource: AccountResourcePattern,
        }),
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
        PermissionPattern::AccountPlugin(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountPluginResourcePattern::Any,
        }),
        PermissionPattern::AccountToolRelease(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountToolReleaseResourcePattern::Any,
        }),
        PermissionPattern::AccountToolMiddlewareRelease(ClassPermissionPattern {
            verb: None,
            owner: account_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: AccountToolMiddlewareReleaseResourcePattern::Any,
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
        PermissionPattern::EnvironmentToolGrant(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentToolGrantResourcePattern::Any,
        }),
        PermissionPattern::EnvironmentToolMiddlewareGrant(ClassPermissionPattern {
            verb: None,
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: EnvironmentToolMiddlewareGrantResourcePattern::Any,
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
        PermissionPattern::Rdbms(ClassPermissionPattern {
            verb: Some(RdbmsVerb::Unanalyzed),
            owner: environment_owner.clone(),
            recipient: RecipientPattern::Any,
            resource: RdbmsResourcePattern::Database {
                database: "*".to_string(),
            },
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

#[cfg(test)]
mod tests {
    use super::super::account_permission_target;
    use super::account_root_card_record;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::auth::AccountRole;
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{
        AccountVerb, Card, DelegationSurface, EffectiveSurface, PermissionPattern, parse_permission,
    };
    use test_r::test;

    #[test]
    fn account_root_card_includes_tool_release_and_environment_grant_permissions() {
        let account_id = AccountId::new();
        let account_email = AccountEmail::new("tool-owner@example.com");
        let card: Card = account_root_card_record(account_id, account_email, &[])
            .try_into()
            .unwrap();

        assert!(
            card.lower_positive
                .iter()
                .any(|grant| matches!(grant, PermissionPattern::AccountToolRelease(_)))
        );
        assert!(
            card.lower_positive
                .iter()
                .any(|grant| matches!(grant, PermissionPattern::EnvironmentToolGrant(_)))
        );

        let delegation = DelegationSurface::from_cards(&[card]);
        let scoped = parse_permission(
            "rdbms(tool-owner@example.com/shop/prod) @ tool-owner@example.com/shop/prod/migrations/Migrator : unanalyzed : database(orders)",
        )
        .unwrap();
        assert!(
            delegation
                .validate_attenuation(&[scoped], &[], &[], &[])
                .is_ok()
        );
        let other_account = parse_permission(
            "rdbms(other@example.com/shop/prod) @ * : unanalyzed : database(orders)",
        )
        .unwrap();
        assert!(
            delegation
                .validate_attenuation(&[other_account], &[], &[], &[])
                .is_err()
        );
    }

    fn root_card(account_email: &AccountEmail, roles: &[AccountRole]) -> Card {
        account_root_card_record(AccountId::new(), account_email.clone(), roles)
            .try_into()
            .unwrap()
    }

    fn allows(
        card: &Card,
        holder: &AccountEmail,
        account: &AccountEmail,
        verb: AccountVerb,
    ) -> bool {
        EffectiveSurface::from_cards(
            std::slice::from_ref(card),
            &RecipientPattern::Account {
                account: holder.clone(),
            },
        )
        .unwrap()
        .authorize(&account_permission_target(account, verb))
        .unwrap()
    }

    #[test]
    fn owner_root_card_allows_every_account_verb_except_set_plan() {
        let owner = AccountEmail::new("owner@example.com");
        let card = root_card(&owner, &[]);

        for verb in [
            AccountVerb::View,
            AccountVerb::Update,
            AccountVerb::Delete,
            AccountVerb::ViewPlan,
        ] {
            assert!(allows(&card, &owner, &owner, verb), "{verb:?}");
        }
        assert!(!allows(&card, &owner, &owner, AccountVerb::SetPlan));
    }

    #[test]
    fn owner_cannot_share_set_plan_on_the_own_account() {
        let owner = AccountEmail::new("owner@example.com");
        let delegation = DelegationSurface::from_cards(&[root_card(&owner, &[])]);

        let set_plan =
            parse_permission("account(owner@example.com) @ other@example.com : set-plan :")
                .unwrap();
        assert!(
            delegation
                .validate_attenuation(&[set_plan], &[], &[], &[])
                .is_err()
        );
        let view_plan =
            parse_permission("account(owner@example.com) @ other@example.com : view-plan :")
                .unwrap();
        assert!(
            delegation
                .validate_attenuation(&[view_plan], &[], &[], &[])
                .is_ok()
        );
    }

    #[test]
    fn admin_root_card_allows_set_plan_on_every_account() {
        let admin = AccountEmail::new("admin@example.com");
        let customer = AccountEmail::new("customer@example.com");
        let card = root_card(&admin, &[AccountRole::Admin]);

        assert!(allows(&card, &admin, &admin, AccountVerb::SetPlan));
        assert!(allows(&card, &admin, &customer, AccountVerb::SetPlan));
        assert!(!allows(
            &root_card(&admin, &[AccountRole::MarketingAdmin]),
            &admin,
            &admin,
            AccountVerb::SetPlan
        ));
    }
}
