// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use std::time::Duration;
use test_r::{test, timeout};

#[test]
#[timeout("5s")]
async fn concurrency_is_shared_between_separate_users() {
    let transport = McpTransport::new(Limits {
        concurrency: 1,
        timeout: Duration::from_secs(1),
        ..Limits::default()
    })
    .unwrap();
    let first_user = transport.clone();
    let second_user = transport.clone();
    let first = first_user.acquire().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second_user.acquire())
            .await
            .is_err()
    );
    drop(first);
    second_user.acquire().await.unwrap();
}

#[test]
#[timeout("5s")]
async fn saturated_admission_exhausts_the_operation_deadline() {
    let transport = McpTransport::new(Limits {
        concurrency: 1,
        timeout: Duration::from_millis(50),
        ..Limits::default()
    })
    .unwrap();
    let first = transport.acquire().await.unwrap();
    assert!(matches!(
        transport.acquire().await,
        Err(TransportError::Timeout)
    ));
    assert!(matches!(first.limits(), Err(TransportError::Timeout)));
    drop(first);
    transport.acquire().await.unwrap();
}

#[test]
fn unrepresentable_deadline_is_rejected_at_construction() {
    assert!(matches!(
        McpTransport::new(Limits {
            timeout: Duration::MAX,
            ..Limits::default()
        }),
        Err(TransportError::Configuration(_))
    ));
}

#[test]
async fn permit_exposes_configured_bounds_and_remaining_total_timeout() {
    let configured = Limits {
        request_bytes: 123,
        response_bytes: 456,
        concurrency: 2,
        timeout: Duration::from_secs(2),
        ..Limits::default()
    };
    let permit = McpTransport::new(configured)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let effective = permit.limits().unwrap();
    assert_eq!(effective.request_bytes, 123);
    assert_eq!(effective.response_bytes, 456);
    assert!(effective.timeout <= configured.timeout);
    assert!(!effective.timeout.is_zero());
}
