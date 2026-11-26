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

//! An in-process registry service that keeps the permission cards of a test.
//!
//! A test that starts an [`EmulatedRegistryService`] and passes it to
//! [`TestExecutorOverrides::with_registry`](crate::TestExecutorOverrides::with_registry) gets
//! the production `CardServiceDefault` over a production `GrpcRegistryService` client that
//! calls this service. The executor's other registry clients do not change.
//!
//! The service answers the three card RPCs of `golem.registry.v1.RegistryService` from an
//! in-memory store, as the real registry does:
//!
//! - `BatchGetCards` gives the stored cards it has and leaves out unknown and revoked cards.
//! - `CreateRuntimeCard` needs every parent to be stored and not expired. It refuses a card that
//!   was revoked, with the `RUNTIME_CARD_REVOKED` code. It gives the stored card when the same
//!   card with the same provenance is stored already, and a conflict when a different one is. It
//!   stores and gives back the card as sent: unlike the real registry, it does not sort the
//!   parent IDs, round timestamps to microseconds, or set `managed_by` on a concrete card.
//! - `RevokeCard` removes the card and its stored descendants and gives the IDs it removed. For
//!   a card that is unknown or revoked already, it answers `NotFound`.
//!
//! It does not check which cards may be revoked (`managed_by`, system cards), and it sends no
//! registry invalidations. Every other RPC answers `Status::unimplemented` with its name.

use futures::stream::BoxStream;
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::registry::v1::registry_service_server::{
    RegistryService, RegistryServiceServer,
};
use golem_api_grpc::proto::golem::registry::v1::*;
use golem_common::base_model::api::error_code;
use golem_common::model::card::{CardId, CardManagedByRuntimeDerived, StoredCard};
use golem_service_base::clients::registry::{GrpcRegistryService, GrpcRegistryServiceConfig};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::task::AbortOnDropHandle;
use tonic::codec::CompressionEncoding;
use tonic::{Request, Response, Status};

#[derive(Default)]
struct Cards {
    /// The stored cards, with the provenance of the runtime-derived ones.
    stored: HashMap<CardId, (StoredCard, Option<CardManagedByRuntimeDerived>)>,
    revoked: HashSet<CardId>,
}

impl Cards {
    fn create_runtime_card(
        &mut self,
        card: StoredCard,
        provenance: CardManagedByRuntimeDerived,
    ) -> Result<StoredCard, registry_service_error::Error> {
        let card_id = card.card_id();
        let now = jiff::Timestamp::now();
        if let Some(parent_id) = card.parent_ids().iter().find(|parent_id| {
            !self.stored.get(parent_id).is_some_and(|(parent, _)| {
                parent
                    .expires_at()
                    .is_none_or(|expires_at| expires_at > now)
            })
        }) {
            return Err(card_not_found(*parent_id));
        }
        if self.revoked.contains(&card_id) {
            return Err(already_exists(
                format!("Runtime card {card_id} has been revoked"),
                error_code::RUNTIME_CARD_REVOKED,
            ));
        }
        match self.stored.get(&card_id) {
            Some((stored, Some(stored_provenance)))
                if *stored == card && *stored_provenance == provenance =>
            {
                Ok(stored.clone())
            }
            Some(_) => Err(already_exists(
                format!("Card {card_id} already exists with different runtime card data"),
                error_code::CONCURRENT_UPDATE,
            )),
            None => {
                self.stored
                    .insert(card_id, (card.clone(), Some(provenance)));
                Ok(card)
            }
        }
    }

    fn revoke_card(
        &mut self,
        card_id: CardId,
    ) -> Result<Vec<CardId>, registry_service_error::Error> {
        if !self.stored.contains_key(&card_id) {
            return Err(card_not_found(card_id));
        }
        let mut removed = vec![card_id];
        let mut index = 0;
        while index < removed.len() {
            let parent = removed[index];
            removed.extend(
                self.stored
                    .iter()
                    .filter(|(id, (card, _))| {
                        card.parent_ids().contains(&parent) && !removed.contains(id)
                    })
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>(),
            );
            index += 1;
        }
        for card_id in &removed {
            self.stored.remove(card_id);
            self.revoked.insert(*card_id);
        }
        Ok(removed)
    }
}

/// An in-process registry service that keeps the permission cards of a test.
/// See the [module documentation](self).
#[derive(Clone)]
pub struct EmulatedRegistryService {
    cards: Arc<Mutex<Cards>>,
    port: u16,
    _server: Arc<AbortOnDropHandle<()>>,
}

