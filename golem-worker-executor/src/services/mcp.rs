// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

pub use golem_mcp_import::transport::Limits;
use golem_mcp_import::transport::TransportError;
use golem_mcp_import::transport::sender::{HttpClient, HttpPolicy, HttpSender};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, timeout_at};

#[derive(Clone)]
pub struct McpTransport {
    limits: Limits,
    permits: Arc<Semaphore>,
    http: HttpClient,
}

impl McpTransport {
    pub fn new(limits: Limits) -> Result<Self, TransportError> {
        limits.validate()?;
        Ok(Self {
            permits: Arc::new(Semaphore::new(limits.concurrency)),
            limits,
            http: HttpClient::new()?,
        })
    }

    /// Acquires executor-wide admission. The returned limits contain only the
    /// time remaining after waiting, so the client's timeout covers the whole operation.
    pub async fn acquire(&self) -> Result<McpTransportPermit, TransportError> {
        let deadline = Instant::now() + self.limits.timeout;
        let permit = timeout_at(deadline, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Configuration("MCP transport closed".into()))?;
        Ok(McpTransportPermit {
            limits: self.limits,
            deadline,
            http: self.http.clone(),
            _permit: permit,
        })
    }
}

pub struct McpTransportPermit {
    limits: Limits,
    deadline: Instant,
    http: HttpClient,
    _permit: OwnedSemaphorePermit,
}

impl McpTransportPermit {
    pub fn limits(&self) -> Result<Limits, TransportError> {
        let mut limits = self.limits;
        limits.timeout = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(TransportError::Timeout)?;
        Ok(limits)
    }

    pub fn sender<P: HttpPolicy>(&self, policy: P) -> HttpSender<P> {
        self.http.sender(policy)
    }
}

#[cfg(test)]
mod tests {
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
}
