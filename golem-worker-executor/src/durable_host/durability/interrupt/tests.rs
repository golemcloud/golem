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
use test_r::test;

#[test]
async fn synthetic_interrupt_latch_covers_late_and_concurrent_subscribers() {
    use futures::FutureExt;
    let signal = Arc::new(tokio::sync::broadcast::channel(1).0);
    let status = std::sync::RwLock::new(ExecutionStatus::Running {
        agent_mode: golem_common::model::agent::AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
        interrupt_signal: signal.clone(),
    });
    assert!(signal.send(InterruptKind::Restart).is_err());
    assert!(matches!(
        subscribe_interrupt_with_latch(&status, || true).now_or_never(),
        Some(InterruptKind::Interrupt(_))
    ));
    let concurrent = subscribe_interrupt_with_latch(&status, || {
        signal.send(InterruptKind::Restart).unwrap();
        false
    });
    assert!(matches!(
        concurrent.now_or_never(),
        Some(InterruptKind::Restart)
    ));
    *status.write().unwrap() = ExecutionStatus::Interrupting {
        interrupt_kind: InterruptKind::ShardLost,
        await_interruption: Arc::new(tokio::sync::broadcast::channel(1).0),
        agent_mode: golem_common::model::agent::AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
    };
    assert!(matches!(
        subscribe_interrupt_with_latch(&status, || true).now_or_never(),
        Some(InterruptKind::ShardLost)
    ));
}
