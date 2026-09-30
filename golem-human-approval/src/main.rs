// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use golem_human_approval::{ApprovalServiceConfig, ApprovalStore, default_store_path, router};

#[tokio::main]
async fn main() {
    let listen =
        std::env::var("GOLEM_APPROVAL_LISTEN").unwrap_or_else(|_| "127.0.0.1:9090".to_string());
    let store_path = std::env::var_os("GOLEM_APPROVAL_STORE")
        .map(Into::into)
        .unwrap_or_else(|| default_store_path().to_path_buf());
    let config = ApprovalServiceConfig {
        request_token: required("GOLEM_APPROVAL_REQUEST_TOKEN"),
        decision_token: required("GOLEM_APPROVAL_DECISION_TOKEN"),
        golem_api_url: required("GOLEM_API_URL"),
        golem_api_token: required("GOLEM_API_TOKEN"),
    };
    let store = ApprovalStore::open(store_path).expect("open durable approval store");
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .expect("bind approval service");
    axum::serve(listener, router(store, config))
        .await
        .expect("serve approval service");
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}
