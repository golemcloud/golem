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

use super::*;
use crate::custom_api::openapi::service::tests::{
    controlled, document, inputs, paused, provider_routes,
};
use crate::custom_api::route_resolver::tests::test_resolver;
use poem::Request;
use test_r::test;

fn start(service: &OpenApiService, inputs: Arc<OpenApiInputs>) -> tokio::task::JoinHandle<Result> {
    let service = service.clone();
    tokio::spawn(async move { service.generate(inputs).await })
}

async fn ready(task: tokio::task::JoinHandle<Result>) -> Result {
    // Keep the paused clock from auto-advancing past a blocking CPU task. Only
    // the explicit advances in the test move monotonic cache time.
    let wall = std::time::Instant::now();
    while !task.is_finished() {
        assert!(wall.elapsed() < Duration::from_secs(10));
        tokio::task::yield_now().await;
    }
    task.await.unwrap()
}

#[test]
#[test_r::timeout("30s")]
async fn same_key_joins_at_capacity_and_disconnected_owner_does_not_cancel() {
    let (service, mut calls, mut cleanups) = controlled();
    let mut tasks = Vec::new();
    let mut replies = Vec::new();
    let first_inputs = inputs(1).await;
    tasks.push(start(&service, first_inputs.clone()));
    replies.push(calls.recv().await.unwrap().1);
    for i in 1..8 {
        let mut inputs = inputs(1).await;
        Arc::get_mut(&mut inputs).unwrap().key.domain.0 = format!("domain{i}.test");
        tasks.push(start(&service, inputs));
        replies.push(calls.recv().await.unwrap().1);
    }
    let mut waiter = std::pin::pin!(service.generate(first_inputs.clone()));
    assert!(futures::poll!(waiter.as_mut()).is_pending());
    let mut ninth = inputs(1).await;
    Arc::get_mut(&mut ninth).unwrap().key.domain.0 = "ninth.test".into();
    assert_eq!(
        service.generate(ninth).await.unwrap_err().status(),
        http::StatusCode::SERVICE_UNAVAILABLE
    );
    tasks[0].abort();
    for reply in replies {
        reply.send(Ok(document())).unwrap();
    }
    let document = waiter.await.unwrap();
    assert!(Arc::ptr_eq(
        &document,
        &service.generate(first_inputs).await.unwrap()
    ));
    for task in tasks.into_iter().skip(1) {
        task.await.unwrap().unwrap();
    }
    assert!(calls.try_recv().is_err());
    assert!(cleanups.try_recv().is_err());
}

#[test]
fn success_and_failure_ttls_start_at_completion_and_expire_at_boundary() {
    paused(async {
        for (output, ttl) in [
            (Ok(document()), 300),
            (Err(OpenApiError::new("provider-invocation")), 5),
        ] {
            let (service, mut calls, _cleanups) = controlled();
            let inputs = inputs(1).await;
            let task = start(&service, inputs.clone());
            let (_, reply) = calls.recv().await.unwrap();
            tokio::time::advance(Duration::from_secs(4)).await;
            reply.send(output.clone()).unwrap();
            let first = ready(task).await;
            assert_eq!(first.is_ok(), output.is_ok());
            tokio::time::advance(Duration::from_millis(ttl * 1000 - 1)).await;
            assert_eq!(
                service.generate(inputs.clone()).await.is_ok(),
                output.is_ok()
            );
            assert!(calls.try_recv().is_err());
            tokio::time::advance(Duration::from_millis(1)).await;
            let refreshed = start(&service, inputs);
            calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
            assert!(ready(refreshed).await.is_ok());
        }
    });
}

