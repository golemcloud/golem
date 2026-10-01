// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use golem_human_approval::{
    ApprovalDecision, ApprovalOwner, ApprovalRecord, ApprovalRequest, ApprovalServiceConfig,
    ApprovalState, ApprovalStore, CallbackState,
};
use reqwest::Client;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;

const REQUEST_TOKEN: &str = "k4-request-token";
const DECISION_TOKEN: &str = "k4-decision-token";
const GOLEM_TOKEN: &str = "k4-golem-token";

#[derive(Default)]
struct CallbackProbe {
    calls: AtomicUsize,
    hold: AtomicBool,
    release: Notify,
}

async fn complete_promise(
    State(probe): State<Arc<CallbackProbe>>,
    headers: HeaderMap,
    Json(_completion): Json<serde_json::Value>,
) -> StatusCode {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer k4-golem-token")
    {
        return StatusCode::UNAUTHORIZED;
    }
    probe.calls.fetch_add(1, Ordering::SeqCst);
    if probe.hold.load(Ordering::SeqCst) {
        probe.release.notified().await;
    }
    StatusCode::OK
}

struct RunningServer {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_callback_server(probe: Arc<CallbackProbe>) -> RunningServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .fallback(post(complete_promise))
                .with_state(probe),
        )
        .await
        .unwrap();
    });
    RunningServer {
        base_url: format!("http://{address}"),
        task,
    }
}

async fn start_approval_server(store_path: &Path, callback_url: &str) -> RunningServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let store = ApprovalStore::open(store_path).unwrap();
    let app = golem_human_approval::router(
        store,
        ApprovalServiceConfig {
            request_token: REQUEST_TOKEN.to_string(),
            decision_token: DECISION_TOKEN.to_string(),
            golem_api_url: callback_url.to_string(),
            golem_api_token: GOLEM_TOKEN.to_string(),
        },
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    RunningServer {
        base_url: format!("http://{address}"),
        task,
    }
}

struct Harness {
    _directory: tempfile::TempDir,
    store_path: PathBuf,
    callback: Arc<CallbackProbe>,
    _callback_server: RunningServer,
    approval_server: Option<RunningServer>,
    client: Client,
}

impl Harness {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store_path = directory.path().join("k4-approvals.json");
        let callback = Arc::new(CallbackProbe::default());
        let callback_server = start_callback_server(callback.clone()).await;
        let approval_server = start_approval_server(&store_path, &callback_server.base_url).await;
        Self {
            _directory: directory,
            store_path,
            callback,
            _callback_server: callback_server,
            approval_server: Some(approval_server),
            client: Client::new(),
        }
    }

    fn base_url(&self) -> &str {
        &self.approval_server.as_ref().unwrap().base_url
    }

    async fn restart(&mut self) {
        let previous = self.approval_server.take().unwrap();
        let callback_url = self._callback_server.base_url.clone();
        previous.task.abort();
        drop(previous);
        self.approval_server = Some(start_approval_server(&self.store_path, &callback_url).await);
    }

    async fn register(&self, request: &ApprovalRequest, token: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/requests", self.base_url()))
            .bearer_auth(token)
            .json(request)
            .send()
            .await
            .unwrap()
    }

    async fn get(&self, request_id: Uuid, token: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/v1/requests/{request_id}", self.base_url()))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    async fn decide(
        &self,
        request: &ApprovalRequest,
        state: ApprovalState,
        decided_by: &str,
        token: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/v1/requests/{}/decision",
                self.base_url(),
                request.request_id
            ))
            .bearer_auth(token)
            .json(&ApprovalDecision {
                owner: request.owner.clone(),
                state,
                decided_by: decided_by.to_string(),
            })
            .send()
            .await
            .unwrap()
    }

    async fn abandon(&self, request: &ApprovalRequest, token: &str) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/v1/requests/{}/abandon",
                self.base_url(),
                request.request_id
            ))
            .bearer_auth(token)
            .json(&request.owner)
            .send()
            .await
            .unwrap()
    }
}

