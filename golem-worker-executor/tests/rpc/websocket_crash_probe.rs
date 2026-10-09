use super::*;
use futures::{SinkExt, StreamExt};
use golem_test_framework::dsl::count_agent_invocation_pair_since;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio_tungstenite::tungstenite::Message;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_effect_caller")]
    PrecompiledComponent
);

struct Peer {
    port: u16,
    writes: mpsc::UnboundedReceiver<Vec<u8>>,
    tasks: tokio::task::JoinSet<anyhow::Result<()>>,
}

async fn start_peer() -> anyhow::Result<Peer> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (writes_tx, writes) = mpsc::unbounded_channel();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (socket, _) = listener.accept().await?;
            let writes_tx = writes_tx.clone();
            connections.spawn(async move {
                let mut first = false;
                let mut socket = tokio_tungstenite::accept_hdr_async(
                    socket,
                    |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                     response| {
                        first = request.uri().path() == "/first";
                        Ok(response)
                    },
                )
                .await?;
                if first {
                    socket.send(Message::Text("Connected".into())).await?;
                }
                while let Some(message) = socket.next().await {
                    match message {
                        Ok(Message::Binary(bytes)) => {
                            writes_tx.send(bytes.to_vec())?;
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
                Ok::<_, anyhow::Error>(())
            });
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    Ok(Peer {
        port,
        writes,
        tasks,
    })
}

fn has_terminal(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> bool {
    entries.iter().any(|entry| match &entry.entry {
        PublicOplogEntry::End(end) => end.start_index == start,
        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
        _ => false,
    })
}

#[test]
#[timeout("60s")]
async fn effect_duplex_websocket_crash_continuation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_effect_caller")] effect: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, effect)
        .store()
        .await?;

    // The same duplex session must continue both with and without reconstruction.
    for crash in [false, true] {
        let mut peer = start_peer().await?;
        let worker = executor
            .start_agent(
                &component.id,
                agent_id!(
                    "ConcurrentStreamProbe",
                    format!("duplex-recovery-{crash}"),
                    2_f64,
                    peer.port as f64
                ),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
                if entries.iter().any(|entry| {
                    matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
                }) {
                    return anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        let boundary = executor.oplog_max_index(&worker).await?;
        let metadata = executor.get_worker_metadata(&worker).await?;
        let start = InvocationStart {
            agent_id: Some(worker.clone().into()),
            method_name: Some("echo".to_string()),
            input: Some(golem_schema::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::RecordValue(RecordValue {
                    fields: vec![golem_schema::proto::golem::schema::SchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id: 1 },
                        )),
                    }],
                })),
            }),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        };
        let (requests, receiver) = mpsc::channel(8);
        requests
            .send(InvocationRequest {
                request: Some(invocation_request::Request::Start(start)),
            })
            .await?;
        let mut responses = executor
            .client
            .clone()
            .invoke_agent_session(ReceiverStream::new(receiver))
            .await?
            .into_inner();
        let accepted = tokio::time::timeout(Duration::from_secs(10), responses.message())
            .await??
            .expect("acceptance");
        let Some(invocation_response::Response::Accepted(accepted)) = accepted.response else {
            anyhow::bail!("missing acceptance");
        };
        let stream_id = accepted.stream_mappings[0]
            .handle
            .as_ref()
            .and_then(|h| h.stream_id);
        let input = |sequence, text: &str| InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: 1,
                sequence,
                payload: Some(input_stream_item::Payload::Value(
                    SchemaValue::String(text.to_string()).try_into().unwrap(),
                )),
                durable_stream_id: stream_id,
                epoch: accepted.epoch,
            })),
        };
        requests.send(input(0, "hello")).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let response = responses.message().await?.expect("first output");
                if let Some(invocation_response::Response::OutputItem(item)) = response.response {
                    assert_eq!(
                        item.value.unwrap().value,
                        Some(schema_value::Value::StringValue("hello".to_string()))
                    );
                    break;
                }
            }
            assert_eq!(peer.writes.recv().await.unwrap(), b"hello");
            // Crash only after the internal clock End and a completed receive are durable.
            // The other websocket reader remains blocked on an open peer with no input.
            loop {
                executor.commit_oplog(&worker).await?;
                let entries = executor.get_oplog(&worker, boundary.next()).await?;
                let completed_internal_clock = entries.iter().any(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::Start(start)
                        if start.function_name.ends_with("monotonic-clock::now")
                            && start.parent_start_index.is_some()
                            && has_terminal(&entries, entry.oplog_index))
                });
                let completed_receive = entries.iter().any(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::Start(start)
                        if start.function_name.ends_with("client::receive")
                            && has_terminal(&entries, entry.oplog_index))
                });
                if completed_internal_clock && completed_receive {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            anyhow::Ok(())
        })
        .await??;
        if crash {
            executor.simulated_crash(&worker).await?;
        }
        requests.send(input(1, "after")).await?;
        requests
            .send(InvocationRequest {
                request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                    transport_stream_id: 1,
                    sequence: 2,
                    durable_stream_id: stream_id,
                    epoch: accepted.epoch,
                })),
            })
            .await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut values = Vec::new();
            let mut closed = false;
            loop {
                let response = responses.message().await?.expect("session completion");
                match response.response {
                    Some(invocation_response::Response::OutputItem(item)) => {
                        values.push(item.value.unwrap().value)
                    }
                    Some(invocation_response::Response::OutputEnd(_)) => closed = true,
                    Some(invocation_response::Response::Finished(completion)) => {
                        assert!(
                            matches!(
                                completion.outcome,
                                Some(invocation_session_completion::Outcome::Success(_))
                            ),
                            "{completion:?}"
                        );
                        break;
                    }
                    _ => {}
                }
            }
            assert!(closed, "output stream must end before Finished");
            assert_eq!(
                values,
                vec![Some(schema_value::Value::StringValue("after".to_string()))]
            );
            assert_eq!(peer.writes.recv().await.unwrap(), b"after");
            anyhow::Ok(())
        })
        .await??;
        let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&entries, boundary),
            (1, 1)
        );
        for entry in &entries {
            if let PublicOplogEntry::Start(start) = &entry.entry {
                assert!(
                    has_terminal(&entries, entry.oplog_index),
                    "unsettled Start at {}",
                    entry.oplog_index
                );
                if start.function_name.ends_with("monotonic-clock::now")
                    && start.parent_start_index.is_some()
                {
                    assert!(
                        entries.iter().all(|terminal| match &terminal.entry {
                            PublicOplogEntry::CompletionDelivered(delivered) =>
                                delivered.start_index != entry.oplog_index,
                            PublicOplogEntry::CompletionDiscarded(discarded) =>
                                discarded.start_index != entry.oplog_index,
                            _ => true,
                        }),
                        "internal clock read must not have a guest delivery marker"
                    );
                }
            }
        }
        peer.tasks.abort_all();
        while peer.tasks.join_next().await.is_some() {}
    }
    Ok(())
}
