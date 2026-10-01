// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path as FilePath, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOwner {
    pub component_id: Uuid,
    pub agent_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: Uuid,
    pub owner: ApprovalOwner,
    pub promise_oplog_idx: u64,
    pub policy: String,
    pub tool_name: String,
    pub command_path: Vec<String>,
    pub principal: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthenticatedPrincipalKind {
    Anonymous,
    Oidc,
    Agent,
    GolemUser,
}

impl TryFrom<&str> for AuthenticatedPrincipalKind {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "anonymous" => Ok(Self::Anonymous),
            "oidc" => Ok(Self::Oidc),
            "agent" => Ok(Self::Agent),
            "golem-user" => Ok(Self::GolemUser),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalState {
    Pending,
    Approved,
    Denied,
    Cancelled,
    Abandoned,
}

impl ApprovalState {
    fn is_decision(self) -> bool {
        matches!(self, Self::Approved | Self::Denied | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecision {
    pub owner: ApprovalOwner,
    pub state: ApprovalState,
    pub decided_by: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallbackState {
    NotRequired,
    Pending,
    Delivering,
    Delivered,
    OwnerGone,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRecord {
    pub request: ApprovalRequest,
    pub state: ApprovalState,
    pub decided_by: Option<String>,
    pub callback: CallbackState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Conflict(String),
    NotFound,
    InvalidTransition(String),
    Io(String),
}

impl From<io::Error> for StoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Snapshot {
    records: BTreeMap<Uuid, ApprovalRecord>,
}

pub struct ApprovalStore {
    path: PathBuf,
    snapshot: Snapshot,
}

impl ApprovalStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let snapshot = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| StoreError::Io(format!("invalid approval store: {error}")))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Snapshot::default(),
            Err(error) => return Err(error.into()),
        };
        let mut store = Self { path, snapshot };
        store.recover_callback_claims()?;
        Ok(store)
    }

    pub fn register(&mut self, request: ApprovalRequest) -> Result<ApprovalRecord, StoreError> {
        if let Some(record) = self.snapshot.records.get(&request.request_id) {
            return if record.request == request {
                Ok(record.clone())
            } else {
                Err(StoreError::Conflict(
                    "request id is already bound to different immutable request data".to_string(),
                ))
            };
        }
        let record = ApprovalRecord {
            request,
            state: ApprovalState::Pending,
            decided_by: None,
            callback: CallbackState::NotRequired,
        };
        self.snapshot
            .records
            .insert(record.request.request_id, record.clone());
        if let Err(error) = self.persist() {
            self.snapshot.records.remove(&record.request.request_id);
            return Err(error);
        }
        Ok(record)
    }

    pub fn get(&self, request_id: Uuid) -> Option<ApprovalRecord> {
        self.snapshot.records.get(&request_id).cloned()
    }

    pub fn list(&self) -> Vec<ApprovalRecord> {
        self.snapshot.records.values().cloned().collect()
    }

    pub fn decide(
        &mut self,
        request_id: Uuid,
        decision: ApprovalDecision,
    ) -> Result<ApprovalRecord, StoreError> {
        if !decision.state.is_decision() {
            return Err(StoreError::InvalidTransition(
                "operator decisions must be approved, denied, or cancelled".to_string(),
            ));
        }
        let record = self
            .snapshot
            .records
            .get_mut(&request_id)
            .ok_or(StoreError::NotFound)?;
        if record.request.owner != decision.owner {
            return Err(StoreError::Conflict(
                "decision owner does not match the request owner".to_string(),
            ));
        }
        if record.state == decision.state {
            return Ok(record.clone());
        }
        if record.state != ApprovalState::Pending {
            return Err(StoreError::Conflict(format!(
                "request is already terminal: {:?}",
                record.state
            )));
        }
        let previous = record.clone();
        record.state = decision.state;
        record.decided_by = Some(decision.decided_by);
        record.callback = CallbackState::Pending;
        let result = record.clone();
        if let Err(error) = self.persist() {
            self.snapshot.records.insert(request_id, previous);
            return Err(error);
        }
        Ok(result)
    }

    pub fn abandon(
        &mut self,
        request_id: Uuid,
        owner: &ApprovalOwner,
    ) -> Result<ApprovalRecord, StoreError> {
        let record = self
            .snapshot
            .records
            .get_mut(&request_id)
            .ok_or(StoreError::NotFound)?;
        if &record.request.owner != owner {
            return Err(StoreError::Conflict(
                "abandon owner does not match the request owner".to_string(),
            ));
        }
        if record.state == ApprovalState::Abandoned {
            return Ok(record.clone());
        }
        if record.state != ApprovalState::Pending {
            return Err(StoreError::Conflict(format!(
                "request is already terminal: {:?}",
                record.state
            )));
        }
        let previous = record.clone();
        record.state = ApprovalState::Abandoned;
        record.callback = CallbackState::NotRequired;
        let result = record.clone();
        if let Err(error) = self.persist() {
            self.snapshot.records.insert(request_id, previous);
            return Err(error);
        }
        Ok(result)
    }

    fn set_callback_state(
        &mut self,
        request_id: Uuid,
        callback: CallbackState,
    ) -> Result<ApprovalRecord, StoreError> {
        let record = self
            .snapshot
            .records
            .get_mut(&request_id)
            .ok_or(StoreError::NotFound)?;
        if !record.state.is_decision() {
            return Err(StoreError::InvalidTransition(
                "only a decided request has a callback".to_string(),
            ));
        }
        let previous = record.clone();
        record.callback = callback;
        let result = record.clone();
        if let Err(error) = self.persist() {
            self.snapshot.records.insert(request_id, previous);
            return Err(error);
        }
        Ok(result)
    }

    fn recover_callback_claim(&mut self, request_id: Uuid) -> Result<(), StoreError> {
        if self
            .snapshot
            .records
            .get(&request_id)
            .is_some_and(|record| record.callback == CallbackState::Delivering)
        {
            self.set_callback_state(request_id, CallbackState::Pending)?;
        }
        Ok(())
    }

    fn recover_callback_claims(&mut self) -> Result<(), StoreError> {
        let claimed = self
            .snapshot
            .records
            .iter()
            .filter_map(|(request_id, record)| {
                (record.callback == CallbackState::Delivering).then_some(*request_id)
            })
            .collect::<Vec<_>>();
        for request_id in claimed {
            self.recover_callback_claim(request_id)?;
        }
        Ok(())
    }

    fn claim_callback(&mut self, request_id: Uuid) -> Result<ApprovalRecord, StoreError> {
        let record = self
            .snapshot
            .records
            .get(&request_id)
            .ok_or(StoreError::NotFound)?;
        if record.callback != CallbackState::Pending {
            return Err(StoreError::InvalidTransition(
                "only a pending callback can be claimed".to_string(),
            ));
        }
        self.set_callback_state(request_id, CallbackState::Delivering)
    }

    fn finish_callback(
        &mut self,
        request_id: Uuid,
        callback: CallbackState,
    ) -> Result<ApprovalRecord, StoreError> {
        if !matches!(
            callback,
            CallbackState::Delivered | CallbackState::OwnerGone
        ) {
            return Err(StoreError::InvalidTransition(
                "callback delivery must finish as delivered or owner-gone".to_string(),
            ));
        }
        let record = self
            .snapshot
            .records
            .get(&request_id)
            .ok_or(StoreError::NotFound)?;
        if record.callback != CallbackState::Delivering {
            return Err(StoreError::InvalidTransition(
                "only a claimed callback can finish delivery".to_string(),
            ));
        }
        self.set_callback_state(request_id, callback)
    }

    fn persist(&self) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(&self.snapshot)
            .map_err(|error| StoreError::Io(error.to_string()))?;
        let mut file = File::create(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, &self.path)?;
        if let Some(parent) = self.path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ApprovalServiceConfig {
    pub request_token: String,
    pub decision_token: String,
    pub golem_api_url: String,
    pub golem_api_token: String,
}

#[derive(Clone)]
struct ApprovalServiceState {
    store: Arc<Mutex<ApprovalStore>>,
    callback_deliveries: Arc<Mutex<BTreeMap<Uuid, Arc<AsyncMutex<()>>>>>,
    config: ApprovalServiceConfig,
    client: reqwest::Client,
}

impl ApprovalServiceState {
    fn callback_delivery(&self, request_id: Uuid) -> Arc<AsyncMutex<()>> {
        self.callback_deliveries
            .lock()
            .unwrap()
            .entry(request_id)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

pub fn router(store: ApprovalStore, config: ApprovalServiceConfig) -> Router {
    let state = ApprovalServiceState {
        store: Arc::new(Mutex::new(store)),
        callback_deliveries: Arc::new(Mutex::new(BTreeMap::new())),
        config,
        client: reqwest::Client::new(),
    };
    Router::new()
        .route("/v1/requests", post(register_request).get(list_requests))
        .route("/v1/requests/{request_id}", get(get_request))
        .route("/v1/requests/{request_id}/decision", post(decide_request))
        .route("/v1/requests/{request_id}/abandon", post(abandon_request))
        .with_state(state)
}

fn authenticated(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {expected}"))
}

async fn register_request(
    State(state): State<ApprovalServiceState>,
    headers: HeaderMap,
    Json(request): Json<ApprovalRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !authenticated(&headers, &state.config.request_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid request token".to_string(),
        ));
    }
    AuthenticatedPrincipalKind::try_from(request.principal.as_str()).map_err(|_| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "principal must be one of anonymous, oidc, agent, or golem-user".to_string(),
        )
    })?;
    let mut store = state.store.lock().unwrap();
    let existed = store.get(request.request_id).is_some();
    store.register(request).map_err(store_error_response)?;
    Ok(if existed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    })
}

