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

//! `golem:network/dns` record lookups against an in-test DNS server that counts the queries it
//! answers, so a replayed lookup is checked at the server.

use super::socket_policy::{CountingPeer, assert_denied, networking_component, socket_operation};
use super::*;
use golem_test_framework::dsl::WorkerLogEventStream;
use hickory_resolver::proto::op::{Message, MessageType, ResponseCode};
use hickory_resolver::proto::rr::rdata::{A, MX, TXT};
use hickory_resolver::proto::rr::{Name, RData, Record, RecordType};
use std::sync::Mutex;
use test_r::{test, timeout};

/// A UDP DNS server for the `test.` zone. `db.test` has an A record for `127.0.0.1`, an MX and a
/// TXT record; every other name does not exist.
struct DnsPeer {
    address: std::net::SocketAddr,
    queries: Arc<Mutex<HashMap<(String, RecordType), usize>>>,
    _task: tokio_util::task::AbortOnDropHandle<()>,
}

impl DnsPeer {
    async fn start() -> anyhow::Result<Self> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let queries = Arc::new(Mutex::new(HashMap::new()));
        let counter = queries.clone();
        let task = tokio::spawn(async move {
            let mut buffer = vec![0u8; 4096];
            while let Ok((length, from)) = socket.recv_from(&mut buffer).await {
                let Ok(request) = Message::from_vec(&buffer[..length]) else {
                    continue;
                };
                let Some(query) = request.queries.first().cloned() else {
                    continue;
                };
                let name = query.name().to_ascii().trim_end_matches('.').to_string();
                *counter
                    .lock()
                    .unwrap()
                    .entry((name.clone(), query.query_type()))
                    .or_insert(0) += 1;
                let mut response = Message::new(
                    request.metadata.id,
                    MessageType::Response,
                    request.metadata.op_code,
                );
                response.metadata.recursion_desired = request.metadata.recursion_desired;
                response.metadata.recursion_available = true;
                response.add_query(query.clone());
                let owner = query.name().clone();
                if name == "db.test" {
                    let data = match query.query_type() {
                        RecordType::A => Some(RData::A(A::new(127, 0, 0, 1))),
                        RecordType::MX => Some(RData::MX(MX::new(
                            10,
                            Name::from_ascii("mail.db.test.").unwrap(),
                        ))),
                        RecordType::TXT => Some(RData::TXT(TXT::new(vec![
                            "v=spf1 ".to_string(),
                            "-all".to_string(),
                        ]))),
                        _ => None,
                    };
                    if let Some(data) = data {
                        response.add_answer(Record::from_rdata(owner, 300, data));
                    }
                } else {
                    response.metadata.response_code = ResponseCode::NXDomain;
                }
                if let Ok(bytes) = response.to_vec() {
                    let _ = socket.send_to(&bytes, from).await;
                }
            }
        });
        Ok(Self {
            address,
            queries,
            _task: tokio_util::task::AbortOnDropHandle::new(task),
        })
    }

    fn queries(&self) -> HashMap<(String, RecordType), usize> {
        self.queries.lock().unwrap().clone()
    }
}

async fn dns_resolve(
    executor: &TestWorkerExecutor,
    component: &golem_common::model::component::ComponentDto,
    agent: &golem_common::model::agent::ParsedAgentKey,
    name: &str,
    record_type: &str,
) -> anyhow::Result<Result<Vec<String>, String>> {
    executor
        .invoke_and_await_agent(
            component,
            agent,
            "dns_resolve",
            data_value!(name, record_type),
        )
        .await?
        .into_typed::<Result<Vec<String>, String>>()
}

#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn dns_record_lookups_replay_without_queries_and_link_a_answers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let dns = DnsPeer::start().await?;
    let dns_address = dns.address;
    let card_authority = authority.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new(move || {
                Arc::new(ScopeCardService {
                    authority: card_authority.clone(),
                })
            })),
            configure: Some(Arc::new(move |config| {
                config.agent_sockets.dns_servers = vec![dns_address];
            })),
            ..TestExecutorOverrides::default()
        }
        .with_agent_sockets(),
    )
    .await?;
    let peer = CountingPeer::start("127.0.0.1:0").await?;
    let peer_address = peer.address.to_string();
    let component = networking_component(
        &executor,
        &context,
        host_api_tests,
        &["db.test".to_string(), "missing.test".to_string()],
    )
    .await?;
    let agent = agent_id!("Networking", "dns-records");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    // Without a lookup, the hostname grant does not reach the address.
    let before = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-connect",
        &peer_address,
    )
    .await?;
    assert_denied(&before, "connect before the A lookup");

    // An MX answer links nothing.
    let mx = dns_resolve(&executor, &component, &agent, "db.test", "mx").await?;
    assert_eq!(mx, Ok(vec!["300 10 mail.db.test".to_string()]));
    let after_mx = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-connect",
        &peer_address,
    )
    .await?;
    assert_denied(&after_mx, "connect after the MX lookup");

    let a = dns_resolve(&executor, &component, &agent, "db.test", "a").await?;
    assert_eq!(a, Ok(vec!["300 127.0.0.1".to_string()]));
    let txt = dns_resolve(&executor, &component, &agent, "db.test", "txt").await?;
    assert_eq!(txt, Ok(vec!["300 v=spf1 -all".to_string()]));
    let missing = dns_resolve(&executor, &component, &agent, "missing.test", "a").await?;
    assert!(
        missing
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "namenotfound")),
        "NXDOMAIN must fail with name-not-found, got {missing:?}"
    );
    let ungranted = dns_resolve(&executor, &component, &agent, "other.test", "a").await?;
    assert!(
        ungranted
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
        "a lookup without a grant must fail with access-denied, got {ungranted:?}"
    );
    for api in ["p2", "p3"] {
        let linked = socket_operation(
            &executor,
            &component,
            &agent,
            api,
            "tcp-connect",
            &peer_address,
        )
        .await?;
        assert_eq!(
            linked,
            Ok("connected".to_string()),
            "{api} connect after the A lookup"
        );
    }

    let expected_queries = HashMap::from([
        (("db.test".to_string(), RecordType::MX), 1),
        (("db.test".to_string(), RecordType::A), 1),
        (("db.test".to_string(), RecordType::TXT), 1),
        (("missing.test".to_string(), RecordType::A), 1),
    ]);
    assert_eq!(dns.queries(), expected_queries);

    // Replay returns every recorded answer, the negative ones too, without a query, and the A
    // answer links again.
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
    assert_eq!(
        after_crash,
        Ok("connected".to_string()),
        "connect after the crash"
    );
    assert_eq!(dns.queries(), expected_queries);
    assert_eq!(peer.accepted(), 3);
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}

