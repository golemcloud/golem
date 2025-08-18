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

use super::account::{AccountError, AccountService};
use super::oauth2_github_client::{
    DeviceWorkflowData, OAuth2GithubClient, OAuth2GithubClientError,
};
use super::token::{TokenError, TokenService};
use crate::config::EdDsaConfig;
use crate::model::login::{
    ExternalLogin, OAuth2DeviceFlowSession, OAuth2Token, OAuth2WebflowState,
    OAuth2WebflowStateMetadata,
};
use crate::repo::model::oauth2_token::OAuth2TokenRecord;
use crate::repo::oauth2_token::OAuth2TokenRepo;
use crate::repo::oauth2_webflow_state::OAuth2WebflowStateRepo;
use anyhow::anyhow;
use applying::Apply;
use chrono::Utc;
use golem_common::model::account::{AccountId, NewAccountData};
use golem_common::model::auth::TokenWithSecret;
use golem_common::model::login::{
    EncodedOAuth2Session, OAuth2DeviceFlowData, OAuth2Provider, OAuth2WebWorkflowData,
    OAuth2WebflowStateId,
};
use golem_common::{SafeDisplay, error_forwarders, into_internal_error};
use golem_service_base::repo::RepoError;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2Error {
    #[error("Invalid encoded oauth2 session: {}", 0.to_string())]
    InvalidSession(jsonwebtoken::errors::Error),
    #[error("OAuth2 web flow state not found: {0}")]
    OAuth2WebflowStateNotFound(OAuth2WebflowStateId),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for OAuth2Error {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidSession(_) => self.to_string(),
            Self::OAuth2WebflowStateNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal Error".to_string(),
        }
    }
}

into_internal_error!(OAuth2Error);

error_forwarders!(
    OAuth2Error,
    OAuth2GithubClientError,
    RepoError,
    AccountError,
    TokenError
);

pub struct OAuth2Service {
    client: Arc<dyn OAuth2GithubClient>,
    account_service: Arc<AccountService>,
    token_service: Arc<TokenService>,
    oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
    oauth2_web_flow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl OAuth2Service {
    pub fn new(
        client: Arc<dyn OAuth2GithubClient>,
        account_service: Arc<AccountService>,
        token_service: Arc<TokenService>,
        oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
        oauth2_web_flow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
        config: &EdDsaConfig,
    ) -> Result<Self, OAuth2Error> {
        let private_key = format_key(config.private_key.as_str(), "PRIVATE");
        let public_key = format_key(config.public_key.as_str(), "PUBLIC");

        let encoding_key =
            EncodingKey::from_ed_pem(private_key.as_bytes()).map_err(anyhow::Error::from)?;

        let decoding_key =
            DecodingKey::from_ed_pem(public_key.as_bytes()).map_err(anyhow::Error::from)?;

        Ok(Self {
            client,
            account_service,
            token_service,
            encoding_key,
            decoding_key,
            oauth2_token_repo,
            oauth2_web_flow_state_repo,
        })
    }

    pub async fn start_web_workflow(
        &self,
        provider: &OAuth2Provider,
        redirect: Option<url::Url>,
    ) -> Result<OAuth2WebWorkflowData, OAuth2Error> {
        let metadata = OAuth2WebflowStateMetadata {
            redirect,
            provider: provider.clone(),
        };

        let state = self
            .oauth2_web_flow_state_repo
            .create(metadata)
            .await?
            .state_id
            .apply(OAuth2WebflowStateId);

        let url = self.get_authorize_url(provider, &state).await?;

        Ok(OAuth2WebWorkflowData { url, state })
    }

    pub async fn handle_web_workflow_callback(
        &self,
        state_id: &OAuth2WebflowStateId,
        code: String,
    ) -> Result<OAuth2WebflowStateMetadata, OAuth2Error> {
        let state: OAuth2WebflowState = self
            .oauth2_web_flow_state_repo
            .get_by_id(&state_id.0)
            .await?
            .ok_or(OAuth2Error::OAuth2WebflowStateNotFound(state_id.clone()))?
            .into();

        let access_token = self
            .exchange_code_for_token(&state.metadata.provider, &code, state_id)
            .await?;

        let external_login = self
            .get_external_login(&state.metadata.provider, &access_token)
            .await?;

        let existing_data = self
            .oauth2_token_repo
            .get_by_external_provider(
                &state.metadata.provider.to_string(),
                &external_login.external_id,
            )
            .await?
            .map(TryInto::<OAuth2Token>::try_into)
            .transpose()?;

        let account_id = match &existing_data {
            Some(token) => token.account_id.clone(),
            None => self.make_account(&external_login).await?,
        };

        let token = match existing_data.and_then(|token| token.token_id) {
            Some(token_id) => self.token_service.get(&token_id).await?,
            None => {
                // This will also link the external id to the account id, ensure that no additional
                // accounts are created in the future.
                self.make_token(state.metadata.provider.clone(), external_login, account_id)
                    .await?
            }
        };

        self.oauth2_web_flow_state_repo
            .set_token_id(&state_id.0, &token.id.0)
            .await?;

        Ok(state.metadata)
    }

    pub async fn get_web_workflow_state(
        &self,
        state_id: &OAuth2WebflowStateId,
    ) -> Result<OAuth2WebflowState, OAuth2Error> {
        let state: OAuth2WebflowState = self
            .oauth2_web_flow_state_repo
            .get_by_id(&state_id.0)
            .await?
            .ok_or(OAuth2Error::OAuth2WebflowStateNotFound(state_id.clone()))?
            .into();

        Ok(state)
    }

    pub async fn start_device_workflow(
        &self,
        provider: OAuth2Provider,
    ) -> Result<OAuth2DeviceFlowData, OAuth2Error> {
        let data = self.initiate_device_workflow(&provider).await?;
        let now = chrono::Utc::now();
        let session = OAuth2DeviceFlowSession {
            provider,
            device_code: data.device_code,
            interval: data.interval,
            expires_at: now + data.expires_in,
        };
        let encoded_session = self.encode_session(&session)?;

        Ok(OAuth2DeviceFlowData {
            url: data.verification_uri,
            user_code: data.user_code,
            expires: session.expires_at,
            encoded_session,
        })
    }

    pub async fn finish_device_workflow(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<TokenWithSecret, OAuth2Error> {
        let session = self.decode_session(encoded_session)?;
        let access_token = self
            .client
            .get_device_workflow_access_token(
                &session.device_code,
                session.interval,
                session.expires_at,
            )
            .await?;

        let external_login = self.client.get_external_login(&access_token).await?;

        let existing_data = self
            .oauth2_token_repo
            .get_by_external_provider(&session.provider.to_string(), &external_login.external_id)
            .await?
            .map(TryInto::<OAuth2Token>::try_into)
            .transpose()?;

        let account_id = match &existing_data {
            Some(token) => token.account_id.clone(),
            None => self.make_account(&external_login).await?,
        };

        let token = match existing_data.and_then(|token| token.token_id) {
            Some(token_id) => self.token_service.get(&token_id).await?,
            None => {
                // This will also link the external id to the account id, ensure that no additional
                // accounts are created in the future.
                self.make_token(session.provider, external_login, account_id)
                    .await?
            }
        };

        Ok(token)
    }

    async fn initiate_device_workflow(
        &self,
        provider: &OAuth2Provider,
    ) -> Result<DeviceWorkflowData, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => {
                let data = self.client.initiate_device_workflow().await?;
                Ok(data)
            }
        }
    }