impl EmulatedRegistryService {
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let cards = Arc::new(Mutex::new(Cards::default()));
        let service = RegistryServiceServer::new(Handler(cards.clone()))
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip);
        let server = tokio::spawn(async move {
            if let Err(error) = tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
            {
                tracing::error!("emulated registry service stopped: {error}");
            }
        });
        Ok(Self {
            cards,
            port,
            _server: Arc::new(AbortOnDropHandle::new(server)),
        })
    }

    /// Stores `card`, such as the initial card of an agent type. A test adds every card that an
    /// agent checks before the agent starts: the executor caches a card it did not find as
    /// revoked.
    pub fn add_card(&self, card: StoredCard) {
        self.cards
            .lock()
            .unwrap()
            .stored
            .insert(card.card_id(), (card, None));
    }

    /// Whether a revoke removed `card_id`, directly or as the descendant of a revoked card.
    pub fn is_revoked(&self, card_id: CardId) -> bool {
        self.cards.lock().unwrap().revoked.contains(&card_id)
    }

    /// A production registry client that calls this service.
    pub(crate) fn client(&self) -> GrpcRegistryService {
        GrpcRegistryService::new(&GrpcRegistryServiceConfig {
            host: "127.0.0.1".to_string(),
            port: self.port,
            ..GrpcRegistryServiceConfig::default()
        })
    }
}

struct Handler(Arc<Mutex<Cards>>);

fn card_not_found(card_id: CardId) -> registry_service_error::Error {
    registry_service_error::Error::NotFound(ErrorBody {
        error: format!("Card {card_id} not found"),
        code: error_code::CARD_NOT_FOUND.to_string(),
    })
}

fn already_exists(error: String, code: &str) -> registry_service_error::Error {
    registry_service_error::Error::AlreadyExists(ErrorBody {
        error,
        code: code.to_string(),
    })
}

fn bad_request(error: impl Into<String>) -> registry_service_error::Error {
    registry_service_error::Error::BadRequest(golem_api_grpc::proto::golem::common::ErrorsBody {
        errors: vec![error.into()],
        code: error_code::INVALID_RUNTIME_CARD.to_string(),
    })
}

fn registry_error(error: registry_service_error::Error) -> RegistryServiceError {
    RegistryServiceError { error: Some(error) }
}

