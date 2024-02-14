// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use golem_common::model::{InvocationKey, WorkerId};
use tokio::sync::broadcast::{Receiver, Sender};
use tracing::debug;
use uuid::Uuid;

use crate::error::GolemError;
use crate::metrics::invocation_keys::{
    record_confirmed_invocation_keys_count, record_pending_invocation_keys_count,
};

/// Service responsible for generating and looking up invocation keys
#[async_trait]
pub trait InvocationKeyService {
    fn generate_key(&self, worker_id: &WorkerId) -> InvocationKey;
    fn lookup_key(&self, worker_id: &WorkerId, key: &InvocationKey) -> LookupResult;
    fn confirm_key(
        &self,
        worker_id: &WorkerId,
        key: &InvocationKey,
        vals: Result<Vec<golem_wasm_rpc::protobuf::Val>, GolemError>,
    );
    fn interrupt_key(&self, worker_id: &WorkerId, key: &InvocationKey);
    fn resume_key(&self, worker_id: &WorkerId, key: &InvocationKey);
    async fn wait_for_confirmation(
        &self,
        worker_id: &WorkerId,
        key: &InvocationKey,
    ) -> LookupResult;
}

#[derive(Debug)]
pub struct InvocationKeyServiceDefault {
    state: Arc<Mutex<State>>,
    #[allow(unused)]
    confirm_receiver: Receiver<(WorkerId, InvocationKey)>,
    confirm_sender: Sender<(WorkerId, InvocationKey)>,
    pending_key_retention: Duration,
}

#[derive(Debug)]
struct State {
    pending_keys: std::collections::HashMap<(WorkerId, InvocationKey), PendingStatus>,
    confirmed_keys: std::collections::HashMap<
        (WorkerId, InvocationKey),
        Result<Vec<golem_wasm_rpc::protobuf::Val>, GolemError>,
    >,
}

#[derive(Clone, Debug)]
struct PendingStatus {
    started_at: Instant,
    interrupted: bool,
}

impl PendingStatus {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            interrupted: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LookupResult {
    Invalid,
    Pending,
    Interrupted,
    Complete(Result<Vec<golem_wasm_rpc::protobuf::Val>, GolemError>),
}

impl Default for InvocationKeyServiceDefault {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), 1024)
    }
}

impl InvocationKeyServiceDefault {
    /// Creates a new instance of the default implementation of the invocation key service.
    /// Parameters:
    /// - `pending_key_retention`: how long to keep keys which are pending before removing them from memory
    /// - `confirm_queue_capacity`: how many keys can be simultaneously enqueued for confirmation. If this value is lower than the maximum number of concurrent `wait_for_confirmation` calls, some of those will potentially miss the confirmation event.
    pub fn new(pending_key_retention: Duration, confirm_queue_capacity: usize) -> Self {
        let (confirm_sender, confirm_receiver) =
            tokio::sync::broadcast::channel(confirm_queue_capacity);
        Self {
            state: Arc::new(Mutex::new(State {
                pending_keys: std::collections::HashMap::new(),
                confirmed_keys: std::collections::HashMap::new(),
            })),
            confirm_receiver,
            confirm_sender,
            pending_key_retention,
        }
    }

    fn cleanup(&self) {
        self.state
            .lock()
            .unwrap()
            .pending_keys
            .retain(|_, v| v.started_at.elapsed() < self.pending_key_retention);
    }
}

#[async_trait]
impl InvocationKeyService for InvocationKeyServiceDefault {
    fn generate_key(&self, worker_id: &WorkerId) -> InvocationKey {
        self.cleanup();
        let mut state = self.state.lock().unwrap();

        let uuid = Uuid::new_v4();
        let key = InvocationKey::new(uuid.to_string());
        state
            .pending_keys
            .insert((worker_id.clone(), key.clone()), PendingStatus::new());

        record_pending_invocation_keys_count(state.pending_keys.len());

        key
    }

    fn lookup_key(&self, worker_id: &WorkerId, key: &InvocationKey) -> LookupResult {
        self.cleanup();
        let key = (worker_id.clone(), key.clone());
        let state = self.state.lock().unwrap();
        match state.confirmed_keys.get(&key) {
            Some(vals) => LookupResult::Complete(vals.clone()),
            None => match state.pending_keys.get(&key) {
                Some(PendingStatus { interrupted, .. }) => {
                    if *interrupted {
                        LookupResult::Interrupted
                    } else {
                        LookupResult::Pending
                    }
                }
                None => LookupResult::Invalid,
            },
        }
    }

