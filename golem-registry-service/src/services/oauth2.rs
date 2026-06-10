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
use super::oauth2_github_client::{OAuth2GithubClient, OAuth2GithubClientError};
use super::token::{TokenError, TokenService};
use crate::config::OAuth2Config;
use crate::model::login::{
    ExternalLogin, OAuth2Token, OAuth2WebflowState, OAuth2WebflowStateMetadata, WebflowKind,
};
use crate::repo::model::oauth2_token::OAuth2TokenRecord;
use crate::repo::oauth2_token::OAuth2TokenRepo;
use crate::repo::oauth2_webflow_state::OAuth2WebflowStateRepo;
use anyhow::anyhow;
use applying::Apply;
use chrono::{Duration, Utc};
use golem_common::model::account::{AccountCreation, AccountEmail, AccountId};
use golem_common::model::auth::TokenWithSecret;
use golem_common::model::login::{OAuth2Provider, OAuth2WebflowData, OAuth2WebflowStateId};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tap::Pipe;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2Error {
    #[error("Invalid redirect domain: {0}")]
    InvalidRedirectDomain(String),
    #[error("OAuth2 web flow state not found: {0}")]
    OAuth2WebflowStateNotFound(OAuth2WebflowStateId),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

/// The action the callback endpoint should take after completing the webflow.
pub enum WebflowCallbackAction {
    /// Browser flow: redirect to `redirect` with the token secret appended as `token=<secret>`.
    BrowserRedirect { redirect: url::Url },
    /// CLI flow: redirect the browser to the server's configured CLI redirect URL.
    CliRedirect { redirect: url::Url },
}

impl SafeDisplay for OAuth2Error {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidRedirectDomain(_) => self.to_string(),
            Self::OAuth2WebflowStateNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal Error".to_string(),
        }
    }
}

error_forwarding!(
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
    webflow_state_expiry: Duration,
    cli_redirect: url::Url,
    allowed_redirect_domains: Vec<String>,
}

