use capable_streaming_tool_guest_client::CapableStreamingClient;
use futures_concurrency::prelude::*;
use golem_rust::agentic::{
    InputStream, Principal, ToolInvocation, pump_tool_stdin, spawn_local, tool_protocol_error,
};
use golem_rust::durability::{Durability, DurableFunctionType};
use golem_rust::golem_agentic::golem::tool::host::{
    self as tool_host, ByteStreamCloseCause, ByteStreamFailure, RpcError, ToolRpc,
};
use golem_rust::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, agent_definition, agent_implementation,
};
use std::io::Write;
use streaming_tool_guest_client::{StreamSummary, StreamingClient, StreamingRunError};

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub struct StreamEvidence {
    pub output: Vec<u8>,
    pub chunks_read: u32,
    pub bytes_read: u64,
    pub output_closed: bool,
    pub completion: String,
}

#[derive(IntoSchema)]
struct RawRunInput {
    mode: String,
}

#[derive(IntoSchema)]
struct RawNoStreamInput {
    value: String,
}

#[derive(IntoSchema)]
struct RawOptionalStreamsInput {}

#[derive(IntoSchema)]
struct RawCapableInput {
    path: String,
}

#[agent_definition]
pub trait ToolStreamingCaller {
    fn new(name: String) -> Self;

    async fn concurrent_attempt_identity_replay(&self) -> Vec<String>;
    async fn marker_before_eof(&self, first: Vec<u8>, rest: Vec<u8>) -> StreamEvidence;
    async fn alternating_echo(&self, chunk_count: u32, chunk_size: u32) -> StreamEvidence;
    async fn collect(&self, mode: String, input: Vec<u8>, fragment_size: u32) -> StreamEvidence;
    async fn result_before_stdout(&self, mode: String) -> StreamEvidence;
    async fn two_live_calls(&self, left: Vec<u8>, right: Vec<u8>) -> Vec<Vec<u8>>;
    async fn two_http_calls(
        &self,
        left_first: Vec<u8>,
        left_rest: Vec<u8>,
        right_first: Vec<u8>,
        right_rest: Vec<u8>,
    ) -> Vec<StreamEvidence>;
    async fn edge_lifecycles(&self) -> Vec<String>;
    async fn raw_modes_and_handles(&self) -> Vec<String>;
    async fn raw_handle_lifecycles(&self) -> Vec<String>;
    async fn raw_observer_detach_and_fire_open(&self) -> Vec<String>;
    async fn stdout_drop_preserves_sibling(&self) -> Vec<String>;
    async fn hold_open_stdin(&self, calls: u32);
    async fn hold_unread_stdout(&self, calls: u32);
    async fn hold_capable_stdout_before_result(&self, path: String, input: Vec<u8>);
    async fn hold_synchronous_capable_staging(&self, input: Vec<u8>);
    async fn hold_fire_and_forget_capable_staging(&self, input: Vec<u8>);
    async fn hold_capable_publication_checkpoint(&self, path: String, input: Vec<u8>);
    async fn capable_modes_and_cohorts(&self) -> Vec<String>;
    async fn collect_capable(&self, path: String, input: Vec<u8>) -> StreamEvidence;
    async fn clean_stdout_then_trap(&self);
    async fn trap_with_blocked_sibling(&self);
    async fn drop_trapping_result(&self);
    async fn fire_and_forget_trap(&self);
    async fn hold_incapable_checkpoint(&self, checkpoint: String);
    async fn hold_capable_staging_checkpoint(&self, input: Vec<u8>);
    async fn hold_capable_checkpoint(&self, path: String, input: Vec<u8>);
    async fn hold_capable_terminal_checkpoint(&self, path: String, input: Vec<u8>);
    async fn hold_capable_published_checkpoint(&self, path: String, input: Vec<u8>);
    async fn hold_capable_overflow_terminal(&self, input: Vec<u8>);
    async fn hold_completed_reconstruction_barrier(&self);
    async fn hold_completed_attachment_reconstruction_under_pressure(
        &self,
        path: String,
        input_size: u64,
    );
    async fn reject_incomplete_attachment_upgrade_under_pressure(&self) -> Vec<String>;
    async fn hold_completed_reconstruction_before_exclusive_clock(&self);
    async fn hold_reconstruction_backpressure_before_exclusive_clock(
        &self,
        first: Vec<u8>,
        second: Vec<u8>,
    );
    async fn hold_completed_reconstruction_before_incomplete_custom(&self);
    async fn principal_context(&self, principal: Principal) -> Vec<String>;
}

struct ToolStreamingCallerImpl;

fn input_stream(chunks: Vec<Vec<u8>>) -> InputStream {
    let (mut writer, reader) =
        golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
    spawn_local(async move {
        for chunk in chunks {
            if !writer.write_all(vec![Ok(chunk)]).await.is_empty() {
                break;
            }
        }
    });
    reader
}

fn chunks(input: Vec<u8>, fragment_size: u32) -> Vec<Vec<u8>> {
    let fragment_size = usize::try_from(fragment_size.max(1)).unwrap();
    input
        .chunks(fragment_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn principal_class(principal: &Principal) -> &'static str {
    match principal {
        Principal::Anonymous => "anonymous",
        Principal::Oidc(_) => "oidc",
        Principal::Agent(_) => "agent",
        Principal::GolemUser(_) => "golem-user",
    }
}

fn evidence(
    result: Result<StreamSummary, golem_rust::agentic::ToolError<StreamingRunError>>,
    output: Vec<u8>,
) -> StreamEvidence {
    match result {
        Ok(summary) => StreamEvidence {
            output,
            chunks_read: summary.chunks_read,
            bytes_read: summary.bytes_read,
            output_closed: summary.output_closed,
            completion: "ok".to_string(),
        },
        Err(error) => StreamEvidence {
            output,
            chunks_read: 0,
            bytes_read: 0,
            output_closed: false,
            completion: format!("{error:?}"),
        },
    }
}

async fn read_all(mut stdout: InputStream) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(item) = stdout.next().await {
        match item {
            Ok(chunk) => output.extend(chunk),
            Err(failure) => panic!("tool stdout failed: {failure:?}"),
        }
    }
    output
}

async fn first_chunk<T, E>(invocation: &mut ToolInvocation<T, E>) -> Vec<u8> {
    match invocation.stdout.next().await {
        Some(Ok(chunk)) => chunk,
        other => panic!("expected a live stdout chunk, got {other:?}"),
    }
}

