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

use super::ApiResult;
use crate::services::auth::AuthService;
use crate::services::card::{AccountCardFilter, CardService};
use golem_common::model::account::AccountId;
use golem_common::model::card::{CardId, StoredCard};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::Object;
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct CardsApi {
    card_service: Arc<CardService>,
    auth_service: Arc<AuthService>,
}

#[derive(Clone, Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct RevokedCardsResponse {
    pub revoked_card_ids: Vec<CardId>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Card
)]
impl CardsApi {
    pub fn new(card_service: Arc<CardService>, auth_service: Arc<AuthService>) -> Self {
        Self {
            card_service,
            auth_service,
        }
    }

    /// List cards owned by an account.
    #[oai(
        path = "/accounts/:account_id/cards",
        method = "get",
        operation_id = "list_account_cards",
        tag = ApiTags::Account,
    )]
    async fn list_account_cards(
        &self,
        account_id: Path<AccountId>,
        include_root: Query<Option<bool>>,
        include_permission_shares: Query<Option<bool>>,
        include_environment_defaults: Query<Option<bool>>,
        include_agent_initials: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<StoredCard>>> {
        let record =
            recorded_http_api_request!("list_account_cards", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let filter = AccountCardFilter {
            root: include_root.0.unwrap_or(true),
            permission_share: include_permission_shares.0.unwrap_or(true),
            environment_default: include_environment_defaults.0.unwrap_or(true),
            agent_initial: include_agent_initials.0.unwrap_or(true),
        };

        let response = self
            .list_account_cards_internal(account_id.0, filter, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_account_cards_internal(
        &self,
        account_id: AccountId,
        filter: AccountCardFilter,
        auth: AuthCtx,
    ) -> ApiResult<Json<Vec<StoredCard>>> {
        Ok(Json(
            self.card_service
                .list_account_cards(account_id, filter, &auth)
                .await?,
        ))
    }

    /// Get a card by id.
    #[oai(
        path = "/cards/:card_id",
        method = "get",
        operation_id = "get_card",
        tag = ApiTags::Card,
    )]
    async fn get_card(
        &self,
        card_id: Path<CardId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<StoredCard>> {
        let record = recorded_http_api_request!("get_card", card_id = card_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_card_internal(card_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_card_internal(
        &self,
        card_id: CardId,
        auth: AuthCtx,
    ) -> ApiResult<Json<StoredCard>> {
        Ok(Json(self.card_service.get_card(card_id, &auth).await?))
    }

    /// Revoke a card and all of its descendants.
    #[oai(
        path = "/cards/:card_id",
        method = "delete",
        operation_id = "revoke_card",
        tag = ApiTags::Card,
    )]
    async fn revoke_card(
        &self,
        card_id: Path<CardId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<RevokedCardsResponse>> {
        let record = recorded_http_api_request!("revoke_card", card_id = card_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .revoke_card_internal(card_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn revoke_card_internal(
        &self,
        card_id: CardId,
        auth: AuthCtx,
    ) -> ApiResult<Json<RevokedCardsResponse>> {
        Ok(Json(RevokedCardsResponse {
            revoked_card_ids: self.card_service.revoke_card(card_id, &auth).await?,
        }))
    }
}
