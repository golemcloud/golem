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
use hickory_resolver::proto::rr::rdata::{A, AAAA, CAA, CNAME, MX, NAPTR, SOA, SRV, TXT};
use hickory_resolver::proto::rr::{Name, RData, Record, RecordType};
use std::sync::Mutex;
use test_r::{test, timeout};

/// A UDP DNS server for the `test.` zone. Every answer has a TTL of 300 seconds.
///
/// - `db.test` has an A record for `127.0.0.1`, an MX and a TXT record.
/// - `v6.test` has only an AAAA record for `::1`.
/// - `alias.test` is a CNAME for `db.test`, so its A answer also holds that CNAME record.
/// - `records.test` has an SRV, a CAA, an SOA and a NAPTR record.
/// - `flaky.test` fails with SERVFAIL until [`DnsPeer::heal`], then has an A record for
///   `127.0.0.1`.
///
/// Every other name does not exist.
struct DnsPeer {
    address: std::net::SocketAddr,
    queries: Arc<Mutex<HashMap<(String, RecordType), usize>>>,
    healed: Arc<AtomicBool>,
    _task: tokio_util::task::AbortOnDropHandle<()>,
}

fn test_name(name: &str) -> Name {
    Name::from_ascii(name).unwrap()
}

/// The answer records of `name` for `record_type`, or the response code of a failure.
fn answer(name: &str, record_type: RecordType, healed: bool) -> Result<Vec<Record>, ResponseCode> {
    let record = |owner: &str, data: RData| Record::from_rdata(test_name(owner), 300, data);
    let data = match (name, record_type) {
        ("db.test", RecordType::A) => RData::A(A::new(127, 0, 0, 1)),
        ("db.test", RecordType::MX) => RData::MX(MX::new(10, test_name("mail.db.test."))),
        ("db.test", RecordType::TXT) => {
            RData::TXT(TXT::new(vec!["v=spf1 ".to_string(), "-all".to_string()]))
        }
        ("v6.test", RecordType::AAAA) => RData::AAAA(AAAA::new(0, 0, 0, 0, 0, 0, 0, 1)),
        ("alias.test", RecordType::CNAME) => RData::CNAME(CNAME(test_name("db.test."))),
        ("alias.test", RecordType::A) => {
            return Ok(vec![
                record("alias.test.", RData::CNAME(CNAME(test_name("db.test.")))),
                record("db.test.", RData::A(A::new(127, 0, 0, 1))),
            ]);
        }
        ("records.test", RecordType::SRV) => {
            RData::SRV(SRV::new(10, 5, 8080, test_name("db.test.")))
        }
        ("records.test", RecordType::CAA) => {
            RData::CAA(CAA::new_issue(true, Some(test_name("ca.test")), vec![]))
        }
        ("records.test", RecordType::SOA) => RData::SOA(SOA::new(
            test_name("ns.test."),
            test_name("admin.test."),
            2026111801,
            3600,
            600,
            86400,
            60,
        )),
        ("records.test", RecordType::NAPTR) => RData::NAPTR(NAPTR::new(
            100,
            10,
            b"S".to_vec().into_boxed_slice(),
            b"SIP+D2U".to_vec().into_boxed_slice(),
            b"".to_vec().into_boxed_slice(),
            test_name("_sip._udp.records.test."),
        )),
        ("flaky.test", _) if !healed => return Err(ResponseCode::ServFail),
        ("flaky.test", RecordType::A) => RData::A(A::new(127, 0, 0, 1)),
        ("db.test" | "v6.test" | "alias.test" | "records.test" | "flaky.test", _) => {
            return Ok(vec![]);
        }
        _ => return Err(ResponseCode::NXDomain),
    };
    Ok(vec![record(&format!("{name}."), data)])
}

impl DnsPeer {
    async fn start() -> anyhow::Result<Self> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let queries = Arc::new(Mutex::new(HashMap::new()));
        let counter = queries.clone();
        let healed = Arc::new(AtomicBool::new(false));
        let is_healed = healed.clone();
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
                match answer(&name, query.query_type(), is_healed.load(Ordering::SeqCst)) {
                    Ok(records) => {
                        for record in records {
                            response.add_answer(record);
                        }
                    }
                    Err(code) => response.metadata.response_code = code,
                }
                if let Ok(bytes) = response.to_vec() {
                    let _ = socket.send_to(&bytes, from).await;
                }
            }
        });
        Ok(Self {
            address,
            queries,
            healed,
            _task: tokio_util::task::AbortOnDropHandle::new(task),
        })
    }

    /// Makes `flaky.test` answer from now on.
    fn heal(&self) {
        self.healed.store(true, Ordering::SeqCst);
    }

    fn queries(&self) -> HashMap<(String, RecordType), usize> {
        self.queries.lock().unwrap().clone()
    }
}

