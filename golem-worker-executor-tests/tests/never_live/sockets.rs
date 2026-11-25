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

//! Native sockets in the history of a never-live fork, with the `SocketSession` agent
//! (host-api-tests), which keeps P2 and P3 sockets in named slots across invocations.

use super::peers::{TcpPeer, UdpPeer};
use super::{settled_update_outcomes, tip};
use crate::Tracing;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{OplogIndex, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const SOCKET_SESSION: &str = "SocketSession";

/// An executor that lets agents use sockets and keeps fork activation local, with the
/// host-api-tests component.
struct Sockets {
    executor: TestWorkerExecutor,
    context: TestContext,
    component: ComponentDto,
}

impl Sockets {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        host_api_tests: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor = crate::fork::start_with_local_resume_and(
            deps,
            &context,
            TestExecutorOverrides::agent_sockets_enabled(),
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        Ok(Self {
            executor,
            context,
            component,
        })
    }

    async fn source(&self, name: &str) -> anyhow::Result<OwnedAgentId> {
        self.executor
            .start_agent(
                &self.component.id,
                agent_key!(SOCKET_SESSION, name.to_string()),
            )
            .await
    }

    async fn call<T: golem_common::schema::FromSchema>(
        &self,
        name: &str,
        method: &str,
        params: golem_common::schema::TypedSchemaValue,
    ) -> anyhow::Result<T> {
        self.executor
            .invoke_and_await_agent(
                &self.component,
                &agent_key!(SOCKET_SESSION, name.to_string()),
                method,
                params,
            )
            .await?
            .into_typed::<T>()
    }

    /// Forks `source_id` at its tip with the never-live mark and applies an automatic update to
    /// its own revision. Returns the fork and the index of its last entry before the update.
    async fn marked_update(
        &self,
        source_id: &OwnedAgentId,
    ) -> anyhow::Result<(OwnedAgentId, OplogIndex)> {
        let cut = tip(&self.executor, source_id).await?;
        let fork = self.executor.fork_worker_never_live(source_id, cut).await?;
        let before = tip(&self.executor, &fork).await?;
        self.executor
            .auto_update_worker(&fork, self.component.revision, false)
            .await?;
        Ok((fork, before))
    }

    async fn outcomes(
        &self,
        fork: &OwnedAgentId,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        settled_update_outcomes(&self.executor, fork).await
    }

    /// The entries of the fork after `before`, which must be only its update entries.
    async fn appended(
        &self,
        fork: &OwnedAgentId,
        before: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntry>> {
        Ok(self
            .executor
            .get_oplog(fork, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .filter(|entry| entry.oplog_index > before)
            .map(|entry| entry.entry)
            .collect())
    }
}

fn assert_only_update_entries(appended: &[PublicOplogEntry]) {
    assert!(
        appended.iter().all(|entry| matches!(
            entry,
            PublicOplogEntry::PendingUpdate(_)
                | PublicOplogEntry::SuccessfulUpdate(_)
                | PublicOplogEntry::FailedUpdate(_)
        )),
        "{appended:?}"
    );
}

/// E14, and the socket half of E19, as Simon decided: a source made a P2 TCP connection and
/// sent a P2 UDP datagram to counting peers. The P2 TCP connect is a durable call that records
/// its result. A never-live fork, updated to its own revision, replays that history without
/// connecting and without sending, and writes exactly one `SuccessfulUpdate` and no other entry.
/// A replayed P2 TCP session is admitted, as a replayed UDP session is (E21); a connect that
/// executes now stays refused. The control is an executor restart of the source, whose ordinary
/// replay of the recorded connect restores the local address of the connection and makes no
/// connection either.
#[test]
#[timeout("4m")]
async fn a_never_live_fork_replays_its_p2_sockets_without_reaching_a_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let sockets = Sockets::start(last_unique_id, deps, host_api_tests).await?;
    let tcp = TcpPeer::silent().await?;
    let udp = UdpPeer::start().await?;
    let source_id = sockets.source("e14").await?;
    let connected: Result<(), String> = sockets
        .call(
            "e14",
            "p2_tcp_connect",
            data_value!("t".to_string(), tcp.port),
        )
        .await?;
    assert_eq!(connected, Ok(()));
    let bound: Result<(), String> = sockets
        .call("e14", "p2_udp_bind", data_value!("u".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let sent: Result<u64, String> = sockets
        .call(
            "e14",
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(udp.port), b"e14".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    tcp.wait_for(1).await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while udp.datagrams() < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    let (fork, before) = sockets.marked_update(&source_id).await?;
    let outcomes = sockets.outcomes(&fork).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(tcp.connections(), 1, "the fork connected");
    assert_eq!(udp.datagrams(), 1, "the fork sent a datagram");
    assert_only_update_entries(&sockets.appended(&fork, before).await?);
    assert_eq!(sockets.outcomes(&fork).await?.len(), 1);
    let address: Result<String, String> = sockets
        .call("e14", "p2_tcp_local_address", data_value!("t".to_string()))
        .await?;
    let address = address.map_err(anyhow::Error::msg)?;

    let Sockets {
        executor,
        context,
        component,
    } = sockets;
    drop(executor);
    let restarted = crate::fork::start_with_local_resume_and(
        deps,
        &context,
        TestExecutorOverrides::agent_sockets_enabled(),
    )
    .await?;
    let restored: Result<String, String> = restarted
        .invoke_and_await_agent(
            &component,
            &agent_key!(SOCKET_SESSION, "e14".to_string()),
            "p2_tcp_local_address",
            data_value!("t".to_string()),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        restored,
        Ok(address),
        "the restarted source did not replay its connection"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(tcp.connections(), 1, "the restarted source connected");
    assert_eq!(udp.datagrams(), 1, "the restarted source sent a datagram");
    Ok(())
}

/// E18, as Simon decided: a source binds a P3 TCP socket on a loopback port and asks it to
/// listen, which the socket rule refuses for every agent. A never-live fork updated to its own
/// revision replays the bind and the listen and publishes no endpoint: a client's connect to the
/// recorded port is refused, during and after the update. The update writes one
/// `SuccessfulUpdate`; listen gets no never-live refusal of its own. The source keeps only
/// whether the listen failed: a replayed bind of any agent binds no native socket, so the
/// replayed listen fails with `connection-broken` where the live one failed with
/// `access-denied`.
#[test]
#[timeout("4m")]
async fn a_replayed_p3_listen_of_a_never_live_fork_publishes_no_endpoint(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let sockets = Sockets::start(last_unique_id, deps, host_api_tests).await?;
    let port = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await?
        .local_addr()?
        .port();
    let source_id = sockets.source("e18").await?;
    let bound: Result<(), String> = sockets
        .call("e18", "p3_tcp_bind", data_value!("l".to_string(), port))
        .await?;
    assert_eq!(bound, Ok(()));
    let refused: Result<bool, String> = sockets
        .call("e18", "p3_tcp_listen_refused", data_value!("l".to_string()))
        .await?;
    assert_eq!(refused, Ok(true), "an agent listened");
    let closed: bool = sockets
        .call("e18", "close", data_value!("l".to_string()))
        .await?;
    assert!(closed);

    let (fork, before) = sockets.marked_update(&source_id).await?;
    let mut connects = 0;
    loop {
        let refused = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err();
        assert!(refused, "a client connected to the recorded port");
        connects += 1;
        let outcomes = super::update_outcomes(&sockets.executor, &fork).await?;
        if !outcomes.is_empty() && !sockets.executor.worker_is_loaded(&fork).await {
            break;
        }
        assert!(connects < 9_000, "the update never ended");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let outcomes = sockets.outcomes(&fork).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
    assert_only_update_entries(&sockets.appended(&fork, before).await?);
    Ok(())
}

/// E21, as Simon decided: a source binds a P2 UDP socket and receives one datagram, with no send
/// and no connect. Its never-live fork, updated to its own revision while a test peer keeps
/// sending fresh datagrams to the recorded port, replays the recorded receive: no fresh datagram
/// reaches the fork's oplog, the replay does not wait for one, and the update writes one
/// `SuccessfulUpdate`. Without a fresh datagram it ends the same way. A replayed UDP session is
/// not refused; a receive that executes in a load hook is, in
/// `sockets_in_a_load_hook_of_a_never_live_fork_reach_no_peer`.
#[test]
#[timeout("4m")]
async fn a_replayed_p2_udp_receive_of_a_never_live_fork_gets_no_fresh_datagram(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let sockets = Sockets::start(last_unique_id, deps, host_api_tests).await?;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let port = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await?
        .local_addr()?
        .port();
    let source_id = sockets.source("e21").await?;
    let bound: Result<(), String> = sockets
        .call("e21", "p2_udp_bind", data_value!("u".to_string(), port))
        .await?;
    assert_eq!(bound, Ok(()));
    let (received, sent) = tokio::join!(
        sockets.call::<Result<(Vec<u8>, String), String>>(
            "e21",
            "p2_udp_receive",
            data_value!("u".to_string())
        ),
        async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            sender.send_to(b"recorded", ("127.0.0.1", port)).await
        }
    );
    sent?;
    assert_eq!(received?.map(|(data, _)| data), Ok(b"recorded".to_vec()));
    let closed: bool = sockets
        .call("e21", "close", data_value!("u".to_string()))
        .await?;
    assert!(closed);

    for fresh in [true, false] {
        let (fork, before) = sockets.marked_update(&source_id).await?;
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                if fresh {
                    sender.send_to(b"fresh-e21", ("127.0.0.1", port)).await?;
                }
                let outcomes = super::update_outcomes(&sockets.executor, &fork).await?;
                if !outcomes.is_empty() && !sockets.executor.worker_is_loaded(&fork).await {
                    break anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let outcomes = sockets.outcomes(&fork).await?;
        assert_eq!(outcomes.len(), 1, "fresh {fresh}: {outcomes:?}");
        assert!(
            matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
            "fresh {fresh}: {outcomes:?}"
        );
        let appended = sockets.appended(&fork, before).await?;
        assert_only_update_entries(&appended);
        assert!(
            !format!("{appended:?}").contains("fresh-e21"),
            "{appended:?}"
        );
    }
    Ok(())
}