#[test]
#[test_r::timeout("30s")]
async fn completed_cache_is_lru_bounded_and_pending_work_is_not_evicted() {
    let (service, mut calls, _cleanups) = controlled();
    let mut keys = Vec::new();
    for index in 0..=256 {
        let mut inputs = inputs(1).await;
        Arc::get_mut(&mut inputs).unwrap().key.domain.0 = format!("domain{index}.test");
        keys.push(inputs);
    }
    let mut first = None;
    for key in &keys[..256] {
        let task = start(&service, key.clone());
        calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
        let doc = task.await.unwrap().unwrap();
        first.get_or_insert(doc);
    }
    assert!(Arc::ptr_eq(
        first.as_ref().unwrap(),
        &service.generate(keys[0].clone()).await.unwrap()
    ));
    let task = start(&service, keys[256].clone());
    let (_, reply) = calls.recv().await.unwrap();
    // Pending work does not count against completed capacity or evict a hit.
    assert_eq!(service.cache.lock().unwrap().completed.len(), 256);
    reply.send(Ok(document())).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(service.cache.lock().unwrap().completed.len(), 256);
    assert!(Arc::ptr_eq(
        first.as_ref().unwrap(),
        &service.generate(keys[0].clone()).await.unwrap()
    ));
    let evicted = start(&service, keys[1].clone());
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    evicted.await.unwrap().unwrap();
    assert!(calls.try_recv().is_err());
}

#[test]
#[test_r::timeout("30s")]
async fn secret_invalidation_is_environment_scoped_and_fences_delivery_after_publication() {
    let (service, mut calls, _cleanups) = controlled();
    let inputs_a = inputs(1).await;
    let mut inputs_b = inputs(1).await;
    Arc::get_mut(&mut inputs_b).unwrap().key.environment_id = EnvironmentId::new();
    let b = start(&service, inputs_b.clone());
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    let b = b.await.unwrap().unwrap();
    let mut delayed_waiter = std::pin::pin!(service.generate(inputs_a.clone()));
    assert!(futures::poll!(delayed_waiter.as_mut()).is_pending());
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    // Another waiter proves publication without polling the delayed receiver.
    service.generate(inputs_a.clone()).await.unwrap();
    service.invalidate_environment(inputs_a.key.environment_id);
    assert!(delayed_waiter.await.unwrap_err().is_stale());
    assert!(Arc::ptr_eq(&b, &service.generate(inputs_b).await.unwrap()));
    let fresh = start(&service, inputs_a);
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    fresh.await.unwrap().unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn invalidation_cancels_pending_work_and_fences_late_fill() {
    let (service, mut calls, mut cleanups) = controlled();
    let inputs = inputs(1).await;
    let old = start(&service, inputs.clone());
    let (old_key, old_reply) = calls.recv().await.unwrap();
    service.invalidate_environment(inputs.key.environment_id);
    let current = start(&service, inputs.clone());
    let (current_key, current_reply) = calls.recv().await.unwrap();
    assert_ne!(old_key, current_key);
    assert!(old.await.unwrap().unwrap_err().is_stale());
    assert_eq!(cleanups.recv().await.unwrap(), old_key);
    assert!(old_reply.is_closed());
    current_reply.send(Ok(document())).unwrap();
    let current = current.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(
        &current,
        &service.generate(inputs).await.unwrap()
    ));
}

#[test]
#[test_r::timeout("30s")]
async fn obsolete_route_snapshot_is_rejected_on_miss_fill_and_hit() {
    for stage in 0..3 {
        let (service, mut calls, _cleanups) = controlled();
        let snapshot = inputs(1).await;
        let pending = if stage > 0 {
            let task = start(&service, snapshot.clone());
            let (_, reply) = calls.recv().await.unwrap();
            if stage == 2 {
                reply.send(Ok(document())).unwrap();
                task.await.unwrap().unwrap();
                None
            } else {
                Some((task, reply))
            }
        } else {
            None
        };
        snapshot.freshness.counter.fetch_add(1, Ordering::SeqCst);
        if let Some((task, reply)) = pending {
            reply.send(Ok(document())).unwrap();
            assert!(task.await.unwrap().unwrap_err().is_stale());
        }
        assert!(service.generate(snapshot).await.unwrap_err().is_stale());
        assert!(calls.try_recv().is_err());
    }
}

