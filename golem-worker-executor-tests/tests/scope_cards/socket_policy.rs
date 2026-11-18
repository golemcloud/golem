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

//! The address policy of agent sockets, on P2 and P3: no listening, limited binds, D28 for
//! network grants, and the destinations that no grant reaches. The peers count accepted
//! connections, so a refusal is checked at the peer as well as in the guest's error.

use super::*;
use golem_common::model::component::ComponentDto;
use std::sync::atomic::AtomicUsize;
use test_r::{test, timeout};

const API: [&str; 2] = ["p2", "p3"];

/// A TCP peer that counts the connections it accepts.
struct CountingPeer {
    address: std::net::SocketAddr,
    accepted: Arc<AtomicUsize>,
    _task: tokio_util::task::AbortOnDropHandle<()>,
}

impl CountingPeer {
    async fn start(address: &str) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(address).await?;
        let address = listener.local_addr()?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = stream.shutdown().await;
            }
        });
        Ok(Self {
            address,
            accepted,
            _task: tokio_util::task::AbortOnDropHandle::new(task),
        })
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

fn free_loopback_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

fn network_permission(resource: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("network() @ * : connect : {resource}"))
        .expect("valid test network permission")
}

async fn networking_component(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    host_api_tests: &PrecompiledComponent,
    grants: &[String],
) -> anyhow::Result<ComponentDto> {
    let grants = grants
        .iter()
        .map(|grant| network_permission(grant))
        .collect::<Vec<_>>();
    executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .without_default_host_permissions("Networking")
        .update_agent_provision_config("Networking", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(grants);
        })
        .store()
        .await
}

async fn socket_operation(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &golem_common::model::agent::ParsedAgentKey,
    api: &str,
    operation: &str,
    address: &str,
) -> anyhow::Result<Result<String, String>> {
    executor
        .invoke_and_await_agent(
            component,
            agent,
            &format!("socket_policy_{api}"),
            data_value!(operation, address),
        )
        .await?
        .into_typed::<Result<String, String>>()
}

fn assert_denied(result: &Result<String, String>, what: &str) {
    assert!(
        result
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
        "{what} must fail with access-denied, got {result:?}"
    );
}

fn assert_not_denied(result: &Result<String, String>, what: &str) {
    assert!(
        !result
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
        "{what} must not fail with access-denied, got {result:?}"
    );
}

#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn agent_sockets_never_listen_and_bind_only_port_zero_or_loopback(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = networking_component(&executor, &context, host_api_tests, &[]).await?;
    let agent = agent_key!("Networking", "socket-policy-binds");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    for api in API {
        let loopback = format!("127.0.0.1:{}", free_loopback_port()?);
        let listen =
            socket_operation(&executor, &component, &agent, api, "tcp-listen", &loopback).await?;
        assert_denied(&listen, &format!("{api} TCP listen"));
        for operation in ["tcp-bind", "udp-bind"] {
            for address in ["0.0.0.0:8080", "[::]:8080"] {
                let bound =
                    socket_operation(&executor, &component, &agent, api, operation, address)
                        .await?;
                assert_denied(&bound, &format!("{api} {operation} {address}"));
            }
            let loopback = format!("127.0.0.1:{}", free_loopback_port()?);
            for address in ["0.0.0.0:0", loopback.as_str()] {
                let bound =
                    socket_operation(&executor, &component, &agent, api, operation, address)
                        .await?;
                assert_eq!(
                    bound,
                    Ok("bound".to_string()),
                    "{api} {operation} {address} must bind"
                );
            }
        }
    }
    let unbound = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-listen-unbound",
        "0.0.0.0:0",
    )
    .await?;
    assert_denied(&unbound, "P3 TCP listen with an implicit bind");

    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}

