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

use crate::repo::plan::{PlanRepo};
use golem_common::model::{PlanId, TokenId};
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, info};
use crate::config::PlansConfig;
use golem_common::model::account::{AccountId, Plan};
use crate::repo::model::plan::PlanRecord;
use std::collections::BTreeMap;
use crate::repo::model::account_usage::UsageType;
use anyhow::anyhow;
use crate::repo::token::TokenRepo;
use golem_common::model::auth::TokenSecret;
use chrono::{DateTime, Utc};
use crate::repo::model::token::TokenRecord;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for TokenError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl From<RepoError> for TokenError {
    fn from(value: RepoError) -> Self {
        Self::InternalError(anyhow::Error::new(value).context("from RepoError"))
    }
}

pub struct TokenService {
    token_repo: Arc<dyn TokenRepo>,
}

impl TokenService {
    pub fn new(
        token_repo: Arc<dyn TokenRepo>,
    ) -> Self {
        Self  { token_repo }
    }

    pub async fn create_known_secret(
        &self,
        account_id: AccountId,
        secret: TokenSecret,
        expires_at: &DateTime<Utc>
    ) -> anyhow::Result<()> {
        let created_at = Utc::now();
        let token_id = TokenId::new_v4();

        let record = TokenRecord {
            token_id: token_id.0,
            secret: secret.value,
            account_id: account_id.0,
            created_at: created_at.into(),
            expires_at: expires_at.into()
        };


        let response = self.token_repo.create(token)
    }

}