#[test]
#[test_r::timeout("30s")]
async fn cache_key_separates_deployment_domain_and_environment() {
    let (service, mut calls, _cleanups) = controlled();
    for field in 0..4 {
        let mut inputs = inputs(1).await;
        let key = &mut Arc::get_mut(&mut inputs).unwrap().key;
        match field {
            1 => {
                key.deployment_revision =
                    golem_common::model::deployment::DeploymentRevision::new(3).unwrap()
            }
            2 => key.domain.0 = "other.test".into(),
            3 => key.environment_id = EnvironmentId::new(),
            _ => {}
        }
        let task = start(&service, inputs.clone());
        calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
        let result = task.await.unwrap().unwrap();
        assert!(Arc::ptr_eq(
            &result,
            &service.generate(inputs).await.unwrap()
        ));
    }
}

#[test]
#[test_r::timeout("30s")]
async fn unrelated_environment_route_invalidation_keeps_completed_entry_current() {
    let resolver = test_resolver(provider_routes(1));
    let resolved = resolver
        .resolve_matching_route(
            &Request::builder()
                .uri("/openapi.json".parse().unwrap())
                .header("host", "example.com")
                .finish(),
        )
        .await
        .unwrap();
    let inputs = resolved.openapi_inputs.unwrap();
    let environment_a = inputs.key.environment_id;
    let environment_b = EnvironmentId::new();
    assert_ne!(environment_a, environment_b);

    let (service, mut calls, _cleanups) = controlled();
    let first = start(&service, inputs.clone());
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    let first = first.await.unwrap().unwrap();

    resolver
        .invalidate_domains_for_environment(environment_b)
        .await;
    service.invalidate_environment(environment_b);
    assert!(
        service
            .generate(inputs.clone())
            .await
            .unwrap_err()
            .is_stale()
    );
    // Re-resolution is required after the router's global clear, but the
    // environment A document must remain reusable by that fresh snapshot.
    let fresh = Arc::new(OpenApiInputs {
        key: inputs.key.clone(),
        freshness: Freshness::capture(inputs.freshness.counter.clone()),
        public_origin: inputs.public_origin.clone(),
        routes: inputs.routes.clone(),
    });
    let cached = service
        .generate(fresh)
        .await
        .expect("an unrelated environment event must not stale environment A");
    assert!(Arc::ptr_eq(&first, &cached));
    assert!(calls.try_recv().is_err());
}

#[test]
#[test_r::timeout("30s")]
async fn unrelated_route_invalidation_preserves_pending_generation_for_fresh_waiter() {
    let (service, mut calls, mut cleanups) = controlled();
    let inputs = inputs(1).await;
    let old = start(&service, inputs.clone());
    let (_, reply) = calls.recv().await.unwrap();
    inputs.freshness.counter.fetch_add(1, Ordering::SeqCst);
    let fresh = Arc::new(OpenApiInputs {
        key: inputs.key.clone(),
        freshness: Freshness::capture(inputs.freshness.counter.clone()),
        public_origin: inputs.public_origin.clone(),
        routes: inputs.routes.clone(),
    });
    let mut joined = std::pin::pin!(service.generate(fresh));
    assert!(futures::poll!(joined.as_mut()).is_pending());
    reply.send(Ok(document())).unwrap();
    assert!(old.await.unwrap().unwrap_err().is_stale());
    joined.await.unwrap();
    assert!(calls.try_recv().is_err());
    assert!(cleanups.try_recv().is_err());
}

#[test]
fn timed_out_processing_retains_admission_and_cannot_overwrite_cached_failure() {
    paused(async {
        let (mut service, mut calls, _cleanups) = controlled();
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        service.processing_hook = Some(Arc::new(move || {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }));
        let inputs = inputs(1).await;
        let task = start(&service, inputs.clone());
        calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
        let wall = std::time::Instant::now();
        while started_rx.try_recv().is_err() {
            assert!(wall.elapsed() < Duration::from_secs(5));
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(31)).await;
        assert_eq!(
            ready(task).await.unwrap_err().status(),
            http::StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(service.admission.available_permits(), 7);
        release_tx.send(()).unwrap();
        let wall = std::time::Instant::now();
        while service.admission.available_permits() != 8 {
            assert!(wall.elapsed() < Duration::from_secs(5));
            tokio::task::yield_now().await;
        }
        assert_eq!(
            service.generate(inputs).await.unwrap_err().status(),
            http::StatusCode::GATEWAY_TIMEOUT
        );
        assert!(calls.try_recv().is_err());
    });
}
