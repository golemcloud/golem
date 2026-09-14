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

use golem_common::model::{AgentId, AgentInvocationOutput, IdempotencyKey};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

pub struct Events {
    sender: tokio::sync::broadcast::Sender<Arc<Event>>,
    _receiver: tokio::sync::broadcast::Receiver<Arc<Event>>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new(32768)
    }
}

impl Events {
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = tokio::sync::broadcast::channel(capacity);
        Self {
            sender,
            _receiver: receiver,
        }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.sender.send(Arc::new(event));
    }

    pub fn subscribe(&self) -> EventsSubscription {
        EventsSubscription {
            receiver: self.sender.subscribe(),
        }
    }
}

pub struct EventsSubscription {
    receiver: tokio::sync::broadcast::Receiver<Arc<Event>>,
}

impl EventsSubscription {
    pub async fn wait_for<F, R>(&mut self, f: F) -> Result<R, RecvError>
    where
        F: Fn(&Event) -> Option<R>,
    {
        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if let Some(result) = f(event.as_ref()) {
                        break Ok(result);
                    } else {
                        continue;
                    }
                }
                Err(err) => break Err(err),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    InvocationCompleted {
        agent_id: AgentId,
        idempotency_key: IdempotencyKey,
        result: Result<AgentInvocationOutput, WorkerExecutorError>,
    },
    WorkerLoaded {
        agent_id: AgentId,
        start_attempt: Uuid,
        result: Result<(), WorkerExecutorError>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::component::ComponentId;
    use golem_common::model::{AgentInvocationOutput, AgentInvocationResult};
    use golem_common::schema::SchemaValue;
    use test_r::{test, timeout};

    fn agent_id(name: &str) -> AgentId {
        AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        }
    }

    fn output(value: &str) -> AgentInvocationOutput {
        AgentInvocationOutput {
            result: AgentInvocationResult::AgentMethod {
                output: SchemaValue::String(value.to_string()),
            },
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        }
    }

    fn invocation_completed(agent_id: AgentId, key: &str, value: &str) -> Event {
        Event::InvocationCompleted {
            agent_id,
            idempotency_key: IdempotencyKey::new(key.to_string()),
            result: Ok(output(value)),
        }
    }

    #[test]
    #[timeout("5s")]
    async fn waiters_ignore_multiple_unrelated_large_outputs_and_match_agent_and_key() {
        let events = Events::new(16);
        let mut first = events.subscribe();
        let mut second = events.subscribe();
        let first_agent = agent_id("first-agent");
        let second_agent = agent_id("second-agent");
        let unrelated_payload = "x".repeat(1024 * 1024);

        for index in 0..3 {
            events.publish(invocation_completed(
                agent_id(&format!("unrelated-{index}")),
                "unrelated-key",
                &unrelated_payload,
            ));
        }
        events.publish(invocation_completed(
            first_agent.clone(),
            "wrong-key",
            "same-agent-wrong-key",
        ));
        events.publish(invocation_completed(
            agent_id("wrong-agent"),
            "first-key",
            "wrong-agent-same-key",
        ));
        events.publish(invocation_completed(
            second_agent.clone(),
            "wrong-key",
            "same-second-agent-wrong-key",
        ));
        events.publish(invocation_completed(
            agent_id("other-wrong-agent"),
            "second-key",
            "other-agent-same-second-key",
        ));
        events.publish(invocation_completed(
            first_agent.clone(),
            "first-key",
            "first-result",
        ));
        events.publish(invocation_completed(
            second_agent.clone(),
            "second-key",
            "second-result",
        ));

        let first_result = first
            .wait_for(|event| match event {
                Event::InvocationCompleted {
                    agent_id,
                    idempotency_key,
                    result,
                } if agent_id == &first_agent
                    && idempotency_key == &IdempotencyKey::new("first-key".to_string()) =>
                {
                    Some(result.clone())
                }
                _ => None,
            })
            .await
            .unwrap()
            .unwrap();
        let second_result = second
            .wait_for(|event| match event {
                Event::InvocationCompleted {
                    agent_id,
                    idempotency_key,
                    result,
                } if agent_id == &second_agent
                    && idempotency_key == &IdempotencyKey::new("second-key".to_string()) =>
                {
                    Some(result.clone())
                }
                _ => None,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(first_result, output("first-result"));
        assert_eq!(second_result, output("second-result"));
    }

    #[test]
    #[timeout("5s")]
    async fn subscribers_share_the_published_event() {
        let events = Events::new(1);
        let mut first = events.subscribe();
        let mut second = events.subscribe();
        events.publish(invocation_completed(agent_id("agent"), "key", "result"));

        let first_event = first.receiver.recv().await.unwrap();
        let second_event = second.receiver.recv().await.unwrap();

        assert!(Arc::ptr_eq(&first_event, &second_event));
    }

    #[test]
    #[timeout("5s")]
    async fn worker_loaded_matches_agent_and_start_attempt() {
        let events = Events::new(4);
        let mut subscription = events.subscribe();
        let target_agent = agent_id("target");
        let target_attempt = Uuid::new_v4();
        events.publish(Event::WorkerLoaded {
            agent_id: agent_id("wrong-agent"),
            start_attempt: target_attempt,
            result: Err(WorkerExecutorError::unknown("wrong agent")),
        });
        events.publish(Event::WorkerLoaded {
            agent_id: target_agent.clone(),
            start_attempt: Uuid::new_v4(),
            result: Err(WorkerExecutorError::unknown("wrong attempt")),
        });
        events.publish(Event::WorkerLoaded {
            agent_id: target_agent.clone(),
            start_attempt: target_attempt,
            result: Ok(()),
        });

        let result = subscription
            .wait_for(|event| match event {
                Event::WorkerLoaded {
                    agent_id,
                    start_attempt,
                    result,
                } if agent_id == &target_agent && start_attempt == &target_attempt => {
                    Some(result.clone())
                }
                _ => None,
            })
            .await
            .unwrap();

        assert!(result.is_ok());
    }

    #[test]
    #[timeout("5s")]
    async fn wait_for_reports_lag() {
        let events = Events::new(1);
        let mut subscription = events.subscribe();
        events.publish(Event::WorkerLoaded {
            agent_id: agent_id("first"),
            start_attempt: Uuid::new_v4(),
            result: Ok(()),
        });
        events.publish(Event::WorkerLoaded {
            agent_id: agent_id("second"),
            start_attempt: Uuid::new_v4(),
            result: Ok(()),
        });

        assert!(matches!(
            subscription.wait_for(|_| Some(())).await,
            Err(RecvError::Lagged(1))
        ));
    }

    #[test]
    #[timeout("5s")]
    async fn wait_for_reports_closed_channel() {
        let mut subscription = {
            let events = Events::new(1);
            events.subscribe()
        };

        assert!(matches!(
            subscription.wait_for(|_| Some(())).await,
            Err(RecvError::Closed)
        ));
    }
}
