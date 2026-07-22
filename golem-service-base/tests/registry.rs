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

use axum::Router;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::routing::post;
use chrono::Utc;
use golem_api_grpc::proto::golem::registry::v1::{
    CreateRuntimeCardResponse, CreateRuntimeCardSuccessResponse, RuntimeCardData,
    create_runtime_card_response,
};
use golem_common::model::card::{Card, CardId, CardManagedByRuntimeDerived, StoredCard};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, IdempotencyKey, OplogIndex};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService, RegistryServiceError,
};
use prost::Message;
use test_r::test;

async fn call_create_runtime_card_with_success_payload(
    malformed: bool,
) -> (StoredCard, Result<StoredCard, RegistryServiceError>) {
    let card = StoredCard::Concrete(Card {
        card_id: CardId(uuid::Uuid::nil()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    });
    let response_card_json = if malformed {
        b"not valid JSON".to_vec()
    } else {
        serde_json::to_vec(&card).unwrap()
    };
    let response = CreateRuntimeCardResponse {
        result: Some(create_runtime_card_response::Result::Success(
            CreateRuntimeCardSuccessResponse {
                card: Some(RuntimeCardData {
                    card_id: Some(uuid::Uuid::nil().into()),
                    card_json: response_card_json,
                }),
            },
        )),
    };
    let mut framed_response = Vec::with_capacity(response.encoded_len() + 5);
    framed_response.push(0);
    framed_response.extend_from_slice(&(response.encoded_len() as u32).to_be_bytes());
    response.encode(&mut framed_response).unwrap();

    let app = Router::new().route(
        "/golem.registry.v1.RegistryService/CreateRuntimeCard",
        post(move || {
            let framed_response = framed_response.clone();
            async move {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/grpc")
                    .header("grpc-status", "0")
                    .body(Body::from(framed_response))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let service = GrpcRegistryService::new(&GrpcRegistryServiceConfig {
        host: "127.0.0.1".to_string(),
        port,
        ..GrpcRegistryServiceConfig::default()
    });
    let provenance = CardManagedByRuntimeDerived {
        environment_id: EnvironmentId::new(),
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "runtime-card-test".to_string(),
        },
        invocation_key: IdempotencyKey::new("runtime-card-test".to_string()),
        oplog_index: OplogIndex::from_u64(1),
    };

    let result = service.create_runtime_card(card.clone(), provenance).await;
    server.abort();
    (card, result)
}

#[test]
async fn create_runtime_card_rejects_malformed_success_payload_without_panicking() {
    let call = tokio::spawn(call_create_runtime_card_with_success_payload(true));
    let (_, result) = call
        .await
        .expect("malformed registry responses must not panic the client");
    assert!(matches!(
        result,
        Err(RegistryServiceError::InternalClientError(_))
    ));
}

#[test]
async fn create_runtime_card_decodes_success_payload() {
    let (expected, result) = call_create_runtime_card_with_success_payload(false).await;
    assert_eq!(result.unwrap(), expected);
}
