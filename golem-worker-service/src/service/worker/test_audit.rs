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

//! Audit sinks for the worker service's tests.

use async_trait::async_trait;
use golem_service_base::model::auth::audit::{
    AuditRecord, AuditSink, AuditSinkError, DecisionAudit,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The `Retry-After` of the tests' decision audit.
pub const TEST_RETRY_AFTER: Duration = Duration::from_secs(5);

/// Keeps every record that it stores, or fails every write.
#[derive(Default)]
pub struct TestAuditSink {
    records: Mutex<Vec<AuditRecord>>,
    failing: bool,
}

impl TestAuditSink {
    pub fn keeping() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn failing() -> Arc<Self> {
        Arc::new(Self {
            failing: true,
            ..Self::default()
        })
    }

    pub fn records(&self) -> Vec<AuditRecord> {
        self.records.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuditSink for TestAuditSink {
    async fn record(&self, record: AuditRecord) -> Result<(), AuditSinkError> {
        if self.failing {
            return Err(AuditSinkError::Unavailable("test sink fails".to_string()));
        }
        self.records.lock().unwrap().push(record);
        Ok(())
    }
}

/// A decision audit whose records go to `sink`.
pub fn audit_into(sink: Arc<TestAuditSink>) -> DecisionAudit {
    DecisionAudit::new(sink, TEST_RETRY_AFTER)
}

/// A decision audit that keeps its records where no test looks at them.
pub fn test_audit() -> DecisionAudit {
    audit_into(TestAuditSink::keeping())
}
