// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::oauth2_provider_client::{OAuth2ProviderClient, OAuth2ProviderClientError};
use super::model::{ExternalLogin, OAuth2Provider, OAuth2Token, OAuth2WeblowStateMetadata, Oauth2WeblowState};
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::auth::{TokenSecret, TokenWithSecret};
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tracing::debug;
use tracing::error;
use super::error::LoginError;
use golem_common::model::auth::TokenId;
use crate::services::account::AccountService;
use crate::services::token::TokenService;
use golem_common::model::account::{AccountId, NewAccountData};
use anyhow::anyhow;
use super::OAuth2TokenRepo;
use super::oauth2_webflow_state_repo::OAuth2WebflowStateRepo;
use crate::repo::model::oauth2_token::OAuth2TokenRecord;

pub struct LoginService {
    client: Arc<dyn OAuth2ProviderClient>,
    account_service: Arc<AccountService>,
    token_service: Arc<TokenService>,
    oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
    oauth2_web_flow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
}

impl LoginService {
    pub fn new(
        client: Arc<dyn OAuth2ProviderClient>,
        account_service: Arc<AccountService>,
        token_service: Arc<TokenService>,
        oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
        oauth2_web_flow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
    ) -> Self {
        Self {
            client,
            account_service,
            token_service,
            oauth2_token_repo,
            oauth2_web_flow_state_repo,
        }
    }

    pub async fn oauth2(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<TokenWithSecret, LoginError> {
        self.oauth2_web_flow_state_repo
            .delete_expired(Utc::now().into())
            .await?;

        let external_login = self.client.external_user_id(provider, access_token).await?;

        let existing_data = self
            .oauth2_token_repo
            .get(&provider.to_string(), &external_login.external_id)
            .await?
            .map(TryInto::<OAuth2Token>::try_into)
            .transpose()
            .map_err(LoginError::Internal)?;

        let account_id = match existing_data.clone() {
            Some(token) => token.account_id,
            None => self.make_account(provider, &external_login).await?,
        };

        let unsafe_token = match existing_data.and_then(|token| token.token_id) {
            Some(token_id) => self.token_service.get_unsafe(&token_id).await?,
            None => {
                self.make_token(provider, &external_login, &account_id)
                    .await?
            }
        };
        Ok(unsafe_token)
    }

    pub async fn generate_temp_token_state(
        &self,
        redirect: Option<url::Url>,
    ) -> Result<String, LoginError> {
        let metadata = OAuth2WeblowStateMetadata { redirect };

        let token_state = self
            .oauth2_web_flow_state_repo
            .create(metadata)
            .await?;

        Ok(token_state)
    }

    pub async fn link_temp_token(
        &self,
        token_id: &TokenId,
        state: &str,
    ) -> Result<UnsafeTokenWithMetadata, LoginError> {
        debug!("Get link temp token {}", token_id);
        self.oauth2_web_flow_state_repo
            .delete_expired_states()
            .await?;

        let linked_token = self
            .oauth2_web_flow_state_repo
            .set_token_id(&token_id.0, state)
            .await?
            .ok_or(LoginError::UnknownTokenState(state.to_string()))?;


        let token = UnsafeTokenWithMetadata::try_from(linked_token).map_err(|e| {
            LoginError::InternalSerializationError {
                error: e,
                context: "Failed to deserialize temp token".to_string(),
            }
        })?;

        Ok(token)
    }

    pub async fn unlink_temp_token(&self, token_id: &TokenId) -> Result<(), LoginError> {
        debug!("Unlink temp token id {}", token_id);

        let tokens = self.oauth2_token_repo.get_by_token_id(&token_id.0).await?;

        // it is not expected that there will be more than one token records with same token_id and different account_id
        for token in tokens {
            self.oauth2_token_repo
                .clean_token_id(&token.provider, &token.external_id)
                .await?;
        }

        Ok(())
    }

    pub async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, LoginError> {
        debug!("Get temp token by state");
        self.oauth2_web_flow_state_repo
            .delete_expired_states()
            .await?;

        let token_state = self
            .oauth2_web_flow_state_repo
            .get_temp_token(state)
            .await?;

        match token_state {
            LinkedTokenState::Linked(linked_token) => {
                let token = UnsafeTokenWithMetadata::try_from(linked_token).map_err(|e| {
                    LoginError::InternalSerializationError {
                        error: e,
                        context: "Failed to deserialize temp token".to_string(),
                    }
                })?;
                Ok(Some(token))
            }
            LinkedTokenState::Pending => Ok(None),
            LinkedTokenState::NotFound => Err(LoginError::UnknownTokenState(state.to_string())),
        }
    }

    async fn make_account(
        &self,
        _provider: &OAuth2Provider,
        external_login: &ExternalLogin,
    ) -> Result<AccountId, LoginError> {
        let email = external_login
            .email
            .clone()
            .ok_or(anyhow!("No user email from OAuth2 Provider for login {}", external_login.external_id))?;

        let name = external_login
            .name
            .clone()
            .unwrap_or(external_login.external_id.clone());

        let account = self
            .account_service
            .create(NewAccountData { name, email })
            .await?;

        Ok(account.id)
    }

    async fn make_token(
        &self,
        provider: &OAuth2Provider,
        external_login: &ExternalLogin,
        account_id: AccountId,
    ) -> Result<TokenWithSecret, LoginError> {
        let expiration = Utc::now()
            // Ten years.
            .checked_add_months(chrono::Months::new(10 * 12))
            .ok_or(anyhow!("Failed to calculate token expiry"))?;

        let unsafe_token = self.token_service.create(account_id, expiration).await?;

        {
            let token: TokenSecret = unsafe_token.secret.clone();

            let oauth2_token = OAuth2Token {
                provider: provider.clone(),
                external_id: external_login.external_id.clone(),
                account_id: account_id.clone(),
                token_id: Some(unsafe_token.id),
            };

            let record: OAuth2TokenRecord = oauth2_token.into();

            self.oauth2_token_repo.upsert(&record).await?;
        }

        Ok(unsafe_token)
    }
}

// #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
// pub struct TempTokenMetadata {
//     pub redirect: Option<url::Url>,
// }

// #[derive(Debug, Clone)]
// pub struct UnsafeTokenWithMetadata {
//     pub token: TokenWithSecret,
//     pub metadata: TempTokenMetadata,
// }

// impl TryFrom<super::oauth2_web_flow_state_repo::LinkedToken> for UnsafeTokenWithMetadata {
//     type Error = serde_json::Error;

//     fn try_from(
//         linked_token: super::oauth2_web_flow_state_repo::LinkedToken,
//     ) -> Result<Self, Self::Error> {
//         let secret: TokenSecret = TokenSecret::new(linked_token.token.secret);
//         let metadata: TempTokenMetadata = serde_json::from_slice(&linked_token.metadata)?;
//         let token = UnsafeToken {
//             data: linked_token.token.into(),
//             secret,
//         };
//         Ok(UnsafeTokenWithMetadata { token, metadata })
//     }
// }
