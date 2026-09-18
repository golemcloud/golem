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

use super::budget::{Budget, GENERATION_TIMEOUT};
use super::{Freshness, OpenApiDocument, OpenApiError, OpenApiInputs, OpenApiKey, OpenApiService};
use crate::metrics::{record_openapi_cache, record_openapi_generation};
use futures::FutureExt;
use golem_common::model::environment::EnvironmentId;
use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Instant, timeout_at};
use tracing::Instrument;

const SUCCESS_TTL: Duration = Duration::from_secs(300);
const FAILURE_TTL: Duration = Duration::from_secs(5);
const COMPLETED_CAPACITY: usize = 256;
type Result = std::result::Result<Arc<OpenApiDocument>, OpenApiError>;

struct Generation {
    inputs: Arc<OpenApiInputs>,
    environment: Freshness,
    budget: Budget,
    result: watch::Sender<Option<Result>>,
}

impl Generation {
    fn is_current(&self) -> bool {
        self.environment.is_current()
    }
}

struct Completed {
    generation: Arc<Generation>,
    expires_at: Instant,
    result: Result,
}

// Pending entries cannot be evicted by completed-entry LRU pressure. Admission
// remains with the generation's CPU/cleanup tasks, even after invalidation.
#[derive(Default)]
pub(super) struct CacheState {
    pending: HashMap<OpenApiKey, Arc<Generation>>,
    completed: VecDeque<Completed>,
    environments: HashMap<EnvironmentId, Weak<AtomicU64>>,
}

impl OpenApiService {
    pub async fn generate(&self, inputs: Arc<OpenApiInputs>) -> Result {
        let (generation, mut receiver) = {
            let mut state = self.cache.lock().unwrap();
            if !inputs.freshness.is_current() {
                record_openapi_cache("stale-snapshot");
                return Err(OpenApiError::new("stale"));
            }
            state
                .completed
                .retain(|entry| entry.generation.is_current() && Instant::now() < entry.expires_at);
            if let Some(index) = state
                .completed
                .iter()
                .position(|entry| entry.generation.inputs.key == inputs.key)
            {
                let entry = state.completed.remove(index).unwrap();
                let result = if entry.generation.is_current() && inputs.freshness.is_current() {
                    record_openapi_cache(if entry.result.is_ok() {
                        "hit"
                    } else {
                        "failure-hit"
                    });
                    entry.result.clone()
                } else {
                    record_openapi_cache("stale-hit");
                    Err(OpenApiError::new("stale"))
                };
                state.completed.push_back(entry);
                return result;
            }
            if let Some(previous) = state.pending.get(&inputs.key)
                && !previous.is_current()
            {
                previous.budget.cancelled.cancel();
                state.pending.remove(&inputs.key);
            }
            if let Some(pending) = state.pending.get(&inputs.key) {
                record_openapi_cache("join");
                (pending.clone(), pending.result.subscribe())
            } else {
                let lease = Arc::new(self.admission.clone().try_acquire_owned().map_err(|_| {
                    record_openapi_cache("admission");
                    OpenApiError::new("admission")
                })?);
                record_openapi_cache("miss");
                state
                    .environments
                    .retain(|_, counter| counter.strong_count() > 0);
                let counter = state
                    .environments
                    .get(&inputs.key.environment_id)
                    .and_then(Weak::upgrade)
                    .unwrap_or_else(|| {
                        let counter = Arc::new(AtomicU64::new(0));
                        state
                            .environments
                            .insert(inputs.key.environment_id, Arc::downgrade(&counter));
                        counter
                    });
                let (tx, rx) = watch::channel(None);
                let generation = Arc::new(Generation {
                    inputs: inputs.clone(),
                    environment: Freshness::capture(counter),
                    budget: Budget::new(Instant::now() + GENERATION_TIMEOUT),
                    result: tx,
                });
                state.pending.insert(inputs.key.clone(), generation.clone());
                let service = self.clone();
                let task = generation.clone();
                let started = Instant::now();
                tokio::spawn(async move {
                    let work = async {
                        tokio::select! {
                            biased;
                            _ = task.budget.cancelled.cancelled() => Err(OpenApiError::new("stale")),
                            result = timeout_at(task.budget.deadline, service.run(task.inputs.clone(), task.budget.clone(), lease)) =>
                                result.unwrap_or_else(|_| Err(OpenApiError::new("generation-timeout"))),
                        }
                    };
                    let result = AssertUnwindSafe(work)
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| Err(OpenApiError::new("generation-failed")));
                    task.budget.cancelled.cancel();
                    let mut state = service.cache.lock().unwrap();
                    let owns_entry = state
                        .pending
                        .get(&task.inputs.key)
                        .is_some_and(|entry| Arc::ptr_eq(entry, &task));
                    if owns_entry {
                        state.pending.remove(&task.inputs.key);
                    }
                    let result = if task.is_current() && owns_entry {
                        if state.completed.len() == COMPLETED_CAPACITY {
                            state.completed.pop_front();
                        }
                        let ttl = if result.is_ok() {
                            SUCCESS_TTL
                        } else {
                            FAILURE_TTL
                        };
                        state.completed.push_back(Completed {
                            generation: task.clone(),
                            expires_at: Instant::now() + ttl,
                            result: result.clone(),
                        });
                        result
                    } else {
                        record_openapi_cache("stale-fill");
                        Err(OpenApiError::new("stale"))
                    };
                    let outcome = result.as_ref().map_or_else(|error| error.category(), |_| "success");
                    record_openapi_generation(outcome, started.elapsed());
                    tracing::debug!(outcome, "OpenAPI generation completed");
                    task.result.send_replace(Some(result));
                }.instrument(tracing::info_span!("generate_openapi")));
                (generation, rx)
            }
        };
        let result = receiver
            .wait_for(Option::is_some)
            .await
            .map(|value| value.as_ref().unwrap().clone())
            .unwrap_or_else(|_| Err(OpenApiError::new("generation-failed")));
        // Check on delivery as well as publication; an event can race a ready
        // watch notification before this waiter is polled again.
        if !generation.is_current() || !inputs.freshness.is_current() {
            record_openapi_cache("stale-delivery");
            Err(OpenApiError::new("stale"))
        } else {
            result
        }
    }

    pub fn invalidate_environment(&self, environment_id: EnvironmentId) {
        self.invalidate(Some(environment_id));
    }

    pub fn clear(&self) {
        self.invalidate(None);
    }

    fn invalidate(&self, environment_id: Option<EnvironmentId>) {
        let mut state = self.cache.lock().unwrap();
        let matches = |id: EnvironmentId| environment_id.is_none_or(|expected| id == expected);
        for (id, counter) in &state.environments {
            if matches(*id)
                && let Some(counter) = counter.upgrade()
            {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }
        state.pending.retain(|key, generation| {
            if matches(key.environment_id) {
                generation.budget.cancelled.cancel();
                false
            } else {
                true
            }
        });
        state
            .completed
            .retain(|entry| !matches(entry.generation.inputs.key.environment_id));
        state
            .environments
            .retain(|_, counter| counter.strong_count() > 0);
    }
}

#[cfg(test)]
mod tests;
