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

use super::account::{AccountError, AccountService};
use super::component::{ComponentError, ComponentService};
use super::environment::{EnvironmentError, EnvironmentService};
use super::permission_share::{PermissionShareError, PermissionShareService};
use super::registry_change_notifier::{RegistryChangeNotifier, RequiresNotificationSignalExt};
use crate::repo::card::CardRepo;
use crate::repo::environment::EnvironmentDefaultCardRef;
use crate::repo::model::card::{CardRecord, CardRepoError};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::card::owner::{
    AccountOwnerPattern, ComponentOwnerPattern, EnvironmentOwnerPattern,
};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    Card, CardId, CardManagedBy, CardManagedByAgentInitial, CardManagedByEnvironmentDefault,
    CardResourcePattern, CardVerb, ClassPermissionPattern, ClassPermissionTarget,
    ComponentResourcePattern, EnvironmentResourcePattern, PermissionPattern, PermissionTarget,
    PolymorphicCard, StoredCard,
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::permission_share::PermissionShareId;
use golem_common::{IntoAnyhow, SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum CardError {
    #[error("Card {0} not found")]
    CardNotFound(CardId),
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Cannot revoke a system card")]
    CannotRevokeSystemCard,
    #[error("Permission-share-managed cards must be revoked through the permission share")]
    CannotRevokePermissionShareCard,
    #[error("Environment-default cards cannot be revoked directly")]
    CannotRevokeEnvironmentDefaultCard,
    #[error("Card {0} has no owner")]
    CardOwnerNotFound(CardId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for CardError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::CardNotFound(_) => self.to_string(),
            Self::AccountNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::CannotRevokeSystemCard => self.to_string(),
            Self::CannotRevokePermissionShareCard => self.to_string(),
            Self::CannotRevokeEnvironmentDefaultCard => self.to_string(),
            Self::CardOwnerNotFound(_) => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(CardError);

impl From<CardRepoError> for CardError {
    fn from(value: CardRepoError) -> Self {
        match value {
            CardRepoError::CardTreeChangedDuringDelete | CardRepoError::ConcurrentModification => {
                Self::ConcurrentModification
            }
            other => Self::InternalError(other.into_anyhow()),
        }
    }
}

impl From<AccountError> for CardError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::AccountNotFound(account_id) => Self::AccountNotFound(account_id),
            AccountError::Unauthorized(inner) => Self::Unauthorized(inner),
            other => Self::InternalError(other.into_anyhow()),
        }
    }
}

impl From<PermissionShareError> for CardError {
    fn from(value: PermissionShareError) -> Self {
        match value {
            PermissionShareError::PermissionShareNotFound(_) => Self::ConcurrentModification,
            PermissionShareError::Unauthorized(inner) => Self::Unauthorized(inner),
            other => Self::InternalError(other.into_anyhow()),
        }
    }
}

impl From<ComponentError> for CardError {
    fn from(value: ComponentError) -> Self {
        Self::InternalError(value.into_anyhow())
    }
}

impl From<EnvironmentError> for CardError {
    fn from(value: EnvironmentError) -> Self {
        Self::InternalError(value.into_anyhow())
    }
}

impl From<RepoError> for CardError {
    fn from(value: RepoError) -> Self {
        Self::InternalError(value.into_anyhow())
    }
}

pub struct CardService {
    card_repo: Arc<dyn CardRepo>,
    account_service: Arc<AccountService>,
    permission_share_service: Arc<PermissionShareService>,
    component_service: Arc<ComponentService>,
    environment_service: Arc<EnvironmentService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountCardFilter {
    pub root: bool,
    pub permission_share: bool,
    pub environment_default: bool,
    pub agent_initial: bool,
}

impl AccountCardFilter {
    pub const ALL: Self = Self {
        root: true,
        permission_share: true,
        environment_default: true,
        agent_initial: true,
    };
}

impl CardService {
    pub fn new(
        card_repo: Arc<dyn CardRepo>,
        account_service: Arc<AccountService>,
        permission_share_service: Arc<PermissionShareService>,
        component_service: Arc<ComponentService>,
        environment_service: Arc<EnvironmentService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            card_repo,
            account_service,
            permission_share_service,
            component_service,
            environment_service,
            registry_change_notifier,
        }
    }