async fn get_request(
    State(state): State<ApprovalServiceState>,
    headers: HeaderMap,
    Path(request_id): Path<Uuid>,
) -> Result<Json<ApprovalRecord>, (StatusCode, String)> {
    if !authenticated(&headers, &state.config.decision_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid decision token".to_string(),
        ));
    }
    state
        .store
        .lock()
        .unwrap()
        .get(request_id)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "request not found".to_string()))
}

async fn list_requests(
    State(state): State<ApprovalServiceState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ApprovalRecord>>, (StatusCode, String)> {
    if !authenticated(&headers, &state.config.decision_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid decision token".to_string(),
        ));
    }
    Ok(Json(state.store.lock().unwrap().list()))
}

async fn decide_request(
    State(state): State<ApprovalServiceState>,
    headers: HeaderMap,
    Path(request_id): Path<Uuid>,
    Json(decision): Json<ApprovalDecision>,
) -> Result<Json<ApprovalRecord>, (StatusCode, String)> {
    if !authenticated(&headers, &state.config.decision_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid decision token".to_string(),
        ));
    }
    let delivery = state.callback_delivery(request_id);
    let _delivery_guard = delivery.lock().await;
    let record = {
        let mut store = state.store.lock().unwrap();
        store
            .recover_callback_claim(request_id)
            .map_err(store_error_response)?;
        store
            .decide(request_id, decision)
            .map_err(store_error_response)?
    };
    if record.callback == CallbackState::Pending {
        let claimed = state
            .store
            .lock()
            .unwrap()
            .claim_callback(request_id)
            .map_err(store_error_response)?;
        match deliver_callback(&state, &claimed).await {
            Ok(callback) => {
                return state
                    .store
                    .lock()
                    .unwrap()
                    .finish_callback(request_id, callback)
                    .map(Json)
                    .map_err(store_error_response);
            }
            Err(delivery_error) => {
                state
                    .store
                    .lock()
                    .unwrap()
                    .set_callback_state(request_id, CallbackState::Pending)
                    .map_err(store_error_response)?;
                return Err(delivery_error);
            }
        }
    }
    Ok(Json(record))
}

