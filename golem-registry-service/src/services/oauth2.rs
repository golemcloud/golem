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

use super::oauth2_github_client::{OAuth2GithubClient, OAuth2GithubClientError};
use crate::model::login::{
    EncodedOAuth2Session, OAuth2AccessToken, OAuth2Data, OAuth2Provider, OAuth2Session,
};
use golem_common::{error_forwarders, into_internal_error, SafeDisplay};
use std::sync::Arc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde_with::serde_as;
use serde::{Deserialize, Serialize};
use crate::config::EdDsaConfig;
use anyhow::anyhow;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2Error {
    #[error("Invalid encoded oauth2 session: {}", 0.to_string())]
    InvalidSession(jsonwebtoken::errors::Error),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error)
}

impl SafeDisplay for OAuth2Error {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidSession(_) => self.to_string(),
            Self::InternalError(_) => "Internal Error".to_string()
        }
    }
}

into_internal_error!(OAuth2Error);

error_forwarders!(OAuth2Error, OAuth2GithubClientError);

pub struct OAuth2Service {
    client: Arc<dyn OAuth2GithubClient>,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl OAuth2Service {
    pub fn new(
        client: Arc<dyn OAuth2GithubClient>,
        config: &EdDsaConfig,
    ) -> Result<Self, OAuth2Error> {
        let private_key = format_key(config.private_key.as_str(), "PRIVATE");
        let public_key = format_key(config.public_key.as_str(), "PUBLIC");

        let encoding_key = EncodingKey::from_ed_pem(private_key.as_bytes()).map_err(anyhow::Error::from)?;

        let decoding_key = DecodingKey::from_ed_pem(public_key.as_bytes()).map_err(anyhow::Error::from)?;

        Ok(Self {
            client,
            encoding_key,
            decoding_key
        })
    }

    pub async fn start_workflow(&self) -> Result<OAuth2Data, OAuth2Error> {
        let data = self.client.initiate_device_workflow().await?;
        let now = chrono::Utc::now();
        let session = OAuth2Session {
            device_code: data.device_code,
            interval: data.interval,
            expires_at: now + data.expires_in,
        };
        let encoded_session = self.encode_session(&session)?;
        Ok(OAuth2Data {
            url: data.verification_uri,
            user_code: data.user_code,
            expires: session.expires_at,
            encoded_session: encoded_session.value,
        })
    }

    pub async fn finish_workflow(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2AccessToken, OAuth2Error> {
        let session = self.decode_session(encoded_session)?;
        let access_token = self
            .client
            .get_access_token(&session.device_code, session.interval, session.expires_at)
            .await?;
        Ok(OAuth2AccessToken {
            provider: OAuth2Provider::Github,
            access_token,
        })
    }

    pub async fn get_authorize_url(
        &self,
        provider: OAuth2Provider,
        state: &str,
    ) -> Result<String, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => {
                let url = self.client.get_authorize_url(state).await;
                Ok(url)
            }
        }
    }

    pub async fn exchange_code_for_token(
        &self,
        provider: OAuth2Provider,
        code: &str,
        state: &str,
    ) -> Result<String, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => Ok(self.client.exchange_code_for_token(code, state).await?),
        }
    }

    fn encode_session(
        &self,
        session: &OAuth2Session,
    ) -> Result<EncodedOAuth2Session, OAuth2Error> {
        let header = Header::new(Algorithm::EdDSA);
        let encoded = jsonwebtoken::encode(&header, session, &self.encoding_key)
            .map_err(anyhow::Error::from)?;

        Ok(EncodedOAuth2Session { value: encoded })
    }

    fn decode_session(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2Session, OAuth2Error> {
        let validation = Validation::new(Algorithm::EdDSA);
        let session =
            jsonwebtoken::decode::<OAuth2Session>(&encoded_session.value, &self.decoding_key, &validation)
                .map_err(OAuth2Error::InvalidSession)?;

        Ok(session.claims)
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