impl Handler {
    fn batch_get_cards(
        &self,
        request: BatchGetCardsRequest,
    ) -> Result<BatchGetCardsSuccessResponse, registry_service_error::Error> {
        let cards = self.0.lock().unwrap();
        let found = request
            .card_ids
            .into_iter()
            .filter_map(|id| cards.stored.get(&CardId(id.into())))
            .map(|(card, _)| {
                Ok(CardData {
                    card_id: Some(card.card_id().0.into()),
                    card: golem_common::serialization::serialize(card).map_err(bad_request)?,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(BatchGetCardsSuccessResponse { cards: found })
    }

    fn create_runtime_card(
        &self,
        request: CreateRuntimeCardRequest,
    ) -> Result<CreateRuntimeCardSuccessResponse, registry_service_error::Error> {
        let data = request
            .card
            .ok_or_else(|| bad_request("missing card field"))?;
        let card: StoredCard = serde_json::from_slice(&data.card_json)
            .map_err(|error| bad_request(format!("invalid card: {error}")))?;
        let provenance: CardManagedByRuntimeDerived =
            serde_json::from_slice(&request.provenance_json)
                .map_err(|error| bad_request(format!("invalid provenance: {error}")))?;
        if data.card_id.map(uuid::Uuid::from) != Some(card.card_id().0) {
            return Err(bad_request(
                "card.card_id does not match the serialized card",
            ));
        }
        let card = self
            .0
            .lock()
            .unwrap()
            .create_runtime_card(card, provenance)?;
        Ok(CreateRuntimeCardSuccessResponse {
            card: Some(RuntimeCardData {
                card_id: Some(card.card_id().0.into()),
                card_json: serde_json::to_vec(&card)
                    .map_err(|error| bad_request(error.to_string()))?,
            }),
        })
    }

    fn revoke_card(
        &self,
        request: RevokeCardRequest,
    ) -> Result<RevokeCardSuccessResponse, registry_service_error::Error> {
        let card_id = request
            .card_id
            .ok_or_else(|| bad_request("missing card_id field"))?;
        let removed = self.0.lock().unwrap().revoke_card(CardId(card_id.into()))?;
        Ok(RevokeCardSuccessResponse {
            revoked_card_ids: removed.into_iter().map(|id| id.0.into()).collect(),
        })
    }
}

/// Implements `RegistryService` for [`Handler`] with the given methods, and answers every RPC
/// in the `unimplemented` list with `Status::unimplemented`.
macro_rules! registry_service {
    (
        { $($emulated:tt)* }
        unimplemented { $($rpc:ident($request:ident) -> $response:ty;)* }
    ) => {
        #[tonic::async_trait]
        impl RegistryService for Handler {
            type DownloadComponentStream =
                BoxStream<'static, Result<DownloadComponentResponse, Status>>;
            type SubscribeRegistryInvalidationsStream =
                BoxStream<'static, Result<RegistryInvalidationEvent, Status>>;

            $($emulated)*

            $(
                async fn $rpc(
                    &self,
                    _request: Request<$request>,
                ) -> Result<Response<$response>, Status> {
                    Err(Status::unimplemented(concat!(
                        "emulated registry service: ",
                        stringify!($rpc)
                    )))
                }
            )*
        }
    };
}

registry_service! {
    {
        async fn batch_get_cards(
            &self,
            request: Request<BatchGetCardsRequest>,
        ) -> Result<Response<BatchGetCardsResponse>, Status> {
            let result = match self.batch_get_cards(request.into_inner()) {
                Ok(success) => batch_get_cards_response::Result::Success(success),
                Err(error) => batch_get_cards_response::Result::Error(registry_error(error)),
            };
            Ok(Response::new(BatchGetCardsResponse { result: Some(result) }))
        }

        async fn create_runtime_card(
            &self,
            request: Request<CreateRuntimeCardRequest>,
        ) -> Result<Response<CreateRuntimeCardResponse>, Status> {
            let result = match self.create_runtime_card(request.into_inner()) {
                Ok(success) => create_runtime_card_response::Result::Success(success),
                Err(error) => create_runtime_card_response::Result::Error(registry_error(error)),
            };
            Ok(Response::new(CreateRuntimeCardResponse { result: Some(result) }))
        }

        async fn revoke_card(
            &self,
            request: Request<RevokeCardRequest>,
        ) -> Result<Response<RevokeCardResponse>, Status> {
            let result = match self.revoke_card(request.into_inner()) {
                Ok(success) => revoke_card_response::Result::Success(success),
                Err(error) => revoke_card_response::Result::Error(registry_error(error)),
            };
            Ok(Response::new(RevokeCardResponse { result: Some(result) }))
        }
    }
    unimplemented {
        authenticate_token(AuthenticateTokenRequest) -> AuthenticateTokenResponse;
        authorize_environment(AuthorizeEnvironmentRequest) -> AuthorizeEnvironmentResponse;
        get_golem_issuer(GetGolemIssuerRequest) -> GetGolemIssuerResponse;
        get_resource_limits(GetResourceLimitsRequest) -> GetResourceLimitsResponse;
        update_worker_connection_limit(UpdateWorkerConnectionLimitRequest) -> UpdateWorkerConnectionLimitResponse;
        batch_update_resource_usage(BatchUpdateResourceUsageRequest) -> BatchUpdateResourceUsageResponse;
        download_component(DownloadComponentRequest) -> Self::DownloadComponentStream;
        get_component_metadata(GetComponentMetadataRequest) -> GetComponentMetadataResponse;
        get_deployed_component_metadata(GetDeployedComponentMetadataRequest) -> GetDeployedComponentMetadataResponse;
        get_all_deployed_component_revisions(GetAllDeployedComponentRevisionsRequest) -> GetAllDeployedComponentRevisionsResponse;
        resolve_component(ResolveComponentRequest) -> ResolveComponentResponse;
        get_all_agent_types(GetAllAgentTypesRequest) -> GetAllAgentTypesResponse;
        get_agent_type(GetAgentTypeRequest) -> GetAgentTypeResponse;
        get_tool_deployment_state(GetToolDeploymentStateRequest) -> GetToolDeploymentStateResponse;
        get_tool_deployment_state_at_revision(GetToolDeploymentStateAtRevisionRequest) -> GetToolDeploymentStateResponse;
        get_mcp_runtime_credential(GetMcpRuntimeCredentialRequest) -> GetMcpRuntimeCredentialResponse;
        report_mcp_resource_unauthorized(ReportMcpResourceUnauthorizedRequest) -> ReportMcpResourceUnauthorizedResponse;
        resolve_mcp_import(ResolveMcpImportRequest) -> ResolveMcpImportResponse;
        resolve_agent_type_by_names(ResolveAgentTypeByNamesRequest) -> ResolveAgentTypeByNamesResponse;
        resolve_type_id(ResolveTypeIdRequest) -> ResolveTypeIdResponse;
        resolve_tool_type_id(ResolveToolTypeIdRequest) -> ResolveToolTypeIdResponse;
        resolve_agent_type_name(ResolveAgentTypeNameRequest) -> ResolveAgentTypeNameResponse;
        get_deployed_agent_type(GetDeployedAgentTypeRequest) -> GetDeployedAgentTypeResponse;
        get_active_routes_for_domain(GetActiveRoutesForDomainRequest) -> GetActiveRoutesForDomainResponse;
        get_active_mcp_for_domain(GetActiveMcpForDomainRequest) -> GetActiveMcpForDomainResponse;
        get_current_environment_state(GetCurrentEnvironmentStateRequest) -> GetCurrentEnvironmentStateResponse;
        get_agent_secret_revision(GetAgentSecretRevisionRequest) -> GetAgentSecretRevisionResponse;
        get_resource_definition_by_id(GetResourceDefinitionByIdRequest) -> GetResourceDefinitionByIdResponse;
        get_resource_definition_by_name(GetResourceDefinitionByNameRequest) -> GetResourceDefinitionByNameResponse;
        batch_get_existing_cards(BatchGetExistingCardsRequest) -> BatchGetExistingCardsResponse;
        subscribe_registry_invalidations(SubscribeRegistryInvalidationsRequest) -> Self::SubscribeRegistryInvalidationsStream;
    }
}