/// An A answer that the guest drops unread links nothing, live and after a restart; the same
/// answer, once read, links.
#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn a_discarded_a_answer_links_nothing_live_or_on_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let dns = DnsPeer::start().await?;
    let dns_address = dns.address;
    let card_authority = authority.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new(move || {
                Arc::new(ScopeCardService {
                    authority: card_authority.clone(),
                })
            })),
            configure: Some(Arc::new(move |config| {
                config.agent_sockets.dns_servers = vec![dns_address];
            })),
            ..TestExecutorOverrides::default()
        }
        .with_agent_sockets(),
    )
    .await?;
    let peer = CountingPeer::start("127.0.0.1:0").await?;
    let peer_address = peer.address.to_string();
    let component = networking_component(
        &executor,
        &context,
        host_api_tests,
        &["db.test".to_string()],
    )
    .await?;
    let agent = agent_id!("Networking", "dns-discarded");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let cancel_value = executor
        .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected a promise"))?;
    let cancel = PromiseId::from_value(&cancel_value)?;

    // The lookup's End is recorded, but its completion stays unpublished until the guest has
    // dropped the lookup, so the guest discards an answer that the host call returned.
    let mut end = executor.gate_next_call_end(&worker, true).await;
    let mut output = executor.make_worker_log_event_stream(&worker).await?;
    let key = IdempotencyKey::fresh();
    let params = || {
        crate::raw_params(vec![
            SchemaValue::String("db.test".to_string()),
            cancel_value.clone(),
        ])
    };
    executor
        .invoke_agent_with_key(&component, &agent, &key, "dns_discard_a", params())
        .await?;
    tokio::time::timeout(Duration::from_secs(10), end.entered()).await?;
    let lookup_start = end.start_index();
    executor.complete_promise(&cancel, vec![1]).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = output.message().await? {
            if golem_test_framework::dsl::stdout_event_matching(&event, "dns lookup discarded\n") {
                return Ok::<_, anyhow::Error>(());
            }
        }
        Err(anyhow::anyhow!(
            "guest output ended before the lookup was discarded"
        ))
    })
    .await??;
    drop(end);
    let pending = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "dns_discard_a", params())
        .await?
        .into_typed::<bool>()?;
    assert!(pending, "the guest must not have read the answer");

    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let marks = |matches: fn(&PublicOplogEntry, OplogIndex) -> bool| {
        oplog
            .iter()
            .filter(|entry| matches(&entry.entry, lookup_start))
            .count()
    };
    assert_eq!(
        marks(|entry, start| matches!(entry, PublicOplogEntry::End(e) if e.start_index == start)),
        1
    );
    assert_eq!(
        marks(
            |entry, start| matches!(entry, PublicOplogEntry::CompletionDiscarded(e) if e.start_index == start)
        ),
        1
    );
    assert_eq!(
        marks(
            |entry, start| matches!(entry, PublicOplogEntry::CompletionDelivered(e) if e.start_index == start)
        ),
        0
    );
    assert_eq!(
        dns.queries(),
        HashMap::from([(("db.test".to_string(), RecordType::A), 1)])
    );

    for api in ["p2", "p3"] {
        let connect = socket_operation(
            &executor,
            &component,
            &agent,
            api,
            "tcp-connect",
            &peer_address,
        )
        .await?;
        assert_denied(
            &connect,
            &format!("{api} connect after the discarded lookup"),
        );
    }
    executor.simulated_crash(&worker).await?;
    let replayed = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-connect",
        &peer_address,
    )
    .await?;
    assert_denied(&replayed, "connect after replaying the discarded lookup");

    // The same answer, once the guest reads it, links the address.
    let a = dns_resolve(&executor, &component, &agent, "db.test", "a").await?;
    assert_eq!(a, Ok(vec!["300 127.0.0.1".to_string()]));
    let linked = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-connect",
        &peer_address,
    )
    .await?;
    assert_eq!(linked, Ok("connected".to_string()));
    assert_eq!(peer.accepted(), 1);
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}