fn raw_input(mode: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawRunInput {
        mode: mode.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode raw streaming tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode raw streaming tool wire input")
}

fn raw_no_stream_input(value: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawNoStreamInput {
        value: value.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode raw no-stream tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode raw no-stream wire input")
}

fn raw_optional_streams_input() -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawOptionalStreamsInput {}
        .into_typed_schema_value()
        .expect("encode raw optional-streams tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode raw optional-streams wire input")
}

fn raw_capable_input(path: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawCapableInput {
        path: path.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode raw capable tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode raw capable wire input")
}

fn raw_stdin(chunks: Vec<Vec<u8>>) -> tool_host::ToolStdin {
    pump_tool_stdin(input_stream(chunks))
}

fn closed_raw_stdin() -> tool_host::ToolStdin {
    let (writer, reader) =
        golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
    drop(writer);
    pump_tool_stdin(reader)
}

async fn raw_result(
    future: &tool_host::FutureInvokeResult,
) -> Result<tool_host::InvocationResult, RpcError> {
    future.get().await
}

async fn raw_chunk(stdout: &mut InputStream) -> Vec<u8> {
    match stdout.next().await {
        Some(Ok(bytes)) => bytes,
        other => panic!("expected raw stdout chunk, got {other:?}"),
    }
}

async fn wait_at_crash_checkpoint(name: &str) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
    };
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("CALLER_CRASH_CHECKPOINT_PORT")
        .or_else(|_| std::env::var("CRASH_CHECKPOINT_PORT"))
        .expect("caller crash checkpoint port is configured");
    let gate_port = std::env::var("CALLER_CRASH_CHECKPOINT_GATE_PORT")
        .or_else(|_| std::env::var("CRASH_CHECKPOINT_GATE_PORT"))
        .expect("caller crash checkpoint gate port is configured")
        .parse()
        .expect("caller crash checkpoint gate port is a valid port");
    let headers = types::Fields::from_list(&[]).expect("valid checkpoint fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set checkpoint method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set checkpoint scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set checkpoint authority");
    request
        .set_path_with_query(Some(&format!("/{name}")))
        .expect("set checkpoint path");
    let receive_response = async move {
        client::send(request)
            .await
            .expect("send checkpoint request")
    };
    let finish_request = async move {
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish checkpoint request");
        transmit.await.expect("transmit checkpoint request");
    };
    let (response, ()) = (receive_response, finish_request).join().await;
    assert_eq!(response.get_status_code(), 204);
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (body, trailers) = types::Response::consume_body(response, response_done_rx);
    response_done_tx
        .write(Ok(()))
        .await
        .expect("finish checkpoint response");
    drop(body);
    drop(trailers);

    golem_rust::atomically_async(|| async {
        let socket =
            TcpSocket::create(IpAddressFamily::Ipv4).expect("create checkpoint gate socket");
        socket
            .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port: gate_port,
            }))
            .await
            .expect("connect to checkpoint gate");
        let (mut stream, received) = socket.receive();
        let mut bytes = Vec::new();
        let mut buffer = Vec::with_capacity(1);
        loop {
            let (result, next_buffer) = stream.read(buffer).await;
            buffer = next_buffer;
            match result {
                StreamResult::Complete(len) => {
                    bytes.extend_from_slice(&buffer[..len]);
                    buffer.clear();
                }
                StreamResult::Dropped => break,
                StreamResult::Cancelled => panic!("checkpoint gate read was cancelled"),
            }
        }
        drop(stream);
        received.await.expect("finish checkpoint gate receive");
        assert_eq!(bytes, [1], "checkpoint gate returns one release byte");
    })
    .await;
}

#[agent_implementation]
impl ToolStreamingCaller for ToolStreamingCallerImpl {
    fn new(_name: String) -> Self {
        Self
    }

    async fn concurrent_attempt_identity_replay(&self) -> Vec<String> {
        let rpc = ToolRpc::new("streaming");
        let first = rpc.invoke_and_await(
            vec!["no-stream".to_string()],
            raw_no_stream_input("hold-attempt-identity"),
            None,
            None,
        );
        let second = rpc.invoke_and_await(
            vec!["no-stream".to_string()],
            raw_no_stream_input("hold-attempt-identity"),
            None,
            None,
        );
        let (first, second) = (first, second).join().await;
        let outcomes = vec![
            if first.is_ok() {
                "accepted"
            } else {
                "rejected"
            }
            .to_string(),
            if second.is_ok() {
                "accepted"
            } else {
                "rejected"
            }
            .to_string(),
        ];
        wait_at_crash_checkpoint("concurrent-attempt-identities").await;
        outcomes
    }