impl OAuth2Service {
    pub fn new(
        client: Arc<dyn OAuth2GithubClient>,
        account_service: Arc<AccountService>,
        token_service: Arc<TokenService>,
        oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
        oauth2_web_flow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
        config: &OAuth2Config,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            client,
            account_service,
            token_service,
            oauth2_token_repo,
            oauth2_web_flow_state_repo,
            webflow_state_expiry: Duration::from_std(config.webflow_state_expiry)?,
            cli_redirect: config.cli_redirect.clone(),
            allowed_redirect_domains: config.allowed_redirect_domains.clone(),
        })
    }

    pub async fn exchange_external_access_token_for_token(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<TokenWithSecret, OAuth2Error> {
        let external_login = self.get_external_login(provider, access_token).await?;

        let existing_data = self
            .oauth2_token_repo
            .get_by_external_provider(&provider.to_string(), &external_login.external_id)
            .await?
            .map(TryInto::<OAuth2Token>::try_into)
            .transpose()?;

        let token = match existing_data {
            Some(OAuth2Token {
                account_id,
                token_id: Some(token_id),
                ..
            }) => match self.token_service.get(token_id, &AuthCtx::system()).await {
                Ok(token) => token,
                // The token was deleted; create a new one for the existing account.
                Err(TokenError::TokenNotFound(_)) => {
                    self.make_token(*provider, external_login, account_id)
                        .await?
                }
                Err(e) => return Err(e.into()),
            },
            Some(OAuth2Token { account_id, .. }) => {
                self.make_token(*provider, external_login, account_id)
                    .await?
            }
            None => {
                let account_id = self.make_account(&external_login).await?;
                // This will also link the external id to the account id, ensuring that no
                // additional accounts are created in the future.
                self.make_token(*provider, external_login, account_id)
                    .await?
            }
        };

        Ok(token)
    }

    pub async fn start_webflow(
        &self,
        provider: &OAuth2Provider,
        kind: WebflowKind,
    ) -> Result<OAuth2WebflowData, OAuth2Error> {
        // Validate the redirect URL domain for browser flows.
        if let WebflowKind::Browser { ref redirect } = kind {
            self.validate_redirect_url(redirect)?;
        }

        let metadata = OAuth2WebflowStateMetadata {
            provider: *provider,
            kind,
        };

        let state = self
            .oauth2_web_flow_state_repo
            .create(metadata)
            .await?
            .state_id
            .apply(OAuth2WebflowStateId);

        let url = self.get_authorize_url(provider, &state).await?;

        Ok(OAuth2WebflowData { url, state })
    }

    fn validate_redirect_url(&self, url: &url::Url) -> Result<(), OAuth2Error> {
        let domain = url.domain().unwrap_or("");
        let allowed = self
            .allowed_redirect_domains
            .iter()
            .any(|allowed| domain == allowed || domain.ends_with(&format!(".{allowed}")));
        if allowed {
            Ok(())
        } else {
            Err(OAuth2Error::InvalidRedirectDomain(domain.to_string()))
        }
    }

    pub async fn handle_webflow_callback(
        &self,
        state_id: &OAuth2WebflowStateId,
        code: String,
    ) -> Result<WebflowCallbackAction, OAuth2Error> {
        self.oauth2_web_flow_state_repo
            .delete_expired((Utc::now() - self.webflow_state_expiry).into())
            .await?;

        let state: OAuth2WebflowState = self
            .oauth2_web_flow_state_repo
            .get_by_id(state_id.0)
            .await?
            .ok_or(OAuth2Error::OAuth2WebflowStateNotFound(*state_id))?
            .into();

        let access_token = self
            .exchange_code_for_token(&state.metadata.provider, &code, state_id)
            .await?;

        let token = self
            .exchange_external_access_token_for_token(&state.metadata.provider, &access_token)
            .await?;

        let action = match state.metadata.kind {
            WebflowKind::Browser { mut redirect } => {
                // Consume the state immediately — no polling needed.
                self.oauth2_web_flow_state_repo
                    .delete_by_id(state_id.0)
                    .await?;
                redirect
                    .query_pairs_mut()
                    .append_pair("token", token.secret.secret());
                WebflowCallbackAction::BrowserRedirect { redirect }
            }
            WebflowKind::Cli => {
                // Store the token for the CLI to pick up via polling.
                self.oauth2_web_flow_state_repo
                    .set_token_id(state_id.0, token.id.0)
                    .await?;
                WebflowCallbackAction::CliRedirect {
                    redirect: self.cli_redirect.clone(),
                }
            }
        };

        Ok(action)
    }

    pub async fn exchange_webflow_state_for_token(
        &self,
        state_id: &OAuth2WebflowStateId,
    ) -> Result<OAuth2WebflowState, OAuth2Error> {
        self.oauth2_web_flow_state_repo
            .delete_expired((Utc::now() - self.webflow_state_expiry).into())
            .await?;

        let state: OAuth2WebflowState = self
            .oauth2_web_flow_state_repo
            .get_by_id(state_id.0)
            .await?
            .ok_or(OAuth2Error::OAuth2WebflowStateNotFound(*state_id))?
            .into();

        // State is only allowed to be exchanged once for access tokens.
        // If we found a token attached to this state, invalidate it for future use.
        if state.token.is_some() {
            self.oauth2_web_flow_state_repo
                .delete_by_id(state_id.0)
                .await?;
        }

        Ok(state)
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

    async fn make_account(&self, external_login: &ExternalLogin) -> Result<AccountId, OAuth2Error> {
        let email = external_login
            .email
            .clone()
            .ok_or(anyhow!(
                "No user email from OAuth2 Provider for login {}",
                external_login.external_id
            ))?
            .pipe(AccountEmail::new);

        let name = external_login
            .name
            .clone()
            .unwrap_or(external_login.external_id.clone());

        let account = self
            .account_service
            .create(
                AccountCreation {
                    name,
                    email,
                    roles: Vec::new(),
                },
                &AuthCtx::system(),
            )
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
            .create(account_id, expiration, &AuthCtx::system())
            .await?;

        {
            let oauth2_token = OAuth2Token {
                provider,
                external_id: external_login.external_id,
                account_id,
                token_id: Some(token_with_secret.id),
            };

            let record: OAuth2TokenRecord = oauth2_token.into();

            self.oauth2_token_repo.create_or_update(record).await?;
        }

        Ok(token_with_secret)
    }
}