async fn abandon_request(
    State(state): State<ApprovalServiceState>,
    headers: HeaderMap,
    Path(request_id): Path<Uuid>,
    Json(owner): Json<ApprovalOwner>,
) -> Result<Json<ApprovalRecord>, (StatusCode, String)> {
    if !authenticated(&headers, &state.config.decision_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid decision token".to_string(),
        ));
    }
    state
        .store
        .lock()
        .unwrap()
        .abandon(request_id, &owner)
        .map(Json)
        .map_err(store_error_response)
}

async fn deliver_callback(
    state: &ApprovalServiceState,
    record: &ApprovalRecord,
) -> Result<CallbackState, (StatusCode, String)> {
    let mut url = reqwest::Url::parse(&state.config.golem_api_url)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    url.path_segments_mut()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Golem API URL cannot be a base URL".to_string(),
            )
        })?
        .extend([
            "v1",
            "components",
            &record.request.owner.component_id.to_string(),
            "workers",
            &record.request.owner.agent_name,
            "complete",
        ]);
    let callback = serde_json::to_vec(&ApprovalCallback {
        request_id: record.request.request_id,
        state: record.state,
    })
    .unwrap();
    let response = state
        .client
        .post(url)
        .bearer_auth(&state.config.golem_api_token)
        .json(&serde_json::json!({
            "oplogIdx": record.request.promise_oplog_idx,
            "data": callback,
        }))
        .send()
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?;
    if response.status().is_success() {
        Ok(CallbackState::Delivered)
    } else if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) {
        Ok(CallbackState::OwnerGone)
    } else {
        Err((
            StatusCode::BAD_GATEWAY,
            format!("Golem promise completion failed: {}", response.status()),
        ))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCallback {
    pub request_id: Uuid,
    pub state: ApprovalState,
}

fn store_error_response(error: StoreError) -> (StatusCode, String) {
    match error {
        StoreError::Conflict(message) | StoreError::InvalidTransition(message) => {
            (StatusCode::CONFLICT, message)
        }
        StoreError::NotFound => (StatusCode::NOT_FOUND, "request not found".to_string()),
        StoreError::Io(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

pub fn default_store_path() -> &'static FilePath {
    FilePath::new("human-approvals.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            request_id: Uuid::from_u128(1),
            owner: ApprovalOwner {
                component_id: Uuid::from_u128(2),
                agent_name: "agent/owner".to_string(),
            },
            promise_oplog_idx: 42,
            policy: "production-change".to_string(),
            tool_name: "deploy".to_string(),
            command_path: vec!["apply".to_string()],
            principal: "golem-user".to_string(),
        }
    }

    #[test]
    fn transition_and_idempotency_rules_survive_store_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("approvals.json");
        let mut store = ApprovalStore::open(&path).unwrap();
        let request = request();
        assert_eq!(
            store.register(request.clone()).unwrap().state,
            ApprovalState::Pending
        );
        assert_eq!(
            store.register(request.clone()).unwrap().state,
            ApprovalState::Pending
        );
        let mut conflicting = request.clone();
        conflicting.tool_name = "other".to_string();
        assert!(matches!(
            store.register(conflicting),
            Err(StoreError::Conflict(_))
        ));
        drop(store);

        let mut store = ApprovalStore::open(&path).unwrap();
        let decision = ApprovalDecision {
            owner: request.owner.clone(),
            state: ApprovalState::Approved,
            decided_by: "operator-1".to_string(),
        };
        let approved = store.decide(request.request_id, decision.clone()).unwrap();
        assert_eq!(approved.callback, CallbackState::Pending);
        assert_eq!(
            store.decide(request.request_id, decision).unwrap(),
            approved
        );
        assert!(matches!(
            store.decide(
                request.request_id,
                ApprovalDecision {
                    owner: request.owner,
                    state: ApprovalState::Denied,
                    decided_by: "operator-2".to_string(),
                }
            ),
            Err(StoreError::Conflict(_))
        ));
    }

    #[test]
    fn wrong_owner_and_late_decisions_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ApprovalStore::open(temp.path().join("approvals.json")).unwrap();
        let request = request();
        store.register(request.clone()).unwrap();
        let wrong_owner = ApprovalOwner {
            component_id: Uuid::from_u128(3),
            agent_name: request.owner.agent_name.clone(),
        };
        assert!(matches!(
            store.decide(
                request.request_id,
                ApprovalDecision {
                    owner: wrong_owner,
                    state: ApprovalState::Approved,
                    decided_by: "operator".to_string(),
                }
            ),
            Err(StoreError::Conflict(_))
        ));
        store.abandon(request.request_id, &request.owner).unwrap();
        assert!(matches!(
            store.decide(
                request.request_id,
                ApprovalDecision {
                    owner: request.owner,
                    state: ApprovalState::Approved,
                    decided_by: "operator".to_string(),
                }
            ),
            Err(StoreError::Conflict(_))
        ));
    }

    #[test]
    fn callback_claim_is_durable_and_recovered_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("approvals.json");
        let request = request();
        let mut store = ApprovalStore::open(&path).unwrap();
        store.register(request.clone()).unwrap();
        store
            .decide(
                request.request_id,
                ApprovalDecision {
                    owner: request.owner,
                    state: ApprovalState::Approved,
                    decided_by: "operator".to_string(),
                },
            )
            .unwrap();
        assert_eq!(
            store.claim_callback(request.request_id).unwrap().callback,
            CallbackState::Delivering
        );
        drop(store);

        let mut recovered = ApprovalStore::open(&path).unwrap();
        assert_eq!(
            recovered.get(request.request_id).unwrap().callback,
            CallbackState::Pending
        );
        assert_eq!(
            recovered
                .claim_callback(request.request_id)
                .unwrap()
                .callback,
            CallbackState::Delivering
        );
    }
}
