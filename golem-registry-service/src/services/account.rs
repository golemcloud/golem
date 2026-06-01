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

use super::plan::{PlanError, PlanService};
use crate::config::PrecreatedAccount;
use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountRepoError, AccountRevisionRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::card::CardRecord;
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use anyhow::anyhow;
use golem_common::model::account::{
    Account, AccountCreation, AccountId, AccountRevision, AccountSetPlan, AccountUpdate,
};
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
use golem_common::model::plan::PlanId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AccountAction, GlobalAction};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Account by email not found: {0}")]
    AccountByEmailNotFound(String),
    #[error("Plan for id not found: {0}")]
    PlanByIdNotFound(PlanId),
    #[error("Email already in use")]
    EmailAlreadyInUse,
    #[error("Concurrent update")]
    ConcurrentUpdate,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AccountError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AccountNotFound(_) => self.to_string(),
            Self::AccountByEmailNotFound(_) => self.to_string(),
            Self::EmailAlreadyInUse => self.to_string(),
            Self::PlanByIdNotFound(_) => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AccountError, PlanError, AccountRepoError);

pub struct AccountService {
    account_repo: Arc<dyn AccountRepo>,
    plan_service: Arc<PlanService>,
    default_plan_id: PlanId,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl AccountService {
    pub fn new(
        account_repo: Arc<dyn AccountRepo>,
        plan_service: Arc<PlanService>,
        default_plan_id: PlanId,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            account_repo,
            plan_service,
            default_plan_id,
            registry_change_notifier,
        }
    }

    pub async fn create_initial_accounts(
        &self,
        accounts: &HashMap<String, PrecreatedAccount>,
    ) -> Result<(), AccountError> {
        for (name, account) in accounts {
            let existing_account = self.get_optional(account.id, &AuthCtx::System).await?;

            if existing_account.is_none() {
                info!("Creating initial account {} with id {}", name, account.id);
                self.create_internal(
                    account.id,
                    AccountCreation {
                        name: account.name.clone(),
                        email: account.email.clone(),
                        roles: vec![account.role],
                    },
                    account.plan_id,
                    &AuthCtx::System,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn create(
        &self,
        account: AccountCreation,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_global_action(GlobalAction::CreateAccount)?;

        let id = AccountId::new();
        info!("Creating account: {}", id);
        self.create_internal(id, account, self.default_plan_id, auth)
            .await
    }

    pub async fn update(
        &self,
        account_id: AccountId,
        update: AccountUpdate,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.get(account_id, auth).await?;

        auth.authorize_account_action(account_id, AccountAction::UpdateAccount)?;

        if update.current_revision != account.revision {
            return Err(AccountError::ConcurrentUpdate);
        };

        info!("Updating account: {}", account_id);

        if let Some(new_name) = update.name {
            account.name = new_name;
        }

        self.update_internal(account, auth).await
    }

    pub async fn set_plan(
        &self,
        account_id: AccountId,
        update: AccountSetPlan,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.get(account_id, auth).await?;

        auth.authorize_account_action(account_id, AccountAction::SetPlan)?;

        if update.current_revision != account.revision {
            return Err(AccountError::ConcurrentUpdate);
        };

        info!("Updating account: {}", account_id);

        // check that plan exists
        self.plan_service
            .get(&update.plan, auth)
            .await
            .map_err(|e| match e {
                PlanError::PlanNotFound(plan_id) => AccountError::PlanByIdNotFound(plan_id),
                other => other.into(),
            })?;

        account.plan_id = update.plan;

        self.update_internal(account, auth).await
    }

    pub async fn delete(
        &self,
        account_id: AccountId,
        current_revision: AccountRevision,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.get(account_id, auth).await?;

        auth.authorize_account_action(account_id, AccountAction::DeleteAccount)?;

        if current_revision != account.revision {
            return Err(AccountError::ConcurrentUpdate);
        };

        info!("Deleting account: {}", account_id);

        account.revision = account.revision.next()?;

        let record = AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::deletion(auth.actor_account_id().0),
        );

        match self.account_repo.delete(record).await {
            Ok(record) => {
                let account: Account = record
                    .signal_new_events_available(&self.registry_change_notifier)
                    .try_into()?;
                Ok(account)
            }
            Err(AccountRepoError::ConcurrentModification) => Err(AccountError::ConcurrentUpdate)?,
            Err(other) => Err(other)?,
        }
    }

    pub async fn get(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_account_action(account_id, AccountAction::ViewAccount)
            .map_err(|_| AccountError::AccountNotFound(account_id))?;

        let account = self
            .account_repo
            .get_by_id(account_id.0)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id))?
            .try_into()?;

        Ok(account)
    }

    pub async fn get_by_email(
        &self,
        account_email: &str,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let account: Account = self
            .account_repo
            .get_by_email(account_email)
            .await?
            .ok_or(AccountError::AccountByEmailNotFound(
                account_email.to_string(),
            ))?
            .try_into()?;

        auth.authorize_account_action(account.id, AccountAction::ViewAccount)
            .map_err(|_| AccountError::AccountByEmailNotFound(account_email.to_string()))?;

        Ok(account)
    }

    pub async fn get_optional(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Option<Account>, AccountError> {
        match self.get(account_id, auth).await {
            Ok(account) => Ok(Some(account)),
            Err(AccountError::AccountNotFound(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    async fn create_internal(
        &self,
        id: AccountId,
        account: AccountCreation,
        plan_id: PlanId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_global_action(GlobalAction::CreateAccount)?;

        if id == AccountId::SYSTEM {
            Err(anyhow!("Cannot create account with reserved account id"))?
        };

        let account_root_card = account_root_card_record(id, &account.roles);

        let record = AccountRevisionRecord::new(
            id,
            account.name,
            account.email.into_inner(),
            plan_id,
            account.roles,
            auth.actor_account_id(),
        );

        let result = self.account_repo.create(record, account_root_card).await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(other) => Err(other)?,
        }
    }

    async fn update_internal(
        &self,
        mut account: Account,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        account.revision = account.revision.next()?;

        let record = AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::new(auth.actor_account_id().0),
        );

        let result = self.account_repo.update(record).await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(AccountRepoError::ConcurrentModification) => Err(AccountError::ConcurrentUpdate)?,
            Err(other) => Err(other)?,
        }
    }
}

fn account_root_card_record(account_id: AccountId, roles: &[AccountRole]) -> CardRecord {
    let account = account_id.to_string();
    let recipient = RecipientPattern::Account {
        account: account.clone(),
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