    fn confirm_key(
        &self,
        worker_id: &WorkerId,
        key: &InvocationKey,
        vals: Result<Vec<golem_wasm_rpc::protobuf::Val>, GolemError>,
    ) {
        self.cleanup();
        let key = (worker_id.clone(), key.clone());

        {
            let mut state = self.state.lock().unwrap();
            state.pending_keys.remove(&key);
            state.confirmed_keys.insert(key.clone(), vals);

            record_pending_invocation_keys_count(state.pending_keys.len());
            record_confirmed_invocation_keys_count(state.confirmed_keys.len());
        }

        self.confirm_sender
            .send(key)
            .expect("failed to send confirmation");
    }

    fn interrupt_key(&self, worker_id: &WorkerId, key: &InvocationKey) {
        self.cleanup();
        let key = (worker_id.clone(), key.clone());
        let confirm = {
            let mut state = self.state.lock().unwrap();
            if let Some(status) = state.pending_keys.get_mut(&key) {
                status.interrupted = true;
                true
            } else {
                false
            }
        };
        if confirm {
            self.confirm_sender
                .send(key)
                .expect("failed to send confirmation");
        }
    }

    fn resume_key(&self, worker_id: &WorkerId, key: &InvocationKey) {
        self.cleanup();
        let key = (worker_id.clone(), key.clone());
        let mut state = self.state.lock().unwrap();
        if let Some(status) = state.pending_keys.get_mut(&key) {
            status.interrupted = false;
        }
    }

    async fn wait_for_confirmation(
        &self,
        worker_id: &WorkerId,
        key: &InvocationKey,
    ) -> LookupResult {
        debug!("wait_for_confirmation {key:?}");
        loop {
            match self.lookup_key(worker_id, key) {
                LookupResult::Invalid => break LookupResult::Invalid,
                LookupResult::Interrupted => break LookupResult::Interrupted,
                LookupResult::Pending => {
                    let expected_key: Option<(WorkerId, InvocationKey)> =
                        Some((worker_id.clone(), key.clone()));
                    let mut receiver = self.confirm_sender.subscribe();
                    let confirmed_key = receiver.recv().await.ok();
                    if confirmed_key == expected_key {
                        break self.lookup_key(worker_id, key);
                    } else {
                        continue;
                    }
                }
                LookupResult::Complete(result) => break LookupResult::Complete(result),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use golem_common::model::{TemplateId, WorkerId};
    use golem_wasm_rpc::protobuf::{val, Val};

    use crate::services::invocation_key::{
        InvocationKeyService, InvocationKeyServiceDefault, LookupResult,
    };

    #[cfg(test)]
    #[test]
    fn replay_in_same_order_works() {
        let svc1 = InvocationKeyServiceDefault::default();
        let uuid = uuid::Uuid::parse_str("14e55083-2ff5-44ec-a414-595a748b19a0").unwrap();

        let worker_id = WorkerId {
            template_id: TemplateId(uuid),
            worker_name: "1".to_string(),
        };

        let key1 = svc1.generate_key(&worker_id);
        let key2 = svc1.generate_key(&worker_id);
        let key3 = svc1.generate_key(&worker_id);

        let svc2 = InvocationKeyServiceDefault::default();
        svc2.confirm_key(
            &worker_id,
            &key1,
            Ok(vec![Val {
                val: Some(val::Val::U32(1)),
            }]),
        );
        svc2.confirm_key(
            &worker_id,
            &key2,
            Ok(vec![Val {
                val: Some(val::Val::U32(2)),
            }]),
        );
        svc2.confirm_key(
            &worker_id,
            &key3,
            Ok(vec![Val {
                val: Some(val::Val::U32(3)),
            }]),
        );

        let r1 = svc2.lookup_key(&worker_id, &key1);
        let r2 = svc2.lookup_key(&worker_id, &key2);
        let r3 = svc2.lookup_key(&worker_id, &key3);

        assert_eq!(
            r1,
            LookupResult::Complete(Ok(vec!(Val {
                val: Some(val::Val::U32(1))
            })))
        );
        assert_eq!(
            r2,
            LookupResult::Complete(Ok(vec!(Val {
                val: Some(val::Val::U32(2))
            })))
        );
        assert_eq!(
            r3,
            LookupResult::Complete(Ok(vec!(Val {
                val: Some(val::Val::U32(3))
            })))
        );
    }
}
