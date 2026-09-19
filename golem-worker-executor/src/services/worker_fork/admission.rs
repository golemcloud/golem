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

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use golem_common::model::AgentId;
use golem_common::serialization::{deserialize, serialize};
use std::sync::Arc;

const SERVICE: &str = "worker-executor";
const API: &str = "export-fork-admission";
const ENTITY: &str = "fork-reservation";
const CONTROL_KEY: &str = "control";
const RATE_KEY: &str = "rate";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    Reserved { candidate: Vec<u8>, existing: bool },
    Conflict,
    LimitReached,
    RateLimited { retry_after_seconds: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
struct Reservation {
    session: String,
    request_hash: Vec<u8>,
    candidate: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
struct Rate {
    updated_millis: u64,
    credit_millis: u64,
}

impl Rate {
    fn credit(&self, limit: u32, now: u64) -> u64 {
        self.credit_millis
            .saturating_add(
                now.saturating_sub(self.updated_millis)
                    .saturating_mul(u64::from(limit)),
            )
            .min(u64::from(limit) * 1000)
    }
}

#[derive(Debug)]
pub struct ExportForkAdmission {
    storage: Arc<dyn KeyValueStorage + Send + Sync>,
}

impl ExportForkAdmission {
    pub fn new(storage: Arc<dyn KeyValueStorage + Send + Sync>) -> Self {
        Self { storage }
    }

    pub async fn candidate(
        &self,
        namespace: KeyValueStorageNamespace,
        target: &AgentId,
    ) -> Result<Option<Vec<u8>>, KeyValueStorageError> {
        self.storage
            .get(
                SERVICE,
                API,
                ENTITY,
                namespace,
                &format!("target:{}", target.to_redis_key()),
            )
            .await?
            .map(|bytes| decode::<Reservation>(&bytes).map(|reservation| reservation.candidate))
            .transpose()
    }

    /// Reject exhausted budgets before doing a copy. Only reserve's CAS grants admission;
    /// this read-only check cannot charge a failed byte-limit preflight or block a retry.
    pub async fn check(
        &self,
        namespace: KeyValueStorageNamespace,
        target: &AgentId,
        session: &str,
        session_limit: u32,
        rate_limit: u32,
        now: u64,
    ) -> Result<Option<Admission>, KeyValueStorageError> {
        let keys = vec![
            format!("target:{}", target.to_redis_key()),
            format!("session:{}", blake3::hash(session.as_bytes())),
            RATE_KEY.to_string(),
        ]
        .into();
        let rows = self
            .storage
            .get_many(SERVICE, API, ENTITY, namespace, keys)
            .await?;
        if rows[0].is_some() {
            return Ok(None);
        }
        let count = rows[1]
            .as_deref()
            .map(decode::<u32>)
            .transpose()?
            .unwrap_or(0);
        if count >= session_limit {
            return Ok(Some(Admission::LimitReached));
        }
        let rate = rows[2]
            .as_deref()
            .map(decode::<Rate>)
            .transpose()?
            .unwrap_or(Rate {
                updated_millis: now,
                credit_millis: u64::from(rate_limit) * 1000,
            });
        Ok(
            (rate.credit(rate_limit, now) < 1000).then_some(Admission::RateLimited {
                retry_after_seconds: 1,
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn reserve(
        &self,
        namespace: KeyValueStorageNamespace,
        target: AgentId,
        session: String,
        request_hash: Vec<u8>,
        candidate: Vec<u8>,
        max_forks_per_session: u32,
        max_forks_per_second: u32,
        now_millis: u64,
    ) -> Result<Admission, KeyValueStorageError> {
        let target_key = format!("target:{}", target.to_redis_key());
        let session_key = format!("session:{}", blake3::hash(session.as_bytes()));

        loop {
            let keys: Arc<[String]> = vec![
                target_key.clone(),
                CONTROL_KEY.to_string(),
                session_key.clone(),
                RATE_KEY.to_string(),
            ]
            .into();
            let rows = self
                .storage
                .get_many(SERVICE, API, ENTITY, namespace.clone(), keys)
                .await?;

            if let Some(bytes) = &rows[0] {
                let reservation: Reservation = decode(bytes)?;
                return Ok(
                    if reservation.session == session && reservation.request_hash == request_hash {
                        Admission::Reserved {
                            candidate: reservation.candidate,
                            existing: true,
                        }
                    } else {
                        Admission::Conflict
                    },
                );
            }

            let control = rows[1].as_deref();
            let generation = control.map(decode::<u64>).transpose()?.unwrap_or(0);
            let session_count = rows[2]
                .as_deref()
                .map(decode::<u32>)
                .transpose()?
                .unwrap_or(0);
            if session_count >= max_forks_per_session {
                return Ok(Admission::LimitReached);
            }

            let capacity = u64::from(max_forks_per_second) * 1000;
            let rate = rows[3]
                .as_deref()
                .map(decode::<Rate>)
                .transpose()?
                .unwrap_or(Rate {
                    updated_millis: now_millis,
                    credit_millis: capacity,
                });
            let credit = rate.credit(max_forks_per_second, now_millis);
            if credit < 1000 {
                return Ok(Admission::RateLimited {
                    retry_after_seconds: 1,
                });
            }

            let reservation = Reservation {
                session: session.clone(),
                request_hash: request_hash.clone(),
                candidate: candidate.clone(),
            };
            let next_control = serialize_value(&generation.checked_add(1).ok_or_else(|| {
                KeyValueStorageError::other("fork admission control generation overflow")
            })?)?;
            let next_session_count = serialize_value(&(session_count + 1))?;
            let next_rate = serialize_value(&Rate {
                updated_millis: now_millis.max(rate.updated_millis),
                credit_millis: credit - 1000,
            })?;
            let reservation = serialize_value(&reservation)?;
            let pairs = [
                (CONTROL_KEY, next_control.as_slice()),
                (session_key.as_str(), next_session_count.as_slice()),
                (RATE_KEY, next_rate.as_slice()),
                (target_key.as_str(), reservation.as_slice()),
            ];
            if self
                .storage
                .compare_and_set_many(
                    SERVICE,
                    API,
                    ENTITY,
                    namespace.clone(),
                    CONTROL_KEY,
                    control,
                    &[],
                    &pairs,
                )
                .await?
            {
                return Ok(Admission::Reserved {
                    candidate,
                    existing: false,
                });
            }
            // A false response can be a genuine race or an ambiguous retry after this write won.
            // Reloading resolves both without charging the reservation a second time.
        }
    }
}

fn decode<T: desert_rust::BinaryDeserializer>(bytes: &[u8]) -> Result<T, KeyValueStorageError> {
    deserialize(bytes).map_err(KeyValueStorageError::other)
}

fn serialize_value<T: desert_rust::BinarySerializer>(
    value: &T,
) -> Result<Vec<u8>, KeyValueStorageError> {
    serialize(value).map_err(KeyValueStorageError::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::AgentFingerprint;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use test_r::test;
    use uuid::Uuid;

    fn namespace() -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::ExportForkAdmissions {
            environment_id: EnvironmentId(Uuid::from_u128(1)),
            agent_id: target("source"),
            fingerprint: AgentFingerprint(Uuid::from_u128(2)),
        }
    }

    fn target(name: &str) -> AgentId {
        AgentId {
            component_id: ComponentId(Uuid::from_u128(3)),
            agent_id: name.to_string(),
        }
    }

    fn service() -> Arc<ExportForkAdmission> {
        Arc::new(ExportForkAdmission::new(Arc::new(
            InMemoryKeyValueStorage::new(),
        )))
    }

    async fn reserve(
        service: &ExportForkAdmission,
        target_name: &str,
        session: &str,
        hash: u8,
        candidate: u8,
        session_limit: u32,
        rate_limit: u32,
        now: u64,
    ) -> Admission {
        service
            .reserve(
                namespace(),
                target(target_name),
                session.to_string(),
                vec![hash],
                vec![candidate],
                session_limit,
                rate_limit,
                now,
            )
            .await
            .unwrap()
    }

    #[test]
    async fn precheck_is_read_only_and_bypasses_existing_reservations() {
        let service = service();
        assert_eq!(
            service
                .check(namespace(), &target("first"), "s", 1, 1, 0)
                .await
                .unwrap(),
            None
        );
        assert!(matches!(
            reserve(&service, "first", "s", 1, 1, 1, 1, 0).await,
            Admission::Reserved {
                existing: false,
                ..
            }
        ));
        assert_eq!(
            service
                .check(namespace(), &target("first"), "s", 0, 0, 0)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            service
                .check(namespace(), &target("second"), "s", 1, 1, 1000)
                .await
                .unwrap(),
            Some(Admission::LimitReached)
        );
        assert_eq!(
            service
                .check(namespace(), &target("second"), "other", 1, 1, 999)
                .await
                .unwrap(),
            Some(Admission::RateLimited {
                retry_after_seconds: 1
            })
        );
        assert_eq!(
            service
                .check(namespace(), &target("second"), "other", 1, 1, 1000)
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    async fn concurrent_reservations_hit_exact_session_cap() {
        let service = service();
        let mut tasks = Vec::new();
        for n in 0..8 {
            let service = service.clone();
            tasks.push(tokio::spawn(async move {
                reserve(&service, &format!("target-{n}"), "session", n, n, 3, 20, 0).await
            }));
        }
        let mut reserved = 0;
        let mut limited = 0;
        for task in tasks {
            match task.await.unwrap() {
                Admission::Reserved {
                    existing: false, ..
                } => reserved += 1,
                Admission::LimitReached => limited += 1,
                other => panic!("unexpected admission: {other:?}"),
            }
        }
        assert_eq!((reserved, limited), (3, 5));
    }

    #[test]
    async fn same_target_race_retains_first_candidate_and_changed_request_conflicts() {
        let service = service();
        let left = tokio::spawn({
            let service = service.clone();
            async move { reserve(&service, "target", "session", 1, 10, 4, 4, 0).await }
        });
        let right = tokio::spawn({
            let service = service.clone();
            async move { reserve(&service, "target", "session", 1, 20, 4, 4, 0).await }
        });
        let left = left.await.unwrap();
        let right = right.await.unwrap();
        let (first, retry) = match (&left, &right) {
            (
                Admission::Reserved {
                    candidate,
                    existing: false,
                },
                retry,
            ) => (candidate, retry),
            (
                retry,
                Admission::Reserved {
                    candidate,
                    existing: false,
                },
            ) => (candidate, retry),
            _ => panic!("one reservation must win: {left:?}, {right:?}"),
        };
        assert_eq!(
            retry,
            &Admission::Reserved {
                candidate: first.clone(),
                existing: true
            }
        );
        assert_eq!(
            reserve(&service, "target", "session", 2, 30, 4, 4, 0).await,
            Admission::Conflict
        );
        assert_eq!(
            reserve(&service, "target", "other", 1, 30, 4, 4, 0).await,
            Admission::Conflict
        );
    }

    #[test]
    async fn retry_bypasses_exhausted_limits_and_returns_original_candidate() {
        let service = service();
        assert!(matches!(
            reserve(&service, "one", "s", 1, 9, 1, 1, 0).await,
            Admission::Reserved {
                existing: false,
                ..
            }
        ));
        assert_eq!(
            reserve(&service, "two", "s", 2, 8, 1, 1, 0).await,
            Admission::LimitReached
        );
        assert_eq!(
            reserve(&service, "one", "s", 1, 7, 0, 0, 0).await,
            Admission::Reserved {
                candidate: vec![9],
                existing: true
            }
        );
    }

    #[test]
    async fn sessions_have_independent_counts_but_share_rate_and_time_advances_window() {
        let service = service();
        assert!(matches!(
            reserve(&service, "one", "a", 1, 1, 1, 2, 1500).await,
            Admission::Reserved { .. }
        ));
        assert!(matches!(
            reserve(&service, "two", "b", 2, 2, 1, 2, 1500).await,
            Admission::Reserved { .. }
        ));
        assert_eq!(
            reserve(&service, "three", "c", 3, 3, 1, 2, 1500).await,
            Admission::RateLimited {
                retry_after_seconds: 1
            }
        );
        assert_eq!(
            reserve(&service, "three", "c", 3, 3, 1, 2, 1999).await,
            Admission::RateLimited {
                retry_after_seconds: 1
            }
        );
        assert!(matches!(
            reserve(&service, "three", "c", 3, 3, 1, 2, 2000).await,
            Admission::Reserved { .. }
        ));
        assert_eq!(
            reserve(&service, "four", "a", 4, 4, 1, 2, 2000).await,
            Admission::LimitReached
        );
    }
}
