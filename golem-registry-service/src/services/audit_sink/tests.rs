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

use super::*;
use crate::repo::audit_staging::{AuditStagingRecord, AuditStagingRepo};
use assert2::{assert, let_assert};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::authority::{
    AuthorizationRequest, CatalogOperation, Decision, Denial, EnvironmentTarget, Operation,
    Resource, Target,
};
use golem_common::model::caller::Caller;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::model::auth::audit::{
    AuditContext, AuditRecord, AuditSink, AuditSinkError,
};
use golem_service_base::model::auth::surface::Surface;
use golem_service_base::repo::{RepoError, RepoResult};
use std::sync::{Arc, Mutex};
use test_r::test;
use uuid::Uuid;

/// A staging table in memory, or one whose storage is down.
#[derive(Default)]
struct MemoryStaging {
    rows: Mutex<Vec<AuditStagingRecord>>,
    down: bool,
}

#[async_trait]
impl AuditStagingRepo for MemoryStaging {
    async fn insert(&self, record: &AuditStagingRecord) -> RepoResult<()> {
        if self.down {
            return Err(RepoError::InternalError(anyhow::anyhow!(
                "database is down"
            )));
        }
        self.rows.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn get(&self, event_id: Uuid) -> RepoResult<Option<AuditStagingRecord>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|row| row.event_id == event_id)
            .cloned())
    }
}

fn denied_listing() -> AuditRecord {
    let request = AuthorizationRequest::new(
        Caller::anonymous(),
        Operation::single(
            CatalogOperation::ListAgents,
            Resource::None,
            Target::Environment(EnvironmentTarget {
                account_id: AccountId(Uuid::from_u128(0xacc0)),
                application_id: ApplicationId(Uuid::from_u128(0xa0)),
                environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
            }),
        )
        .unwrap(),
    );
    AuditRecord::new(
        &request,
        &Decision::Deny(Denial::SignInRequired { scheme: None }),
        &AuditContext::new(Surface::Rest),
    )
}

#[test]
async fn the_registry_sink_stores_the_record_in_the_staging_table() {
    let staging = Arc::new(MemoryStaging::default());
    let sink = RegistryAuditSink::new(staging.clone());
    let record = denied_listing();

    sink.record(record.clone()).await.unwrap();

    let_assert!(Some(row) = staging.get(record.event_id).await.unwrap());
    assert!(row.outcome == "sign-in-required");
    assert!(row.target_account_id == Some(Uuid::from_u128(0xacc0)));
    assert!(row.target_environment_id == Some(Uuid::from_u128(0xe0)));
    assert!(row.to_audit_record().unwrap() == record);
}

#[test]
async fn a_storage_error_makes_the_registry_sink_unavailable() {
    let sink = RegistryAuditSink::new(Arc::new(MemoryStaging {
        down: true,
        ..Default::default()
    }));

    let result = sink.record(denied_listing()).await;

    let_assert!(Err(AuditSinkError::Unavailable(_)) = result);
}