/// An executor with the scope-card service of `authority` that resolves `golem:network/dns`
/// lookups with `dns`, with `retry` as its retry configuration when given.
async fn start_dns_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: &Arc<ScopeCardAuthority>,
    dns: &DnsPeer,
    retry: Option<golem_common::model::RetryConfig>,
    agent_sockets: bool,
) -> anyhow::Result<TestWorkerExecutor> {
    let card_authority = authority.clone();
    let dns_address = dns.address;
    let overrides = TestExecutorOverrides {
        create_card_service: Some(Arc::new(move || {
            Arc::new(ScopeCardService {
                authority: card_authority.clone(),
            })
        })),
        configure: Some(Arc::new(move |config| {
            config.agent_sockets.dns_servers = vec![dns_address];
            if let Some(retry) = &retry {
                config.retry = retry.clone();
            }
        })),
        ..TestExecutorOverrides::default()
    };
    let overrides = if agent_sockets {
        overrides.with_agent_sockets()
    } else {
        overrides
    };
    start_with_overrides(deps, context, overrides).await
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
    let executor = start_dns_executor(deps, &context, &authority, &dns, None, true).await?;
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
    let executor = start_dns_executor(deps, &context, &authority, &dns, None, true).await?;
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

/// An AAAA answer links its addresses to the queried name, as an A answer does, and the A answer
/// of an alias links to the alias, not to the name the alias points to. Each record type reaches
/// the guest with its fields, and an existing name without records of the type is `no-data`.
#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn aaaa_and_alias_answers_link_the_queried_name_and_record_types_convert(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let dns = DnsPeer::start().await?;
    let executor = start_dns_executor(deps, &context, &authority, &dns, None, true).await?;
    let ipv4_peer = CountingPeer::start("127.0.0.1:0").await?;
    let ipv6_peer = CountingPeer::start("[::1]:0").await?;
    let ipv4_address = ipv4_peer.address.to_string();
    let ipv6_address = ipv6_peer.address.to_string();
    // `db.test` is not granted, so only a link to `alias.test` reaches `127.0.0.1`.
    let component = networking_component(
        &executor,
        &context,
        host_api_tests,
        &[
            "v6.test".to_string(),
            "alias.test".to_string(),
            "records.test".to_string(),
        ],
    )
    .await?;
    let agent = agent_id!("Networking", "dns-record-types");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let (executor_ref, component_ref, agent_ref) = (&executor, &component, &agent);
    let connect = |address: String| async move {
        socket_operation(
            executor_ref,
            component_ref,
            agent_ref,
            "p3",
            "tcp-connect",
            &address,
        )
        .await
    };

    assert_denied(
        &connect(ipv6_address.clone()).await?,
        "connect before the AAAA lookup",
    );
    assert_denied(
        &connect(ipv4_address.clone()).await?,
        "connect before the alias lookup",
    );

    let no_data = dns_resolve(&executor, &component, &agent, "v6.test", "a").await?;
    assert!(
        no_data
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "nodata")),
        "an existing name without A records must fail with no-data, got {no_data:?}"
    );
    let aaaa = dns_resolve(&executor, &component, &agent, "v6.test", "aaaa").await?;
    assert_eq!(aaaa, Ok(vec!["300 ::1".to_string()]));
    assert_eq!(
        connect(ipv6_address.clone()).await?,
        Ok("connected".to_string())
    );
    assert_denied(
        &connect(ipv4_address.clone()).await?,
        "connect to 127.0.0.1 after the AAAA lookup of v6.test",
    );

    // The CNAME record that leads from the alias to `db.test` is not part of the A answer.
    let alias = dns_resolve(&executor, &component, &agent, "alias.test", "a").await?;
    assert_eq!(alias, Ok(vec!["300 127.0.0.1".to_string()]));
    assert_eq!(
        connect(ipv4_address.clone()).await?,
        Ok("connected".to_string())
    );
    let cname = dns_resolve(&executor, &component, &agent, "alias.test", "cname").await?;
    assert_eq!(cname, Ok(vec!["300 db.test".to_string()]));

    let mut converted = Vec::new();
    for record_type in ["srv", "caa", "soa", "naptr"] {
        converted
            .push(dns_resolve(&executor, &component, &agent, "records.test", record_type).await?);
    }
    assert_eq!(
        converted,
        vec![
            Ok(vec!["300 10 5 8080 db.test".to_string()]),
            Ok(vec!["300 true issue ca.test".to_string()]),
            Ok(vec![
                "300 ns.test admin.test 2026111801 3600 600 86400 60".to_string()
            ]),
            Ok(vec![
                "300 100 10 S SIP+D2U  _sip._udp.records.test".to_string()
            ]),
        ]
    );

    let expected_queries = HashMap::from([
        (("v6.test".to_string(), RecordType::A), 1),
        (("v6.test".to_string(), RecordType::AAAA), 1),
        (("alias.test".to_string(), RecordType::A), 1),
        (("alias.test".to_string(), RecordType::CNAME), 1),
        (("records.test".to_string(), RecordType::SRV), 1),
        (("records.test".to_string(), RecordType::CAA), 1),
        (("records.test".to_string(), RecordType::SOA), 1),
        (("records.test".to_string(), RecordType::NAPTR), 1),
    ]);
    assert_eq!(dns.queries(), expected_queries);

    // Replay links both answers again without a query.
    executor.simulated_crash(&worker).await?;
    assert_eq!(
        connect(ipv6_address.clone()).await?,
        Ok("connected".to_string())
    );
    assert_eq!(
        connect(ipv4_address.clone()).await?,
        Ok("connected".to_string())
    );
    assert_eq!(dns.queries(), expected_queries);
    assert_eq!((ipv4_peer.accepted(), ipv6_peer.accepted()), (2, 2));
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}

