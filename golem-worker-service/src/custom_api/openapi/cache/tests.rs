// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::*;
use crate::custom_api::openapi::OpenApiKey;
use crate::custom_api::openapi::test_support::{controlled, corpus_case, document, inputs};

fn generate(
    service: &OpenApiService,
    inputs: Arc<OpenApiInputs>,
) -> tokio::task::JoinHandle<Result> {
    let service = service.clone();
    tokio::spawn(async move { service.generate(inputs).await })
}

#[test_r::test]
async fn same_snapshot_coalesces_and_survives_caller_cancellation() {
    let single_flight = corpus_case("cache-single-flight-json-yaml");
    let disconnect = corpus_case("cache-waiter-disconnect");
    let (service, mut calls) = controlled();
    let snapshot = inputs(1).await;
    let cancelled = generate(&service, snapshot.clone());
    let (_, reply) = calls.recv().await.unwrap();
    let waiter = generate(&service, snapshot.clone());
    cancelled.abort();
    reply.send(Ok(document())).unwrap();

    let generated = waiter.await.unwrap().unwrap();
    let cached = service.generate(snapshot).await.unwrap();
    assert!(Arc::ptr_eq(&generated, &cached), "{}", single_flight["id"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&generated.json).unwrap(),
        serde_yaml::from_slice::<serde_json::Value>(&generated.yaml).unwrap(),
        "{}",
        single_flight["id"]
    );
    assert!(calls.try_recv().is_err());
    assert_eq!(disconnect["expect"]["provider_calls"], 1);
    assert_eq!(disconnect["expect"]["provider_cancellations"], 0);
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
    let configured = corpus_case("cache-configured-http-origin");
    let (service, mut calls) = controlled();
    let first = inputs(1).await;
    let public_origin = configured["input"]["public_origin"]
        .as_str()
        .unwrap()
        .to_string();
    let second = Arc::new(OpenApiInputs {
        key: OpenApiKey::from_inputs(&public_origin, &first.routes),
        public_origin,
        routes: first.routes.clone(),
    });
    assert_ne!(first.key, second.key);

    for snapshot in [first, second.clone()] {
        let generation = generate(&service, snapshot);
        calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
        generation.await.unwrap().unwrap();
    }
    assert!(calls.try_recv().is_err());
    let generated = service.generate(second).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&generated.json).unwrap();
    assert_eq!(
        value["servers"], configured["expect"]["servers"],
        "{}",
        configured["id"]
    );
}

#[test_r::test]
async fn failed_generation_is_retried_for_the_same_snapshot() {
    let case = corpus_case("cache-failure-retries-next-request");
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
    assert_eq!(case["expect"]["provider_calls"], 2, "{}", case["id"]);
    assert_eq!(case["expect"]["statuses"], serde_json::json!([502, 200]));
}

#[test_r::test]
async fn changed_snapshot_does_not_discard_an_in_flight_generation() {
    for id in [
        "cache-secret-change-allows-old-fill",
        "cache-deployment-change-allows-old-fill",
    ] {
        let case = corpus_case(id);
        let (service, mut calls) = controlled();
        let first = inputs(1).await;
        let public_origin = "https://changed.example.com".to_string();
        let second = Arc::new(OpenApiInputs {
            key: OpenApiKey::from_inputs(&public_origin, &first.routes),
            public_origin,
            routes: first.routes.clone(),
        });
        assert_ne!(first.key, second.key, "{id}");

        let old_waiter = generate(&service, first);
        let (_, old_reply) = calls.recv().await.unwrap();
        let new_waiter = generate(&service, second);
        let (_, new_reply) = calls.recv().await.unwrap();
        let with_fill = |fill: &str| {
            let mut value: serde_json::Value = serde_json::from_str(&document()).unwrap();
            value["x-fill"] = fill.into();
            value.to_string()
        };
        old_reply.send(Ok(with_fill("f1"))).unwrap();
        new_reply.send(Ok(with_fill("f2"))).unwrap();
        let old = old_waiter.await.unwrap().unwrap();
        let new = new_waiter.await.unwrap().unwrap();
        let old: serde_json::Value = serde_json::from_slice(&old.json).unwrap();
        let new: serde_json::Value = serde_json::from_slice(&new.json).unwrap();
        assert_eq!(old["x-fill"], "f1", "{id}");
        assert_eq!(new["x-fill"], "f2", "{id}");
        assert!(calls.try_recv().is_err(), "{id}");
        if let Some(expected) = case["expect"].get("provider_calls") {
            assert_eq!(*expected, 2, "{id}");
        }
        if let Some(fills) = case["expect"].get("published_fills") {
            assert_eq!(fills.as_array().unwrap().len(), 2, "{id}");
        }
    }
}

#[test_r::test]
async fn configured_origin_is_independent_of_the_first_waiter_transport() {
    let case = corpus_case("cache-origin-not-first-waiter");
    let (service, mut calls) = controlled();
    let original = inputs(1).await;
    let public_origin = case["input"]["public_origin"].as_str().unwrap().to_string();
    let snapshot = Arc::new(OpenApiInputs {
        key: OpenApiKey::from_inputs(&public_origin, &original.routes),
        public_origin,
        routes: original.routes.clone(),
    });
    let json_waiter = generate(&service, snapshot.clone());
    let (_, reply) = calls.recv().await.unwrap();
    let yaml_waiter = generate(&service, snapshot);
    reply.send(Ok(document())).unwrap();
    let json = json_waiter.await.unwrap().unwrap();
    let yaml = yaml_waiter.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&json, &yaml), "{}", case["id"]);
    let value: serde_json::Value = serde_json::from_slice(&json.json).unwrap();
    assert_eq!(
        value["servers"], case["expect"]["servers"],
        "{}",
        case["id"]
    );
    assert_eq!(
        value,
        serde_yaml::from_slice::<serde_json::Value>(&yaml.yaml).unwrap(),
        "{}",
        case["id"]
    );
    assert!(calls.try_recv().is_err());
}
