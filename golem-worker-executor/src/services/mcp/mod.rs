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
mod tests;