    pub async fn create_agent_initial_card(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: AgentTypeName,
        card: &PolymorphicCard,
    ) -> Result<CardId, CardRepoError> {
        let card_id = card.card_id;
        self.card_repo
            .create(CardRecord::polymorphic_creation(
                card_id,
                card.parent_ids.clone(),
                card.lower_positive.clone(),
                card.lower_negative.clone(),
                card.upper_positive.clone(),
                card.upper_negative.clone(),
                card.expires_at,
                card.system_card,
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision,
                    agent_type,
                })),
            ))
            .await?;

        Ok(card_id)
    }

    pub async fn existing(&self, card_ids: Vec<CardId>) -> Result<Vec<CardId>, CardError> {
        let mut result = Vec::new();
        for card_id in card_ids {
            match self.materialized_card_unchecked(card_id).await {
                Ok(card) => result.push(card.card.card_id()),
                Err(CardError::CardNotFound(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(result)
    }

    pub async fn get_cards(&self, card_ids: Vec<CardId>) -> Result<Vec<StoredCard>, CardError> {
        let mut result = Vec::new();
        for card_id in card_ids {
            match self.materialized_card_unchecked(card_id).await {
                Ok(card) => result.push(card.card),
                Err(CardError::CardNotFound(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(result)
    }

    pub async fn list_account_cards(
        &self,
        account_id: AccountId,
        filter: AccountCardFilter,
        auth: &AuthCtx,
    ) -> Result<Vec<StoredCard>, CardError> {
        let account = self
            .account_service
            .get(account_id, &AuthCtx::System)
            .await?;
        authorize_card_permission(auth, &account.email, CardVerb::Inspect)?;

        let mut cards = Vec::new();
        if filter.root {
            cards.push(
                self.get_card_unchecked(account.account_root_card_id)
                    .await?
                    .card,
            );
        }
        if filter.permission_share {
            cards.extend(
                self.permission_share_service
                    .active_share_cards_for_target(account_id)
                    .await?
                    .into_iter()
                    .map(StoredCard::Concrete),
            );
        }
        if filter.environment_default {
            cards.extend(
                self.environment_service
                    .list_default_card_refs_by_account(account_id)
                    .await?
                    .into_iter()
                    .map(environment_default_card_from_ref),
            );
        }
        if filter.agent_initial {
            cards.extend(
                self.get_cards(
                    self.component_service
                        .list_initial_permission_card_ids_by_account(account_id)
                        .await?,
                )
                .await?,
            );
        }

        Ok(cards)
    }

    pub async fn get_card(&self, card_id: CardId, auth: &AuthCtx) -> Result<StoredCard, CardError> {
        let card = self.materialized_card_unchecked(card_id).await?;
        let owner_email = self
            .card_owner_email(card_id, card.managed_by.as_ref())
            .await?;
        authorize_card_permission(auth, &owner_email, CardVerb::Inspect)?;
        Ok(card.card)
    }

    pub async fn revoke_card(
        &self,
        card_id: CardId,
        auth: &AuthCtx,
    ) -> Result<Vec<CardId>, CardError> {
        let card = self.materialized_card_unchecked(card_id).await?;

        if matches!(card.managed_by, Some(CardManagedBy::PermissionShare(_))) {
            return Err(CardError::CannotRevokePermissionShareCard);
        }

        if matches!(card.managed_by, Some(CardManagedBy::EnvironmentDefault(_))) {
            return Err(CardError::CannotRevokeEnvironmentDefaultCard);
        }

        if card.card.system_card() {
            return Err(CardError::CannotRevokeSystemCard);
        }

        let owner_email = self
            .card_owner_email(card_id, card.managed_by.as_ref())
            .await?;
        authorize_card_permission(auth, &owner_email, CardVerb::Revoke)?;

        let deleted = self
            .card_repo
            .delete(card_id)
            .await?
            .signal_new_events_available(&self.registry_change_notifier);

        if deleted.is_empty() {
            return Err(CardError::ConcurrentModification);
        }

        Ok(deleted)
    }

    async fn get_card_unchecked(&self, card_id: CardId) -> Result<CardWithManagedBy, CardError> {
        self.card_repo
            .get(card_id)
            .await?
            .ok_or(CardError::CardNotFound(card_id))?
            .try_into()
    }

    async fn materialized_card_unchecked(
        &self,
        card_id: CardId,
    ) -> Result<CardWithManagedBy, CardError> {
        let card = self.get_card_unchecked(card_id).await?;
        let Some(managed_by) = card.managed_by.as_ref() else {
            return Ok(card);
        };

        let CardManagedBy::EnvironmentDefault(managed_by) = managed_by else {
            if let CardManagedBy::AgentInitial(managed_by) = managed_by {
                let component = self
                    .component_service
                    .get_component_revision(
                        managed_by.component_id,
                        managed_by.component_revision,
                        false,
                        &AuthCtx::System,
                    )
                    .await
                    .map_err(|err| match err {
                        ComponentError::ComponentNotFound(_) => CardError::CardNotFound(card_id),
                        other => other.into(),
                    })?;
                let current_agent_initial_card_ids = self
                    .component_service
                    .list_initial_permission_card_ids_by_account(component.account_id)
                    .await?;
                if !current_agent_initial_card_ids.contains(&card_id) {
                    return Err(CardError::CardNotFound(card_id));
                }
            }
            return Ok(card);
        };

        let Some(default_card_ref) = self
            .environment_service
            .default_card_ref_by_environment(managed_by.environment_id)
            .await?
        else {
            return Err(CardError::CardNotFound(card_id));
        };

        if default_card_ref.card_id != card_id {
            return Err(CardError::CardNotFound(card_id));
        }

        Ok(CardWithManagedBy {
            managed_by: Some(CardManagedBy::EnvironmentDefault(
                CardManagedByEnvironmentDefault {
                    environment_id: default_card_ref.environment_id,
                },
            )),
            card: environment_default_card_from_ref(default_card_ref),
        })
    }

    async fn card_owner_email(
        &self,
        card_id: CardId,
        managed_by: Option<&CardManagedBy>,
    ) -> Result<AccountEmail, CardError> {
        match managed_by {
            Some(CardManagedBy::AccountRoot(managed_by)) => Ok(self
                .account_service
                .get(managed_by.account_id, &AuthCtx::System)
                .await?
                .email),
            Some(CardManagedBy::EnvironmentDefault(managed_by)) => Ok(self
                .environment_service
                .default_card_ref_by_environment(managed_by.environment_id)
                .await?
                .ok_or(CardError::CardNotFound(card_id))?
                .account_email),
            Some(CardManagedBy::PermissionShare(managed_by)) => self
                .permission_share_service
                .target_account_email(PermissionShareId(managed_by.permission_share_id))
                .await
                .map_err(Into::into),
            Some(CardManagedBy::AgentInitial(managed_by)) => Ok(self
                .component_service
                .get_component_revision(
                    managed_by.component_id,
                    managed_by.component_revision,
                    true,
                    &AuthCtx::System,
                )
                .await?
                .account_email),
            None => Err(CardError::CardOwnerNotFound(card_id)),
        }
    }
}

struct CardWithManagedBy {
    card: StoredCard,
    managed_by: Option<CardManagedBy>,
}

impl TryFrom<CardRecord> for CardWithManagedBy {
    type Error = CardError;

    fn try_from(value: CardRecord) -> Result<Self, Self::Error> {
        let managed_by = value
            .managed_by
            .clone()
            .map(|managed_by| managed_by.into_value());
        Ok(Self {
            card: value.try_into()?,
            managed_by,
        })
    }
}

fn environment_default_card_from_ref(default_card_ref: EnvironmentDefaultCardRef) -> StoredCard {
    let environment_owner = EnvironmentOwnerPattern::Environment {
        account: default_card_ref.account_email.clone(),
        application: default_card_ref.application_name.clone(),
        environment: default_card_ref.environment_name.clone(),
    };
    let component_owner = ComponentOwnerPattern::EnvironmentComponents {
        account: default_card_ref.account_email.clone(),
        application: default_card_ref.application_name,
        environment: default_card_ref.environment_name,
    };
    let recipient = RecipientPattern::Account {
        account: default_card_ref.account_email,
    };
    let managed_by = Some(CardManagedBy::EnvironmentDefault(
        CardManagedByEnvironmentDefault {
            environment_id: default_card_ref.environment_id,
        },
    ));

    StoredCard::Concrete(Card {
        card_id: default_card_ref.card_id,
        parent_ids: Vec::new(),
        lower_positive: vec![
            PermissionPattern::Environment(ClassPermissionPattern {
                verb: None,
                owner: environment_owner,
                recipient: recipient.clone(),
                resource: EnvironmentResourcePattern::Any,
            }),
            PermissionPattern::Component(ClassPermissionPattern {
                verb: None,
                owner: component_owner,
                recipient,
                resource: ComponentResourcePattern::Any,
            }),
        ],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: default_card_ref.created_at,
        expires_at: default_card_ref.expires_at,
        system_card: default_card_ref.system_card,
        managed_by,
    })
}

fn authorize_card_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    verb: CardVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Card(ClassPermissionTarget {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: account_email.clone(),
        },
        resource: CardResourcePattern::Any,
    }))
}