fn request(request_id: u128, promise_oplog_idx: u64) -> ApprovalRequest {
    ApprovalRequest {
        request_id: Uuid::from_u128(request_id),
        owner: ApprovalOwner {
            component_id: Uuid::from_u128(0x4b34),
            agent_name: format!("k4-owner-{request_id}"),
        },
        promise_oplog_idx,
        policy: "production-change".to_string(),
        tool_name: "matrix-core".to_string(),
        command_path: vec!["artifact".to_string(), "inspect".to_string()],
        principal: "oidc".to_string(),
    }
}

async fn record(response: reqwest::Response) -> ApprovalRecord {
    response.error_for_status().unwrap().json().await.unwrap()
}

#[tokio::test]
async fn approval_1_request_identity_auth_state_and_late_decision_contract() {
    let harness = Harness::start().await;
    let request = request(1, 41);

    assert_eq!(
        harness.register(&request, "wrong-token").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness.register(&request, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        harness.register(&request, REQUEST_TOKEN).await.status(),
        StatusCode::OK
    );
    let mut identity_collision = request.clone();
    identity_collision.promise_oplog_idx += 1;
    assert_eq!(
        harness
            .register(&identity_collision, REQUEST_TOKEN)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        harness
            .get(request.request_id, REQUEST_TOKEN)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let approved = record(
        harness
            .decide(
                &request,
                ApprovalState::Approved,
                "operator-first",
                DECISION_TOKEN,
            )
            .await,
    )
    .await;
    assert_eq!(approved.state, ApprovalState::Approved);
    assert_eq!(approved.decided_by.as_deref(), Some("operator-first"));
    assert_eq!(approved.callback, CallbackState::Delivered);
    assert_eq!(harness.callback.calls.load(Ordering::SeqCst), 1);

    let duplicate = record(
        harness
            .decide(
                &request,
                ApprovalState::Approved,
                "operator-later",
                DECISION_TOKEN,
            )
            .await,
    )
    .await;
    assert_eq!(duplicate.decided_by.as_deref(), Some("operator-first"));
    assert_eq!(harness.callback.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness
            .decide(
                &request,
                ApprovalState::Denied,
                "operator-later",
                DECISION_TOKEN,
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn approval_1_rejects_unrecognized_principal_kinds() {
    let harness = Harness::start().await;
    let mut request = request(2, 42);
    request.principal = "oidc:forged-subject".to_string();

    assert_eq!(
        harness.register(&request, REQUEST_TOKEN).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "principal must be one of anonymous, oidc, agent, or golem-user"
    );
}

#[tokio::test]
async fn approval_2_pending_request_survives_service_restart() {
    let mut harness = Harness::start().await;
    let request = request(3, 43);
    assert_eq!(
        harness.register(&request, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );

    harness.restart().await;

    let pending = record(harness.get(request.request_id, DECISION_TOKEN).await).await;
    assert_eq!(pending.request, request);
    assert_eq!(pending.state, ApprovalState::Pending);
    assert_eq!(pending.callback, CallbackState::NotRequired);
    let approved = record(
        harness
            .decide(
                &pending.request,
                ApprovalState::Approved,
                "operator-after-restart",
                DECISION_TOKEN,
            )
            .await,
    )
    .await;
    assert_eq!(approved.state, ApprovalState::Approved);
    assert_eq!(approved.callback, CallbackState::Delivered);
    assert_eq!(harness.callback.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn approval_3_concurrent_duplicate_decisions_deliver_one_callback() {
    let harness = Harness::start().await;
    let request = request(4, 44);
    assert_eq!(
        harness.register(&request, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    harness.callback.hold.store(true, Ordering::SeqCst);

    let first = harness.decide(
        &request,
        ApprovalState::Approved,
        "operator-first",
        DECISION_TOKEN,
    );
    let second = harness.decide(
        &request,
        ApprovalState::Approved,
        "operator-duplicate",
        DECISION_TOKEN,
    );
    let release = async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        harness.callback.release.notify_waiters();
    };
    let ((first, second), ()) = tokio::join!(async { tokio::join!(first, second) }, release);
    assert!(first.status().is_success());
    assert!(second.status().is_success());
    let first: ApprovalRecord = first.json().await.unwrap();
    let second: ApprovalRecord = second.json().await.unwrap();
    assert_eq!(first.decided_by, second.decided_by);
    assert!(matches!(
        first.decided_by.as_deref(),
        Some("operator-first" | "operator-duplicate")
    ));
    assert_eq!(
        harness.callback.calls.load(Ordering::SeqCst),
        1,
        "an idempotent duplicate decision must join or observe the first callback, not redeliver it"
    );
    let stored = record(harness.get(request.request_id, DECISION_TOKEN).await).await;
    assert_eq!(stored.state, ApprovalState::Approved);
    assert_eq!(stored.decided_by, first.decided_by);
    assert_eq!(stored.callback, CallbackState::Delivered);
}

#[tokio::test]
async fn approval_3_decisions_require_auth_and_exact_owner() {
    let harness = Harness::start().await;
    let request = request(5, 45);
    assert_eq!(
        harness.register(&request, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        harness
            .decide(&request, ApprovalState::Denied, "operator", "wrong-token",)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let response = harness
        .client
        .post(format!(
            "{}/v1/requests/{}/decision",
            harness.base_url(),
            request.request_id
        ))
        .bearer_auth(DECISION_TOKEN)
        .json(&json!({
            "owner": {
                "componentId": Uuid::from_u128(0xBAD),
                "agentName": request.owner.agent_name,
            },
            "state": "denied",
            "decidedBy": "operator",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let pending = record(harness.get(request.request_id, DECISION_TOKEN).await).await;
    assert_eq!(pending.state, ApprovalState::Pending);
    assert_eq!(harness.callback.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn approval_4_detach_cancel_and_termination_have_distinct_terminals() {
    let harness = Harness::start().await;

    let detached = request(6, 46);
    assert_eq!(
        harness.register(&detached, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    let still_pending = record(harness.get(detached.request_id, DECISION_TOKEN).await).await;
    assert_eq!(still_pending.state, ApprovalState::Pending);
    assert_eq!(still_pending.callback, CallbackState::NotRequired);

    let cancelled = request(7, 47);
    assert_eq!(
        harness.register(&cancelled, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    let cancelled_record = record(
        harness
            .decide(
                &cancelled,
                ApprovalState::Cancelled,
                "operator-cancel",
                DECISION_TOKEN,
            )
            .await,
    )
    .await;
    assert_eq!(cancelled_record.state, ApprovalState::Cancelled);
    assert_eq!(cancelled_record.callback, CallbackState::Delivered);
    assert_eq!(
        harness
            .decide(
                &cancelled,
                ApprovalState::Approved,
                "operator-late",
                DECISION_TOKEN,
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );

    let denied = request(9, 49);
    assert_eq!(
        harness.register(&denied, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    let denied_record = record(
        harness
            .decide(
                &denied,
                ApprovalState::Denied,
                "operator-deny",
                DECISION_TOKEN,
            )
            .await,
    )
    .await;
    assert_eq!(denied_record.state, ApprovalState::Denied);
    assert_eq!(denied_record.callback, CallbackState::Delivered);
    assert_eq!(
        harness
            .decide(
                &denied,
                ApprovalState::Approved,
                "operator-late",
                DECISION_TOKEN,
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );

    let terminated = request(8, 48);
    assert_eq!(
        harness.register(&terminated, REQUEST_TOKEN).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        harness.abandon(&terminated, "wrong-token").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let abandoned = record(harness.abandon(&terminated, DECISION_TOKEN).await).await;
    assert_eq!(abandoned.state, ApprovalState::Abandoned);
    assert_eq!(abandoned.callback, CallbackState::NotRequired);
    assert_eq!(
        harness
            .decide(
                &terminated,
                ApprovalState::Approved,
                "operator-too-late",
                DECISION_TOKEN,
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(harness.callback.calls.load(Ordering::SeqCst), 2);
}