    async fn marker_before_eof(&self, first: Vec<u8>, rest: Vec<u8>) -> StreamEvidence {
        let (mut writer, stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let mut invocation = StreamingClient::default()
            .run("marker-echo".to_string(), stdin)
            .expect("start streaming tool");
        let mut output = first_chunk(&mut invocation).await;
        assert_eq!(output, b"marker:");
        assert!(
            writer.write_all(vec![Ok(first), Ok(rest)]).await.is_empty(),
            "write input chunks"
        );
        drop(writer);

        let result = invocation.result().await;
        output.extend(read_all(invocation.stdout).await);
        evidence(result, output)
    }

    async fn alternating_echo(&self, chunk_count: u32, chunk_size: u32) -> StreamEvidence {
        let (mut writer, stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let mut invocation = StreamingClient::default()
            .run("echo".to_string(), stdin)
            .expect("start alternating echo tool");
        let mut output = Vec::new();
        for index in 0..chunk_count {
            let chunk = vec![(index % 251) as u8; chunk_size as usize];
            assert!(
                writer.write_all(vec![Ok(chunk.clone())]).await.is_empty(),
                "write alternating echo input"
            );
            let echoed = first_chunk(&mut invocation).await;
            assert_eq!(echoed, chunk);
            output.extend(echoed);
        }
        drop(writer);
        let result = invocation.result().await;
        output.extend(read_all(invocation.stdout).await);
        evidence(result, output)
    }

    async fn collect(&self, mode: String, input: Vec<u8>, fragment_size: u32) -> StreamEvidence {
        let invocation = StreamingClient::default()
            .run(mode, input_stream(chunks(input, fragment_size)))
            .expect("start streaming tool");
        match invocation.collect().await {
            Ok((summary, output)) => evidence(Ok(summary), output),
            Err(error) => evidence(Err(error), Vec::new()),
        }
    }

    async fn result_before_stdout(&self, mode: String) -> StreamEvidence {
        let invocation = StreamingClient::default()
            .run(mode, input_stream(Vec::new()))
            .expect("start streaming tool");
        let result = invocation.result().await;
        let output = read_all(invocation.stdout).await;
        evidence(result, output)
    }

    async fn two_live_calls(&self, left: Vec<u8>, right: Vec<u8>) -> Vec<Vec<u8>> {
        let (mut left_writer, left_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (mut right_writer, right_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let mut left_call = StreamingClient::default()
            .run("marker-echo".to_string(), left_stdin)
            .expect("start left streaming tool");
        let mut right_call = StreamingClient::default()
            .run("marker-echo".to_string(), right_stdin)
            .expect("start right streaming tool");

        let mut left_output = first_chunk(&mut left_call).await;
        let mut right_output = first_chunk(&mut right_call).await;
        assert_eq!(left_output, b"marker:");
        assert_eq!(right_output, b"marker:");
        assert!(
            left_writer
                .write_all(vec![Ok(left.clone())])
                .await
                .is_empty(),
            "write left input"
        );
        assert!(
            right_writer
                .write_all(vec![Ok(right.clone())])
                .await
                .is_empty(),
            "write right input"
        );
        drop(left_writer);
        drop(right_writer);

        let left_result = left_call.result();
        let right_result = right_call.result();
        left_output.extend(read_all(left_call.stdout).await);
        right_output.extend(read_all(right_call.stdout).await);
        let _ = left_result.await.expect("left result");
        let _ = right_result.await.expect("right result");
        vec![left_output, right_output]
    }

    async fn two_http_calls(
        &self,
        left_first: Vec<u8>,
        left_rest: Vec<u8>,
        right_first: Vec<u8>,
        right_rest: Vec<u8>,
    ) -> Vec<StreamEvidence> {
        let (mut left_writer, left_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (mut right_writer, right_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let mut left_call = StreamingClient::default()
            .run("http-left".to_string(), left_stdin)
            .expect("start left HTTP streaming tool");
        let mut right_call = StreamingClient::default()
            .run("http-right".to_string(), right_stdin)
            .expect("start right HTTP streaming tool");

        assert!(
            left_writer
                .write_all(vec![Ok(left_first.clone())])
                .await
                .is_empty(),
            "write left initial HTTP input"
        );
        assert!(
            right_writer
                .write_all(vec![Ok(right_first.clone())])
                .await
                .is_empty(),
            "write right initial HTTP input"
        );

        let mut left_output = first_chunk(&mut left_call).await;
        let mut right_output = first_chunk(&mut right_call).await;
        assert!(left_output.starts_with(b"http-left:"));
        assert!(right_output.starts_with(b"http-right:"));

        assert!(
            left_writer
                .write_all(vec![Ok(left_rest.clone())])
                .await
                .is_empty(),
            "write left remaining HTTP input"
        );
        assert!(
            right_writer
                .write_all(vec![Ok(right_rest.clone())])
                .await
                .is_empty(),
            "write right remaining HTTP input"
        );
        drop(left_writer);
        drop(right_writer);

        let left_result = left_call.result();
        let right_result = right_call.result();
        left_output.extend(read_all(left_call.stdout).await);
        right_output.extend(read_all(right_call.stdout).await);
        let left_result = left_result.await;
        let right_result = right_result.await;
        vec![
            evidence(left_result, left_output),
            evidence(right_result, right_output),
        ]
    }

    async fn edge_lifecycles(&self) -> Vec<String> {
        let large = StreamingClient::default()
            .run("large".to_string(), input_stream(Vec::new()))
            .expect("start large streaming tool")
            .collect()
            .await
            .expect("collect large output");
        assert_eq!(large.1.len(), 512 * 4096);

        let (mut ignored_source, ignored_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let ignored = StreamingClient::default()
            .run("early-stdin-close".to_string(), ignored_stdin)
            .expect("start early-stdin-close tool")
            .collect()
            .await
            .expect("collect early-stdin-close output");
        assert_eq!(ignored.1, b"stdin-ignored");
        assert!(
            !ignored_source
                .write_all(vec![Ok(b"ignored".to_vec())])
                .await
                .is_empty(),
            "sidecar stdin drop must close the native source pump"
        );

        let early_stdout = StreamingClient::default()
            .run(
                "early-stdout-close".to_string(),
                input_stream(vec![b"left".to_vec(), b"right".to_vec()]),
            )
            .expect("start early-stdout-close tool")
            .collect()
            .await
            .expect("collect early-stdout-close result");
        assert!(early_stdout.1.is_empty());
        assert_eq!(early_stdout.0.chunks_read, 2);

        let (mut failed_source, failed_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let failed_input = StreamingClient::default()
            .run("marker-echo".to_string(), failed_stdin)
            .expect("start failed-input tool");
        assert!(
            failed_source
                .write_all(vec![Err(ByteStreamFailure::Failed(
                    "deterministic source failure".to_string(),
                ))])
                .await
                .is_empty()
        );
        drop(failed_source);
        let failed_input = failed_input
            .collect()
            .await
            .expect("observe source failure");
        assert_eq!(failed_input.1, b"marker:");

        let rpc = ToolRpc::new("streaming");
        rpc.invoke_and_await(
            vec!["no-stream".to_string()],
            raw_no_stream_input("ok"),
            None,
            None,
        )
        .await
        .expect("no-stream command");

        rpc.invoke_and_await(
            vec!["optional-streams".to_string()],
            raw_optional_streams_input(),
            None,
            None,
        )
        .await
        .expect("optional command without streams");
        rpc.invoke_and_await(
            vec!["optional-streams".to_string()],
            raw_optional_streams_input(),
            Some(raw_stdin(vec![b"stdin-only".to_vec()])),
            None,
        )
        .await
        .expect("optional command with stdin only");
        let (optional_target, optional_stdout) = tool_host::create_stdout();
        let optional = rpc.invoke_and_await(
            vec!["optional-streams".to_string()],
            raw_optional_streams_input(),
            Some(raw_stdin(vec![b"both".to_vec()])),
            Some(optional_target),
        );
        let (optional, optional_output) = (optional, read_all(optional_stdout)).join().await;
        optional.expect("optional command with both streams");
        assert_eq!(optional_output, b"both");

        let (rejected_target, mut rejected_stdout) = tool_host::create_stdout();
        let rejected = rpc
            .invoke_and_await(
                vec!["run".to_string()],
                raw_input("echo"),
                None,
                Some(rejected_target),
            )
            .await;
        assert!(matches!(rejected, Err(RpcError::ProtocolError(_))));
        assert!(matches!(
            rejected_stdout.next().await,
            Some(Err(ByteStreamFailure::Failed(_)))
        ));

        let (unused_target, mut unused_stdout) = tool_host::create_stdout();
        drop(unused_target);
        assert!(matches!(
            unused_stdout.next().await,
            Some(Err(ByteStreamFailure::Abandoned))
        ));

        vec![
            "large-collect".to_string(),
            "early-stdin-close".to_string(),
            "early-stdout-close".to_string(),
            "source-failure".to_string(),
            "no-stream".to_string(),
            "optional-streams".to_string(),
            "pre-dispatch-rejection".to_string(),
            "unused-stdout".to_string(),
        ]
    }

    async fn raw_modes_and_handles(&self) -> Vec<String> {
        let rpc = ToolRpc::new("streaming");
        let path = ["run".to_string()];

        let (sync_stdout_target, sync_stdout) = tool_host::create_stdout();
        let sync_call = rpc.invoke_and_await(
            path.to_vec(),
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"sync".to_vec()])),
            Some(sync_stdout_target),
        );
        let (sync_result, sync_output) = (sync_call, read_all(sync_stdout)).join().await;
        sync_result.expect("raw invoke-and-await result");
        assert_eq!(sync_output, b"marker:sync");

        rpc.invoke(&path, raw_input("large"), Some(raw_stdin(Vec::new())))
            .expect("raw fire-and-forget admission with discarded stdout");

        let (async_stdout_target, mut async_stdout) = tool_host::create_stdout();
        let async_result = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"async".to_vec()])),
            Some(async_stdout_target),
        );
        assert_eq!(raw_chunk(&mut async_stdout).await, b"marker:");
        raw_result(&async_result).await.expect("raw async result");
        assert_eq!(read_all(async_stdout).await, b"async");

        vec![
            "invoke-and-await".to_string(),
            "invoke".to_string(),
            "async-invoke-and-await".to_string(),
        ]
    }

    async fn raw_handle_lifecycles(&self) -> Vec<String> {
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        let rpc = ToolRpc::new("streaming");
        let path = ["run".to_string()];

        let (left_target, mut left_stdout) = tool_host::create_stdout();
        let (right_target, mut right_stdout) = tool_host::create_stdout();
        let left = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"left".to_vec()])),
            Some(left_target),
        );
        let right = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"right".to_vec()])),
            Some(right_target),
        );
        assert_eq!(raw_chunk(&mut left_stdout).await, b"marker:");
        assert_eq!(raw_chunk(&mut right_stdout).await, b"marker:");
        raw_result(&right)
            .await
            .expect("right raw async result first");
        raw_result(&left)
            .await
            .expect("left raw async result second");
        assert_eq!(read_all(left_stdout).await, b"left");
        assert_eq!(read_all(right_stdout).await, b"right");

        let (mut cancel_source, cancel_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (cancel_target, mut cancel_stdout) = tool_host::create_stdout();
        let cancelled = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(pump_tool_stdin(cancel_stdin)),
            Some(cancel_target),
        );
        assert_eq!(raw_chunk(&mut cancel_stdout).await, b"marker:");
        cancelled.cancel();
        assert!(
            matches!(raw_result(&cancelled).await, Err(RpcError::Cancelled)),
            "explicit future cancellation must select cancelled"
        );
        assert!(
            !cancel_source
                .write_all(vec![Ok(b"after-cancel".to_vec())])
                .await
                .is_empty(),
            "cancellation must close the stdin pump"
        );

        let (detached_target, detached_stdout) = tool_host::create_stdout();
        let detached = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"detached".to_vec()])),
            Some(detached_target),
        );
        drop(detached);
        assert_eq!(read_all(detached_stdout).await, b"marker:detached");

        let (dropped_output_target, dropped_output) = tool_host::create_stdout();
        let dropped_output_result = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"ignored".to_vec()])),
            Some(dropped_output_target),
        );
        drop(dropped_output);
        raw_result(&dropped_output_result)
            .await
            .map_err(|error| tool_protocol_error::<StreamingRunError>(format!("{error:?}")))
            .expect("dropping stdout reader must not cancel the result observer");

        let (mut resumed_source, resumed_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (resumed_target, mut resumed_stdout) = tool_host::create_stdout();
        let resumed_result = rpc.async_invoke_and_await(
            &path,
            raw_input("marker-echo"),
            Some(pump_tool_stdin(resumed_stdin)),
            Some(resumed_target),
        );
        assert_eq!(raw_chunk(&mut resumed_stdout).await, b"marker:");
        let mut pending_read = Box::pin(resumed_stdout.read(Vec::with_capacity(1)));
        poll_fn(|cx| match pending_read.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(result) => panic!("stdout read completed before cancellation: {result:?}"),
        })
        .await;
        let (cancelled, buffer) = pending_read.as_mut().cancel();
        assert_eq!(format!("{cancelled:?}"), "Cancelled");
        assert!(buffer.is_empty());
        drop(pending_read);
        assert!(
            resumed_source
                .write_all(vec![Ok(b"resumed".to_vec())])
                .await
                .is_empty()
        );
        drop(resumed_source);
        assert_eq!(read_all(resumed_stdout).await, b"resumed");
        raw_result(&resumed_result)
            .await
            .expect("result after resuming a cancelled stdout read");

        vec![
            "out-of-order-get".to_string(),
            "explicit-cancel".to_string(),
            "result-detach".to_string(),
            "stdout-detach".to_string(),
            "stdout-operation-resume".to_string(),
        ]
    }

    async fn raw_observer_detach_and_fire_open(&self) -> Vec<String> {
        enum SynchronousObserverRace {
            Result,
            Stdout(InputStream),
        }

        let rpc = ToolRpc::new("streaming");
        let path = ["run".to_string()];
        let (mut fire_source, fire_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        rpc.invoke(
            &path,
            raw_input("marker-echo"),
            Some(pump_tool_stdin(fire_stdin)),
        )
        .expect("raw fire-and-forget admission with open stdin and discarded stdout");
        assert!(
            fire_source
                .write_all(vec![Ok(b"fire".to_vec())])
                .await
                .is_empty()
        );

        let (observer_target, mut observer_stdout) = tool_host::create_stdout();
        let synchronous_observer = rpc.invoke_and_await(
            path.to_vec(),
            raw_input("marker-echo"),
            Some(raw_stdin(vec![b"observer-detached".to_vec()])),
            Some(observer_target),
        );
        let observer_outcome = (
            async {
                synchronous_observer
                    .await
                    .expect("synchronous observer result");
                SynchronousObserverRace::Result
            },
            async move {
                assert_eq!(raw_chunk(&mut observer_stdout).await, b"marker:");
                SynchronousObserverRace::Stdout(observer_stdout)
            },
        )
            .race()
            .await;
        let observer_stdout = match observer_outcome {
            SynchronousObserverRace::Result => {
                panic!("synchronous result completed before live stdout")
            }
            SynchronousObserverRace::Stdout(stdout) => stdout,
        };
        assert_eq!(read_all(observer_stdout).await, b"observer-detached");
        drop(fire_source);

        vec![
            "invoke-open-stdin".to_string(),
            "invoke-and-await-observer-detach".to_string(),
        ]
    }

    async fn stdout_drop_preserves_sibling(&self) -> Vec<String> {
        let mut blocked = StreamingClient::default()
            .run("backpressure".to_string(), input_stream(Vec::new()))
            .expect("start output-blocked tool");
        assert_eq!(
            first_chunk(&mut blocked).await.len(),
            4096,
            "backpressured output starts before its reader is dropped"
        );
        let dropped_stdout = std::mem::replace(&mut blocked.stdout, input_stream(Vec::new()));
        drop(dropped_stdout);

        let sibling = StreamingClient::default()
            .run(
                "marker-echo".to_string(),
                input_stream(vec![b"sibling".to_vec()]),
            )
            .expect("start sibling streaming tool")
            .collect()
            .await
            .expect("sibling must remain independent");
        assert_eq!(sibling.1, b"marker:sibling");

        let blocked_result = blocked
            .result()
            .await
            .expect("dropping stdout must wake the blocked writer");
        assert!(blocked_result.output_closed);

        vec![
            "blocked-writer-woke".to_string(),
            "sibling-completed".to_string(),
        ]
    }

    async fn hold_open_stdin(&self, calls: u32) {
        let mut sources = Vec::new();
        let mut invocations = Vec::new();
        for _ in 0..calls {
            let (source, stdin) =
                golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
            let mut invocation = StreamingClient::default()
                .run("marker-echo".to_string(), stdin)
                .expect("start held streaming tool");
            assert_eq!(first_chunk(&mut invocation).await, b"marker:");
            sources.push(source);
            invocations.push(invocation);
        }
        let _keep_sources_open = sources;
        invocations[0]
            .result()
            .await
            .expect("held streaming tool only completes after interruption");
    }

    async fn hold_unread_stdout(&self, calls: u32) {
        let mut invocations = Vec::new();
        for _ in 0..calls {
            invocations.push(
                StreamingClient::default()
                    .run("backpressure".to_string(), input_stream(Vec::new()))
                    .expect("start backpressured streaming tool"),
            );
        }
        invocations[0]
            .result()
            .await
            .expect("backpressured streaming tool only completes after interruption");
    }

    async fn hold_capable_stdout_before_result(&self, path: String, input: Vec<u8>) {
        let mut invocation = CapableStreamingClient::default()
            .run_capable(path, input_stream(vec![input]))
            .expect("start held capable streaming tool");
        let item = invocation.stdout.next().await;
        panic!("capable stdout became visible before result-await admission: {item:?}");
    }

    async fn hold_synchronous_capable_staging(&self, input: Vec<u8>) {
        let rpc = ToolRpc::new("capable-streaming");
        let mut expected_output = b"body-checkpoint".to_vec();
        expected_output.extend_from_slice(&input);
        let (mut source, stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.invoke_and_await(
            vec!["run-capable".to_string()],
            raw_capable_input("hold-body:/sync-capable-staging.bin"),
            Some(pump_tool_stdin(stdin)),
            Some(stdout_target),
        );
        let control = async move {
            assert!(source.write_all(vec![Ok(input)]).await.is_empty());
            wait_at_crash_checkpoint("sync-capable-staging").await;
            drop(source);
            read_all(stdout).await
        };
        let (result, output) = (result, control).join().await;
        result.expect("synchronous capable staging result");
        assert_eq!(output, expected_output);
    }

    async fn hold_fire_and_forget_capable_staging(&self, input: Vec<u8>) {
        let rpc = ToolRpc::new("capable-streaming");
        let (mut source, stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        rpc.invoke(
            &["run-capable".to_string()],
            raw_capable_input("hold-body:/fire-capable-staging.bin"),
            Some(pump_tool_stdin(stdin)),
        )
        .expect("fire-and-forget capable staging admission");
        assert!(source.write_all(vec![Ok(input)]).await.is_empty());
        wait_at_crash_checkpoint("fire-capable-staging").await;
        drop(source);
        wait_at_crash_checkpoint("fire-capable-ready-parent-open").await;
    }

    async fn hold_capable_publication_checkpoint(&self, path: String, input: Vec<u8>) {
        let invocation = CapableStreamingClient::default()
            .run_capable(
                format!("hold-publication:{path}"),
                input_stream(vec![input.clone()]),
            )
            .expect("start capable publication checkpoint tool");
        let result = invocation.result();
        let mut stdout = invocation.stdout;
        let output = async move {
            assert_eq!(
                stdout
                    .next()
                    .await
                    .expect("capable output must be published after lane return")
                    .expect("capable output chunk must succeed"),
                input
            );
            std::fs::write("/capable-publication-observed.bin", b"observed")
                .expect("lane must have returned before capable stdout publication");
            let _ = golem_rust::get_oplog_index();
            wait_at_crash_checkpoint("caller-observed-capable-publication").await;
            read_all(stdout).await
        };
        let (result, remaining_output) = (result, output).join().await;
        result.expect("capable publication checkpoint result");
        assert!(remaining_output.is_empty());
    }

    async fn capable_modes_and_cohorts(&self) -> Vec<String> {
        let rpc = ToolRpc::new("capable-streaming");
        let path = ["run-capable".to_string()];

        let (sync_target, sync_stdout) = tool_host::create_stdout();
        let sync = rpc.invoke_and_await(
            path.to_vec(),
            raw_capable_input("/capable-sync.bin"),
            Some(raw_stdin(vec![b"sync".to_vec()])),
            Some(sync_target),
        );
        let (sync, sync_output) = (sync, read_all(sync_stdout)).join().await;
        sync.expect("synchronous capable result");
        assert_eq!(sync_output, b"sync");

        let (mut reverse_first_source, reverse_first_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (mut reverse_second_source, reverse_second_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (reverse_first_target, reverse_first_stdout) = tool_host::create_stdout();
        let (reverse_second_target, reverse_second_stdout) = tool_host::create_stdout();
        let reverse_first = rpc.invoke_and_await(
            path.to_vec(),
            raw_capable_input("order:R1:/capable-r1.bin"),
            Some(pump_tool_stdin(reverse_first_stdin)),
            Some(reverse_first_target),
        );
        let reverse_second = rpc.invoke_and_await(
            path.to_vec(),
            raw_capable_input("order:R2:/capable-r2.bin"),
            Some(pump_tool_stdin(reverse_second_stdin)),
            Some(reverse_second_target),
        );
        let reverse_feed = async move {
            assert!(
                reverse_second_source
                    .write_all(vec![Ok(b"reverse-second".to_vec())])
                    .await
                    .is_empty()
            );
            drop(reverse_second_source);
            assert!(
                reverse_first_source
                    .write_all(vec![Ok(b"reverse-first".to_vec())])
                    .await
                    .is_empty()
            );
            drop(reverse_first_source);
        };
        let (reverse_first, reverse_second, ()) = (
            (reverse_first, read_all(reverse_first_stdout)).join(),
            (reverse_second, read_all(reverse_second_stdout)).join(),
            reverse_feed,
        )
            .join()
            .await;
        reverse_first
            .0
            .expect("first reverse-staged synchronous call");
        reverse_second
            .0
            .expect("second reverse-staged synchronous call");
        assert_eq!(reverse_first.1, b"reverse-first");
        assert_eq!(reverse_second.1, b"reverse-second");

        let (mut first_source, first_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (mut second_source, second_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (first_target, first_stdout) = tool_host::create_stdout();
        let (second_target, second_stdout) = tool_host::create_stdout();
        let first = rpc.async_invoke_and_await(
            &path,
            raw_capable_input("order:S1:/capable-s1.bin"),
            Some(pump_tool_stdin(first_stdin)),
            Some(first_target),
        );
        let second = rpc.async_invoke_and_await(
            &path,
            raw_capable_input("order:S2:/capable-s2.bin"),
            Some(pump_tool_stdin(second_stdin)),
            Some(second_target),
        );
        assert!(
            second_source
                .write_all(vec![Ok(b"second".to_vec())])
                .await
                .is_empty()
        );
        drop(second_source);
        assert!(
            first_source
                .write_all(vec![Ok(b"first".to_vec())])
                .await
                .is_empty()
        );
        drop(first_source);
        let results = tool_host::get_invoke_results(vec![&first, &second]).await;
        assert!(results.into_iter().all(|result| result.is_ok()));
        assert_eq!(read_all(first_stdout).await, b"first");
        assert_eq!(read_all(second_stdout).await, b"second");

        let (dropped_target, dropped_stdout) = tool_host::create_stdout();
        let dropped = rpc.async_invoke_and_await(
            &path,
            raw_capable_input("order:P1:/capable-parent-async.bin"),
            Some(raw_stdin(vec![b"parent-async".to_vec()])),
            Some(dropped_target),
        );
        drop(dropped);
        drop(dropped_stdout);
        let (no_body_target, no_body_stdout) = tool_host::create_stdout();
        let no_body = rpc.async_invoke_and_await(
            &path,
            raw_capable_input("order:PN:/capable-parent-no-body.bin"),
            Some(raw_stdin(vec![b"must-not-run".to_vec()])),
            Some(no_body_target),
        );
        no_body.cancel();
        drop(no_body);
        drop(no_body_stdout);
        rpc.invoke(
            &path,
            raw_capable_input("order:P2:/capable-parent-fire.bin"),
            Some(raw_stdin(vec![b"parent-fire".to_vec()])),
        )
        .expect("fire-and-forget capable admission");

        vec![
            "synchronous".to_string(),
            "reverse-synchronous".to_string(),
            "reverse-result-cohort".to_string(),
            "parent-end-async".to_string(),
            "parent-end-no-body".to_string(),
            "parent-end-fire-and-forget".to_string(),
        ]
    }

    async fn collect_capable(&self, path: String, input: Vec<u8>) -> StreamEvidence {
        let invocation = CapableStreamingClient::default()
            .run_capable(path.clone(), input_stream(vec![input.clone()]))
            .expect("start capable streaming tool");
        match invocation.collect().await {
            Ok((summary, output)) => StreamEvidence {
                output,
                chunks_read: summary.chunks_read,
                bytes_read: summary.bytes_read,
                output_closed: summary.output_closed,
                completion: "ok".to_string(),
            },
            Err(error) => StreamEvidence {
                output: Vec::new(),
                chunks_read: 0,
                bytes_read: 0,
                output_closed: false,
                completion: format!("{error:?}"),
            },
        }
    }

    async fn clean_stdout_then_trap(&self) {
        let rpc = ToolRpc::new("streaming");
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("trap-after-clean-eof"),
            Some(raw_stdin(Vec::new())),
            Some(stdout_target),
        );
        assert_eq!(read_all(stdout).await, b"marker:");
        wait_at_crash_checkpoint("caller-observed-clean-stdout").await;
        raw_result(&result)
            .await
            .expect("owner trap must abort this result observation");
    }

    async fn trap_with_blocked_sibling(&self) {
        let (blocked_source, blocked_stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let mut blocked = StreamingClient::default()
            .run("marker-echo".to_string(), blocked_stdin)
            .expect("start blocked sibling tool");
        assert_eq!(first_chunk(&mut blocked).await, b"marker:");

        let mut trapped = StreamingClient::default()
            .run("trap".to_string(), input_stream(Vec::new()))
            .expect("start trapping tool");
        assert_eq!(first_chunk(&mut trapped).await, b"marker:");
        assert!(matches!(
            trapped.stdout.next().await,
            Some(Err(ByteStreamFailure::Failed(_)))
        ));
        let _keep_blocked_source_open = blocked_source;
        let _keep_blocked_invocation_alive = blocked;
        trapped
            .result()
            .await
            .expect("owner trap must abort the primary while its sibling remains blocked");
    }

    async fn drop_trapping_result(&self) {
        let rpc = ToolRpc::new("streaming");
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("trap"),
            Some(raw_stdin(Vec::new())),
            Some(stdout_target),
        );
        drop(result);
        drop(stdout);
    }

    async fn fire_and_forget_trap(&self) {
        ToolRpc::new("streaming")
            .invoke(
                &["run".to_string()],
                raw_input("trap"),
                Some(raw_stdin(Vec::new())),
            )
            .expect("start fire-and-forget trapping tool");
    }

    async fn hold_incapable_checkpoint(&self, checkpoint: String) {
        let rpc = ToolRpc::new("streaming");
        let path = ["run".to_string()];
        match checkpoint.as_str() {
            "before-input" => {
                let (source, stdin) = golem_rust::golem_agentic::wit_stream::new::<
                    Result<Vec<u8>, ByteStreamFailure>,
                >();
                let (stdout_target, stdout) = tool_host::create_stdout();
                let result = rpc.async_invoke_and_await(
                    &path,
                    raw_input("echo"),
                    Some(pump_tool_stdin(stdin)),
                    Some(stdout_target),
                );
                wait_at_crash_checkpoint("before-input").await;
                drop(source);
                drop(stdout);
                let _ = raw_result(&result).await;
            }
            "after-input-and-stdout" => {
                let (mut source, stdin) = golem_rust::golem_agentic::wit_stream::new::<
                    Result<Vec<u8>, ByteStreamFailure>,
                >();
                let mut invocation = StreamingClient::default()
                    .run("marker-echo".to_string(), stdin)
                    .expect("start input/output checkpoint tool");
                assert_eq!(first_chunk(&mut invocation).await, b"marker:");
                assert!(
                    source
                        .write_all(vec![Ok(b"checkpoint".to_vec())])
                        .await
                        .is_empty()
                );
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/after-input-and-stdout.checkpoint")
                    .and_then(|mut file| file.write_all(b"reached"))
                    .expect("append caller input/output checkpoint once");
                let _ = golem_rust::get_oplog_index();
                assert_eq!(first_chunk(&mut invocation).await, b"checkpoint");
                wait_at_crash_checkpoint("after-input-and-stdout").await;
                drop(source);
                let _ = invocation.result().await;
            }
            "after-eof-before-terminal" => {
                let (stdout_target, mut stdout) = tool_host::create_stdout();
                let result = rpc.async_invoke_and_await(
                    &path,
                    raw_input("hold-after-eof"),
                    Some(raw_stdin(vec![b"checkpoint".to_vec()])),
                    Some(stdout_target),
                );
                std::fs::write("/after-eof-before-terminal.checkpoint", b"reached")
                    .expect("write caller EOF checkpoint");
                let _ = golem_rust::get_oplog_index();
                assert_eq!(raw_chunk(&mut stdout).await, b"eof-observed");
                let _ = raw_result(&result).await;
                drop(stdout);
            }
            "after-stdout-terminal" => {
                let (mut source, stdin) = golem_rust::golem_agentic::wit_stream::new::<
                    Result<Vec<u8>, ByteStreamFailure>,
                >();
                let (stdout_target, mut stdout) = tool_host::create_stdout();
                let result = rpc.async_invoke_and_await(
                    &path,
                    raw_input("hold-after-stdout-terminal"),
                    Some(pump_tool_stdin(stdin)),
                    Some(stdout_target),
                );
                assert_eq!(raw_chunk(&mut stdout).await, b"ready");
                assert!(
                    source
                        .write_all(vec![Ok(b"checkpoint".to_vec())])
                        .await
                        .is_empty()
                );
                std::fs::write("/after-stdout-terminal.checkpoint", b"reached")
                    .expect("write caller stdout-terminal checkpoint");
                let _ = golem_rust::get_oplog_index();
                assert_eq!(read_all(stdout).await, b"checkpoint");
                wait_at_crash_checkpoint("after-stdout-terminal").await;
                drop(source);
                let _ = raw_result(&result).await;
            }
            "after-terminal-before-result" => {
                let (mut source, stdin) = golem_rust::golem_agentic::wit_stream::new::<
                    Result<Vec<u8>, ByteStreamFailure>,
                >();
                let (stdout_target, stdout) = tool_host::create_stdout();
                let result = rpc.async_invoke_and_await(
                    &path,
                    raw_input("echo"),
                    Some(pump_tool_stdin(stdin)),
                    Some(stdout_target),
                );
                assert!(
                    source
                        .write_all(vec![Ok(b"checkpoint".to_vec())])
                        .await
                        .is_empty()
                );
                std::fs::write("/after-terminal-before-result.checkpoint", b"reached")
                    .expect("write caller terminal checkpoint");
                drop(source);
                assert_eq!(read_all(stdout).await, b"checkpoint");
                let _ = golem_rust::get_oplog_index();
                wait_at_crash_checkpoint("after-entity-terminal").await;
                let _ = raw_result(&result).await;
            }
            other => panic!("unknown incapable crash checkpoint: {other}"),
        }
    }

    async fn hold_capable_staging_checkpoint(&self, input: Vec<u8>) {
        let rpc = ToolRpc::new("capable-streaming");
        let command = ["run-capable".to_string()];
        let (mut source, stdin) =
            golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &command,
            raw_capable_input("/must-remain-staged.bin"),
            Some(pump_tool_stdin(stdin)),
            Some(stdout_target),
        );
        assert!(source.write_all(vec![Ok(input)]).await.is_empty());
        wait_at_crash_checkpoint("capable-staging").await;
        result.cancel();
        drop(source);
        drop(stdout);
        let _ = raw_result(&result).await;
    }

    async fn hold_capable_checkpoint(&self, path: String, input: Vec<u8>) {
        let rpc = ToolRpc::new("capable-streaming");
        let command = ["run-capable".to_string()];
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &command,
            raw_capable_input(&path),
            Some(raw_stdin(vec![input])),
            Some(stdout_target),
        );
        let _ = raw_result(&result).await;
        drop(stdout);
    }

    async fn hold_capable_terminal_checkpoint(&self, path: String, input: Vec<u8>) {
        let invocation = CapableStreamingClient::default()
            .run_capable(
                format!("hold-terminal:{path}"),
                input_stream(vec![input.clone()]),
            )
            .expect("start capable terminal checkpoint tool");
        let (result, output) = (invocation.result(), read_all(invocation.stdout))
            .join()
            .await;
        result.expect("capable terminal checkpoint result");
        assert_eq!(output, input);
        std::fs::write("/capable-terminal-published.checkpoint", b"reached")
            .expect("record capable terminal publication");
        let _ = golem_rust::get_oplog_index();
        wait_at_crash_checkpoint("capable-terminal-published").await;
    }

    async fn hold_capable_published_checkpoint(&self, path: String, input: Vec<u8>) {
        let invocation = CapableStreamingClient::default()
            .run_capable(path, input_stream(vec![input.clone()]))
            .expect("start post-publication capable checkpoint tool");
        let result = invocation.result();
        let output = read_all(invocation.stdout);
        let (result, output) = (result, output).join().await;
        result.expect("post-publication capable checkpoint result");
        assert_eq!(output, input);
        std::fs::write("/capable-published-observed.checkpoint", b"reached")
            .expect("record post-publication capable checkpoint");
        let _ = golem_rust::get_oplog_index();
        wait_at_crash_checkpoint("capable-published").await;
    }

    async fn hold_capable_overflow_terminal(&self, input: Vec<u8>) {
        let invocation = CapableStreamingClient::default()
            .run_capable(
                "/must-not-run-after-overflow.bin".to_string(),
                input_stream(vec![input]),
            )
            .expect("start capable overflow operation");
        let error = invocation
            .result()
            .await
            .expect_err("capable stdin overflow must select a no-body terminal");
        assert!(format!("{error:?}").contains("ResourceExhausted"));
        let _ = golem_rust::get_oplog_index();
        wait_at_crash_checkpoint("capable-stdin-overflow-terminal").await;
        drop(invocation);
    }

    async fn hold_completed_reconstruction_barrier(&self) {
        let rpc = ToolRpc::new("streaming");
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("historical-reconstruction-gate"),
            Some(raw_stdin(Vec::new())),
            Some(stdout_target),
        );
        assert!(read_all(stdout).await.is_empty());
        raw_result(&result)
            .await
            .expect("completed reconstruction result");
        wait_at_crash_checkpoint("reconstruction-live-effect").await;
    }

    async fn hold_completed_attachment_reconstruction_under_pressure(
        &self,
        path: String,
        input_size: u64,
    ) {
        let input = vec![b'i'; input_size as usize];
        let invocation = CapableStreamingClient::default()
            .run_capable(path, input_stream(vec![input.clone()]))
            .expect("start completed attachment reconstruction operation");
        let (summary, output) = invocation
            .collect()
            .await
            .expect("complete attachment reconstruction operation");
        assert_eq!(summary.bytes_read, input.len() as u64);
        assert_eq!(output, input);
        wait_at_crash_checkpoint("completed-attachment-pressure").await;
    }

    async fn reject_incomplete_attachment_upgrade_under_pressure(&self) -> Vec<String> {
        let rpc = ToolRpc::new("streaming");
        let (stdout_target, mut stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("hold-large-after-eof"),
            Some(closed_raw_stdin()),
            Some(stdout_target),
        );
        assert!(matches!(
            raw_result(&result).await,
            Err(RpcError::ResourceExhausted(_))
        ));
        assert!(matches!(
            stdout.next().await,
            Some(Err(ByteStreamFailure::ResourceExhausted))
        ));
        assert!(stdout.next().await.is_none());
        std::fs::write("/incomplete-attachment-upgrade-rejected", b"durable")
            .expect("record durable incomplete attachment rejection");
        vec![
            "resource-exhausted".to_string(),
            "stdout-resource-exhausted".to_string(),
        ]
    }

    async fn hold_completed_reconstruction_before_exclusive_clock(&self) {
        let rpc = ToolRpc::new("streaming");
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("historical-reconstruction-exclusive"),
            Some(closed_raw_stdin()),
            Some(stdout_target),
        );
        let tool = async {
            assert!(read_all(stdout).await.is_empty());
            raw_result(&result)
                .await
                .expect("completed reconstruction result before exclusive clock call");
        };
        let exclusive_clock = async {
            let _ = std::time::SystemTime::now();
        };
        (tool, exclusive_clock).join().await;
    }

    async fn hold_reconstruction_backpressure_before_exclusive_clock(
        &self,
        first: Vec<u8>,
        second: Vec<u8>,
    ) {
        let rpc = ToolRpc::new("streaming");
        let (stdin_writer, stdin, stdin_closed) = tool_host::create_stdin();
        stdin_writer
            .write(first)
            .await
            .expect("prefill backpressured reconstruction stdin");
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("historical-reconstruction-backpressure"),
            Some(stdin),
            Some(stdout_target),
        );
        let stdin = async {
            stdin_writer
                .write(second)
                .await
                .expect("write second backpressured reconstruction stdin chunk");
        };
        let stdin_terminal = async {
            assert!(matches!(
                stdin_closed.wait().await,
                ByteStreamCloseCause::ConsumerCancelled
            ));
        };
        let tool = async {
            assert!(read_all(stdout).await.is_empty());
            raw_result(&result)
                .await
                .expect("backpressured reconstruction result before exclusive clock call");
        };
        let exclusive_clock = async {
            let _ = std::time::Instant::now();
        };
        (stdin, stdin_terminal, tool, exclusive_clock).join().await;
        drop(stdin_writer);
    }

    async fn hold_completed_reconstruction_before_incomplete_custom(&self) {
        let rpc = ToolRpc::new("streaming");
        let (stdout_target, stdout) = tool_host::create_stdout();
        let result = rpc.async_invoke_and_await(
            &["run".to_string()],
            raw_input("historical-reconstruction-gate"),
            Some(raw_stdin(vec![
                b"reconstruction-left".to_vec(),
                b"reconstruction-right".to_vec(),
            ])),
            Some(stdout_target),
        );
        assert!(read_all(stdout).await.is_empty());
        let tool = async {
            raw_result(&result)
                .await
                .expect("completed reconstruction result before custom effect");
        };
        let incomplete_custom = async {
            wait_at_crash_checkpoint("before-reconstruction-custom-effect").await;
            Durability::<(), String>::new(
                "golem-it",
                "reconstruction-barrier-custom-effect",
                DurableFunctionType::WriteRemote,
                &(),
            )
            .run_infallible_async(|| async {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/reconstruction-custom-order.log")
                    .and_then(|mut file| file.write_all(b"C"))
                    .expect("append the first live custom effect after the reconstruction barrier");
                wait_at_crash_checkpoint("reconstruction-custom-effect").await;
            })
            .await;
        };
        (tool, incomplete_custom).join().await;
    }

    async fn principal_context(&self, principal: Principal) -> Vec<String> {
        let caller_class = principal_class(&principal).to_string();
        let invocation = StreamingClient::default()
            .run("nested-principal".to_string(), input_stream(Vec::new()))
            .expect("start nested principal tool");
        let (summary, output) = invocation
            .collect()
            .await
            .expect("collect nested principal tool");
        assert_eq!(summary.bytes_read, 0);
        assert!(!summary.output_closed);
        let provider_classes = String::from_utf8(output).expect("principal classes are UTF-8");
        let (outer_class, nested_class) = provider_classes
            .split_once(':')
            .expect("provider returns outer and nested principal classes");
        assert_eq!(outer_class, caller_class);
        assert_eq!(nested_class, caller_class);
        vec![
            caller_class,
            outer_class.to_string(),
            nested_class.to_string(),
        ]
    }
}
