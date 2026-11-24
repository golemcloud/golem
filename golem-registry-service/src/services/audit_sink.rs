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

#[cfg(test)]
mod tests;

use crate::repo::audit_staging::{AuditStagingRecord, AuditStagingRepo};
use async_trait::async_trait;
use golem_service_base::model::auth::audit::{AuditRecord, AuditSink, AuditSinkError};
use std::sync::Arc;

/// The registry's audit sink: it writes each record into the registry's staging table.
pub struct RegistryAuditSink {
    staging: Arc<dyn AuditStagingRepo>,
}

impl RegistryAuditSink {
    pub fn new(staging: Arc<dyn AuditStagingRepo>) -> Self {
        Self { staging }
    }
}

#[async_trait]
impl AuditSink for RegistryAuditSink {
    async fn record(&self, record: AuditRecord) -> Result<(), AuditSinkError> {
        let row = AuditStagingRecord::from_audit_record(&record)
            .map_err(|error| AuditSinkError::Unavailable(error.to_string()))?;
        self.staging
            .insert(&row)
            .await
            .map_err(|error| AuditSinkError::Unavailable(error.to_string()))
    }
}
