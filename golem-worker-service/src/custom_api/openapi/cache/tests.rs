// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::*;
use crate::custom_api::openapi::OpenApiKey;
use crate::custom_api::openapi::test_support::{controlled, document, inputs};

fn generate(
    service: &OpenApiService,
    inputs: Arc<OpenApiInputs>,
) -> tokio::task::JoinHandle<Result> {
    let service = service.clone();
    tokio::spawn(async move { service.generate(inputs).await })
}

#[test_r::test]
async fn same_snapshot_coalesces_and_survives_caller_cancellation() {
    let (service, mut calls) = controlled();
    let snapshot = inputs(1).await;
    let cancelled = generate(&service, snapshot.clone());
    let (_, reply) = calls.recv().await.unwrap();
    let waiter = generate(&service, snapshot.clone());
    cancelled.abort();
    reply.send(Ok(document())).unwrap();

    let generated = waiter.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(
        &generated,
        &service.generate(snapshot).await.unwrap()
    ));
    assert!(calls.try_recv().is_err());
}

#[test_r::test]
async fn equivalent_route_snapshot_reuses_generation() {
    let (service, mut calls) = controlled();
    let first = inputs(1).await;
    let second = inputs(1).await;
    assert_eq!(first.key, second.key);

    let first_generation = generate(&service, first.clone());
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    let first_document = first_generation.await.unwrap().unwrap();

    let second_document = service.generate(second).await.unwrap();
    assert!(Arc::ptr_eq(&first_document, &second_document));
    assert!(calls.try_recv().is_err());
}

#[test_r::test]
async fn changed_origin_starts_a_new_generation() {
    let (service, mut calls) = controlled();
    let first = inputs(1).await;
    let public_origin = "https://other.example.com".to_string();
    let second = Arc::new(OpenApiInputs {
        key: OpenApiKey::from_inputs(&public_origin, &first.routes),
        public_origin,
        routes: first.routes.clone(),
    });
    assert_ne!(first.key, second.key);

    for snapshot in [first, second] {
        let generation = generate(&service, snapshot);
        calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
        generation.await.unwrap().unwrap();
    }
    assert!(calls.try_recv().is_err());
}

#[test_r::test]
async fn failed_generation_is_retried_for_the_same_snapshot() {
    let (service, mut calls) = controlled();
    let snapshot = inputs(1).await;
    let failed = generate(&service, snapshot.clone());
    calls
        .recv()
        .await
        .unwrap()
        .1
        .send(Err(OpenApiError::new("provider-invocation")))
        .unwrap();
    assert_eq!(
        failed.await.unwrap().unwrap_err().category(),
        "provider-invocation"
    );

    let retry = generate(&service, snapshot);
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    retry.await.unwrap().unwrap();
}