#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn wildcard_network_grants_do_not_reach_internal_socket_destinations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let peer = CountingPeer::start("127.0.0.1:0").await?;
    let component =
        networking_component(&executor, &context, host_api_tests, &["*".to_string()]).await?;
    let agent = agent_key!("Networking", "socket-policy-wildcard");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let peer_address = peer.address.to_string();
    let destinations = [
        peer_address.as_str(),
        "10.0.0.1:443",
        "169.254.169.254:80",
        "[::1]:443",
    ];
    for api in API {
        for operation in ["tcp-connect", "udp-send"] {
            for destination in destinations {
                let result =
                    socket_operation(&executor, &component, &agent, api, operation, destination)
                        .await?;
                assert_denied(
                    &result,
                    &format!("{api} {operation} {destination} under a wildcard grant"),
                );
            }
        }
    }
    assert_eq!(peer.accepted(), 0);

    // Replay returns the recorded denials: the agent recovers and runs again, and the peer
    // still has seen no connection.
    executor.simulated_crash(&worker).await?;
    let after_crash = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-connect",
        &peer_address,
    )
    .await?;
    assert_denied(&after_crash, "P3 connect after the crash");
    assert_eq!(peer.accepted(), 0);
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}

#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn named_network_grants_reach_internal_socket_destinations_except_refused_ones(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let peer = CountingPeer::start("127.0.0.1:0").await?;
    let ipv6_peer = CountingPeer::start("[::1]:0").await?;
    let grpc_port = executor.grpc_port();
    let grants = vec![
        "*".to_string(),
        peer.address.to_string(),
        ipv6_peer.address.to_string(),
        "10.0.0.1:443".to_string(),
        "169.254.169.254:80".to_string(),
        "169.254.1.1:5432".to_string(),
        "[fe80::1]:5432".to_string(),
        format!("127.0.0.1:{grpc_port}"),
        format!("[::1]:{grpc_port}"),
    ];
    let component = networking_component(&executor, &context, host_api_tests, &grants).await?;
    let agent = agent_key!("Networking", "socket-policy-named");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let peer_address = peer.address.to_string();
    let ipv6_peer_address = ipv6_peer.address.to_string();
    for api in API {
        for destination in [&peer_address, &ipv6_peer_address] {
            let connected = socket_operation(
                &executor,
                &component,
                &agent,
                api,
                "tcp-connect",
                destination,
            )
            .await?;
            assert_eq!(
                connected,
                Ok("connected".to_string()),
                "{api} connect to the granted {destination}"
            );
        }
        // An IPv4 grant does not reach the IPv6 loopback, nor another loopback port.
        for destination in [
            format!("[::1]:{}", peer.address.port()),
            format!("127.0.0.1:{}", ipv6_peer.address.port()),
        ] {
            let denied = socket_operation(
                &executor,
                &component,
                &agent,
                api,
                "tcp-connect",
                &destination,
            )
            .await?;
            assert_denied(
                &denied,
                &format!("{api} connect to the ungranted {destination}"),
            );
        }
        for operation in ["tcp-connect", "udp-send"] {
            for destination in [
                "169.254.169.254:80".to_string(),
                "169.254.1.1:5432".to_string(),
                "[fe80::1]:5432".to_string(),
                format!("127.0.0.1:{grpc_port}"),
                format!("[::1]:{grpc_port}"),
            ] {
                let refused =
                    socket_operation(&executor, &component, &agent, api, operation, &destination)
                        .await?;
                assert_denied(
                    &refused,
                    &format!("{api} {operation} {destination} against a grant that names it"),
                );
            }
        }
        // UDP sends without a peer, so the named private address shows the grant check passing
        // without depending on a route to it.
        let private = socket_operation(
            &executor,
            &component,
            &agent,
            api,
            "udp-send",
            "10.0.0.1:443",
        )
        .await?;
        assert_not_denied(
            &private,
            &format!("{api} UDP send to the granted 10.0.0.1:443"),
        );
    }
    assert_eq!(peer.accepted(), 2);
    assert_eq!(ipv6_peer.accepted(), 2);

    // A read-only method cannot use sockets, even with a grant.
    let read_only = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "read_only_tcp_connect_p3",
            data_value!(peer_address.clone()),
        )
        .await;
    let error = read_only.expect_err("a socket connect from a read-only method must be refused");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("ReadOnlyViolation"),
        "expected a read-only violation, got {rendered}"
    );
    assert_eq!(peer.accepted(), 2);

    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}