/// A server failure retries under the retry policy until the server answers, and the guest gets
/// the answer; replay returns it without a query.
#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn a_server_failure_retries_until_the_server_answers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let dns = DnsPeer::start().await?;
    let retry = golem_common::model::RetryConfig {
        max_attempts: 1000,
        min_delay: Duration::from_millis(20),
        max_delay: Duration::from_millis(20),
        multiplier: 1.0,
        max_jitter_factor: None,
    };
    let executor = start_dns_executor(deps, &context, &authority, &dns, Some(retry), true).await?;
    let peer = CountingPeer::start("127.0.0.1:0").await?;
    let component = networking_component(
        &executor,
        &context,
        host_api_tests,
        &["flaky.test".to_string()],
    )
    .await?;
    let agent = agent_id!("Networking", "dns-retry");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let retries = || async {
        let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        Ok::<_, anyhow::Error>(
            oplog
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
                .count(),
        )
    };

    // One resolver lookup sends a few queries at most, so ten failed queries mean the lookup was
    // retried.
    let flaky_queries = || {
        dns.queries()
            .get(&("flaky.test".to_string(), RecordType::A))
            .copied()
            .unwrap_or(0)
    };
    let lookup = dns_resolve(&executor, &component, &agent, "flaky.test", "a");
    let heal = async {
        tokio::time::timeout(Duration::from_secs(60), async {
            while flaky_queries() < 10 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        dns.heal();
        Ok::<_, anyhow::Error>(())
    };
    let (answer, healed) = tokio::join!(lookup, heal);
    healed?;
    assert_eq!(answer?, Ok(vec!["300 127.0.0.1".to_string()]));
    let queries = flaky_queries();
    let recorded_retries = retries().await?;
    assert!(recorded_retries > 0, "the oplog must record the retries");

    executor.simulated_crash(&worker).await?;
    let connect = socket_operation(
        &executor,
        &component,
        &agent,
        "p3",
        "tcp-connect",
        &peer.address.to_string(),
    )
    .await?;
    assert_eq!(
        connect,
        Ok("connected".to_string()),
        "connect after the crash"
    );
    assert_eq!(
        dns.queries(),
        HashMap::from([(("flaky.test".to_string(), RecordType::A), queries)])
    );
    assert_eq!(retries().await?, recorded_retries);
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}

/// With agent sockets disabled, a lookup fails with `permanent-resolver-failure` and queries
/// nothing.
#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn lookups_fail_without_a_query_when_agent_sockets_are_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let dns = DnsPeer::start().await?;
    let executor = start_dns_executor(deps, &context, &authority, &dns, None, false).await?;
    let component = networking_component(
        &executor,
        &context,
        host_api_tests,
        &["db.test".to_string()],
    )
    .await?;
    let agent = agent_id!("Networking", "dns-disabled");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let lookup = dns_resolve(&executor, &component, &agent, "db.test", "a").await?;
    assert!(
        lookup
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "permanentresolverfailure")),
        "a lookup with agent sockets disabled must fail with permanent-resolver-failure, got {lookup:?}"
    );
    assert_eq!(dns.queries(), HashMap::new());
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}