    async fn get_authorize_url(
        &self,
        provider: &OAuth2Provider,
        state: &OAuth2WebflowStateId,
    ) -> Result<String, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => Ok(self.client.get_authorize_url(state).await),
        }
    }

    async fn exchange_code_for_token(
        &self,
        provider: &OAuth2Provider,
        code: &str,
        state: &OAuth2WebflowStateId,
    ) -> Result<String, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => Ok(self.client.exchange_code_for_token(code, state).await?),
        }
    }

    async fn get_external_login(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => Ok(self.client.get_external_login(access_token).await?),
        }
    }

    fn encode_session(
        &self,
        session: &OAuth2DeviceFlowSession,
    ) -> Result<EncodedOAuth2Session, OAuth2Error> {
        let header = Header::new(Algorithm::EdDSA);
        let encoded = jsonwebtoken::encode(&header, session, &self.encoding_key)
            .map_err(anyhow::Error::from)?;

        Ok(EncodedOAuth2Session(encoded))
    }

    fn decode_session(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2DeviceFlowSession, OAuth2Error> {
        let validation = Validation::new(Algorithm::EdDSA);
        let session = jsonwebtoken::decode::<OAuth2DeviceFlowSession>(
            &encoded_session.0,
            &self.decoding_key,
            &validation,
        )
        .map_err(OAuth2Error::InvalidSession)?;

        Ok(session.claims)
    }

    async fn make_account(&self, external_login: &ExternalLogin) -> Result<AccountId, OAuth2Error> {
        let email = external_login.email.clone().ok_or(anyhow!(
            "No user email from OAuth2 Provider for login {}",
            external_login.external_id
        ))?;

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
        provider: OAuth2Provider,
        external_login: ExternalLogin,
        account_id: AccountId,
    ) -> Result<TokenWithSecret, OAuth2Error> {
        let expiration = Utc::now()
            // Ten years.
            .checked_add_months(chrono::Months::new(10 * 12))
            .ok_or(anyhow!("Failed to calculate token expiry"))?;

        let token_with_secret = self
            .token_service
            .create(account_id.clone(), expiration)
            .await?;

        {
            let oauth2_token = OAuth2Token {
                provider,
                external_id: external_login.external_id,
                account_id: account_id.clone(),
                token_id: Some(token_with_secret.id.clone()),
            };

            let record: OAuth2TokenRecord = oauth2_token.into();

            self.oauth2_token_repo.create_or_update(record).await?;
        }

        Ok(token_with_secret)
    }
}

/// Formats a cryptographic key with PEM (Privacy Enhanced Mail) encoding delimiters.
///
/// # Arguments
/// * `key: &str` - The raw key content to be formatted. This should not include any PEM encoding delimiters.
/// * `key_type: &str` - The type of the key. Acceptable values are "PUBLIC" or "PRIVATE", case-insensitive.
///
/// # Returns
/// A String containing the key formatted with PEM encoding delimiters.
/// If the key is already in the correct PEM format, it is returned unchanged.
/// Otherwise, it adds "-----BEGIN {} KEY-----" and "-----END {} KEY-----" around the key, with `{}` replaced by the specified key type.
fn format_key(key: &str, key_type: &str) -> String {
    let key_type = key_type.to_uppercase();
    let begin_marker = format!("-----BEGIN {key_type} KEY-----");
    let end_marker = format!("-----END {key_type} KEY-----");

    if key.trim_start().starts_with(&begin_marker) && key.trim_end().ends_with(&end_marker) {
        key.to_string()
    } else {
        format!("{begin_marker}\n{key}\n{end_marker}")
    }
}
