use golem_rust::agentic::{InputStream, OutputStream, Principal, pump_tool_stdin, spawn_local};
use golem_rust::golem_agentic::golem::tool::host::{self as tool_host, ByteStreamFailure, ToolRpc};
use golem_rust::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, ToolError, tool_definition, tool_implementation,
};
use wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

const MARKER: &[u8] = b"marker:";

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub struct StreamSummary {
    pub chunks_read: u32,
    pub bytes_read: u64,
    pub output_closed: bool,
}

#[derive(Debug, Clone, ToolError)]
pub enum StreamingError {
    #[tool_error(kind = "runtime-error", exit_code = 7)]
    Declared { bytes_read: u64 },
}

#[derive(IntoSchema)]
struct RawRunInput {
    mode: String,
}

#[derive(IntoSchema)]
struct RawCapableInput {
    path: String,
}

#[tool_definition(version = "1.0.0")]
pub trait Streaming {
    async fn run(
        &self,
        mode: String,
        stdin: InputStream,
        stdout: OutputStream,
        principal: golem_rust::agentic::Principal,
    ) -> Result<StreamSummary, StreamingError>;

    async fn no_stream(&self, value: String) -> Result<String, StreamingError>;

    async fn optional_streams(
        &self,
        stdin: Option<InputStream>,
        stdout: Option<OutputStream>,
    ) -> Result<StreamSummary, StreamingError>;
}

#[tool_definition(version = "1.0.0")]
pub trait CapableStreaming {
    async fn run_capable(
        &self,
        path: String,
        stdin: InputStream,
        stdout: OutputStream,
    ) -> Result<StreamSummary, StreamingError>;
}

struct StreamingImpl;

async fn write_chunk(stdout: &mut OutputStream, chunk: Vec<u8>) -> bool {
    stdout.write(chunk).await.is_ok()
}

async fn stream_through_http(
    mode: String,
    mut stdin: InputStream,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("HTTP_GATE_PORT").expect("HTTP_GATE_PORT is configured");
    let tag = mode
        .strip_prefix("http-")
        .expect("HTTP streaming mode has a tag");
    let headers =
        types::Fields::from_list(&[("x-stream-tag".to_string(), tag.as_bytes().to_vec())])
            .expect("valid HTTP fields");
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set HTTP method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set HTTP scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set HTTP authority");
    request
        .set_path_with_query(Some(&format!("/{tag}")))
        .expect("set HTTP path");

    let upload = async move {
        let mut chunks_read = 0;
        let mut bytes_read = 0;
        while let Some(item) = stdin.next().await {
            let chunk = item.expect("tool stdin failed during HTTP upload");
            chunks_read += 1;
            bytes_read += chunk.len() as u64;
            assert!(
                body_tx.write_all(chunk).await.is_empty(),
                "HTTP request body closed before tool stdin"
            );
        }
        drop(body_tx);
        trailers_tx
            .write(Ok(None))
            .await
            .expect("finish HTTP request trailers");
        (chunks_read, bytes_read)
    };
    let download = async move {
        let response = client::send(request).await.expect("send HTTP request");
        assert_eq!(response.get_status_code(), 200);
        let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
        let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
        let mut buffer = Vec::with_capacity(4096);
        let mut output_closed = false;
        loop {
            let (result, next_buffer) = body.read(buffer).await;
            buffer = next_buffer;
            match result {
                StreamResult::Complete(len) => {
                    if stdout.write(buffer[..len].to_vec()).await.is_err() {
                        output_closed = true;
                        break;
                    }
                    buffer.clear();
                }
                StreamResult::Dropped => break,
                StreamResult::Cancelled => panic!("HTTP response body read was cancelled"),
            }
        }
        drop(body);
        trailers.await.expect("read HTTP response trailers");
        response_done_tx
            .write(Ok(()))
            .await
            .expect("acknowledge HTTP response body");
        let _ = stdout.finish().await;
        output_closed
    };
    let finish_transmit = async move {
        transmit.await.expect("transmit HTTP request body");
    };

    let ((chunks_read, bytes_read), output_closed, ()) =
        (upload, download, finish_transmit).join().await;
    Ok(StreamSummary {
        chunks_read,
        bytes_read,
        output_closed,
    })
}

fn raw_run_input(mode: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawRunInput {
        mode: mode.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode nested tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode nested tool wire input")
}

fn raw_capable_input(path: &str) -> golem_rust::schema::wit::wire::TypedSchemaValue {
    let value = RawCapableInput {
        path: path.to_string(),
    }
    .into_typed_schema_value()
    .expect("encode nested capable tool input");
    golem_rust::encode_typed_schema_value(&value).expect("encode nested capable tool wire input")
}

fn nested_input(bytes: Vec<u8>) -> InputStream {
    let (mut writer, reader) =
        golem_rust::golem_agentic::wit_stream::new::<Result<Vec<u8>, ByteStreamFailure>>();
    spawn_local(async move {
        if !bytes.is_empty() {
            let _ = writer.write_all(vec![Ok(bytes)]).await;
        }
    });
    reader
}

fn launch_retained_crash_child() {
    ToolRpc::new("streaming")
        .invoke(
            &["run".to_string()],
            raw_run_input("hold-capable-terminal-child"),
            Some(pump_tool_stdin(nested_input(Vec::new()))),
        )
        .expect("launch retained incapable crash-checkpoint child");
}

fn principal_class(principal: &Principal) -> &'static str {
    match principal {
        Principal::Anonymous => "anonymous",
        Principal::Oidc(_) => "oidc",
        Principal::Agent(_) => "agent",
        Principal::GolemUser(_) => "golem-user",
    }
}

async fn run_nested_principal(
    principal: &Principal,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    use futures_concurrency::prelude::*;

    let outer_class = principal_class(principal);
    let rpc = ToolRpc::new("streaming");
    let (nested_target, nested_stdout) = tool_host::create_stdout();
    let nested = rpc.invoke_and_await(
        vec!["run".to_string()],
        raw_run_input("principal"),
        Some(pump_tool_stdin(nested_input(Vec::new()))),
        Some(nested_target),
    );
    let (nested_result, nested_output) = (nested, async move {
        let mut output = Vec::new();
        let mut nested_stdout = nested_stdout;
        while let Some(item) = nested_stdout.next().await {
            output.extend(item.expect("nested principal stdout failed"));
        }
        output
    })
        .join()
        .await;
    nested_result.expect("nested principal tool result");
    let nested_class = String::from_utf8(nested_output).expect("principal class is UTF-8");
    assert_eq!(nested_class, outer_class);
    let output_closed = !write_chunk(
        &mut stdout,
        format!("{outer_class}:{nested_class}").into_bytes(),
    )
    .await;
    let _ = stdout.finish().await;
    Ok(StreamSummary {
        chunks_read: 0,
        bytes_read: 0,
        output_closed,
    })
}

async fn run_nested_capable(bytes: Vec<u8>) -> Vec<u8> {
    use futures_concurrency::prelude::*;

    let rpc = ToolRpc::new("capable-streaming");
    let (stdout_target, nested_stdout) = tool_host::create_stdout();
    let nested = rpc.invoke_and_await(
        vec!["run-capable".to_string()],
        raw_capable_input("order:N:/capable-nested-inner.bin"),
        Some(pump_tool_stdin(nested_input(bytes))),
        Some(stdout_target),
    );
    let (result, output) = (nested, async move {
        let mut stdout = nested_stdout;
        let mut output = Vec::new();
        while let Some(item) = stdout.next().await {
            output.extend(item.expect("nested capable stdout failed"));
        }
        output
    })
        .join()
        .await;
    result.expect("nested capable result");
    output
}

async fn run_nested(
    stdin: InputStream,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    use futures_concurrency::prelude::*;

    let rpc = ToolRpc::new("streaming");
    let (nested_target, mut nested_stdout) = tool_host::create_stdout();
    let nested = rpc.invoke_and_await(
        vec!["run".to_string()],
        raw_run_input("marker-echo"),
        Some(golem_rust::agentic::pump_tool_stdin(stdin)),
        Some(nested_target),
    );
    let forward = async move {
        let mut chunks_read = 0;
        let mut bytes_read = 0;
        let mut output_closed = false;
        while let Some(item) = nested_stdout.next().await {
            let chunk = item.expect("nested tool stdout failed");
            chunks_read += 1;
            bytes_read += chunk.len() as u64;
            if stdout.write(chunk).await.is_err() {
                output_closed = true;
                break;
            }
        }
        let _ = stdout.finish().await;
        (chunks_read, bytes_read, output_closed)
    };
    let (nested_result, (chunks_read, bytes_read, output_closed)) = (nested, forward).join().await;
    nested_result.expect("nested streaming tool result");
    Ok(StreamSummary {
        chunks_read,
        bytes_read,
        output_closed,
    })
}

async fn run_nested_capable_parent_end(
    stdin: InputStream,
    mut stdout: OutputStream,
) -> Result<StreamSummary, StreamingError> {
    let rpc = ToolRpc::new("capable-streaming");
    let (nested_target, nested_stdout) = tool_host::create_stdout();
    let nested = rpc.async_invoke_and_await(
        &["run-capable".to_string()],
        raw_capable_input("/nested-capable-parent-end.bin"),
        Some(golem_rust::agentic::pump_tool_stdin(stdin)),
        Some(nested_target),
    );
    drop(nested);
    drop(nested_stdout);
    let output_closed = !write_chunk(&mut stdout, b"nested-capable-started".to_vec()).await;
    Ok(StreamSummary {
        chunks_read: 0,
        bytes_read: 0,
        output_closed,
    })
}

fn write_owner_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    let (root, _) = wasi::filesystem::preopens::get_directories()
        .into_iter()
        .next()
        .ok_or_else(|| "capable tool has no preopened owner filesystem".to_string())?;
    let file = root
        .open_at(
            PathFlags::empty(),
            path.trim_start_matches('/'),
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
            DescriptorFlags::WRITE,
        )
        .map_err(|error| format!("failed to open owner file: {error:?}"))?;
    let stream = file
        .write_via_stream(0)
        .map_err(|error| format!("failed to open owner file stream: {error:?}"))?;
    stream
        .blocking_write_and_flush(bytes)
        .map_err(|error| format!("failed to write owner file: {error:?}"))
}

async fn is_first_trap_attempt() -> bool {
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
    };
    use golem_rust::wasip3::wit_bindgen::StreamResult;

    let port = std::env::var("TRAP_ONCE_PORT")
        .expect("TRAP_ONCE_PORT is configured")
        .parse()
        .expect("TRAP_ONCE_PORT is a valid port");
    let socket = TcpSocket::create(IpAddressFamily::Ipv4).expect("create trap-attempt socket");
    socket
        .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            address: (127, 0, 0, 1),
            port,
        }))
        .await
        .expect("connect to trap-attempt server");
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
            StreamResult::Cancelled => panic!("trap-attempt read was cancelled"),
        }
    }
    drop(stream);
    received.await.expect("finish trap-attempt receive");
    assert_eq!(bytes.len(), 1, "trap-attempt server returns one byte");
    bytes[0] == 0
}

async fn wait_at_crash_checkpoint<T>(_retained: &T, name: &str) {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
    };
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("PROVIDER_CRASH_CHECKPOINT_PORT")
        .or_else(|_| std::env::var("CRASH_CHECKPOINT_PORT"))
        .expect("provider crash checkpoint port is configured");
    let gate_port = std::env::var("PROVIDER_CRASH_CHECKPOINT_GATE_PORT")
        .or_else(|_| std::env::var("CRASH_CHECKPOINT_GATE_PORT"))
        .expect("provider crash checkpoint gate port is configured")
        .parse()
        .expect("provider crash checkpoint gate port is a valid port");
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

fn append_owner_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    let (root, _) = wasi::filesystem::preopens::get_directories()
        .into_iter()
        .next()
        .ok_or_else(|| "capable tool has no preopened owner filesystem".to_string())?;
    let file = root
        .open_at(
            PathFlags::empty(),
            path.trim_start_matches('/'),
            OpenFlags::CREATE,
            DescriptorFlags::READ | DescriptorFlags::WRITE,
        )
        .map_err(|error| format!("failed to open owner append file: {error:?}"))?;
    let offset = file
        .stat()
        .map_err(|error| format!("failed to stat owner append file: {error:?}"))?
        .size;
    let stream = file
        .write_via_stream(offset)
        .map_err(|error| format!("failed to open owner append stream: {error:?}"))?;
    stream
        .blocking_write_and_flush(bytes)
        .map_err(|error| format!("failed to append owner file: {error:?}"))
}

#[tool_implementation]
impl Streaming for StreamingImpl {
    async fn run(
        &self,
        mode: String,
        mut stdin: InputStream,
        mut stdout: OutputStream,
        principal: golem_rust::agentic::Principal,
    ) -> Result<StreamSummary, StreamingError> {
        if mode.starts_with("http-") {
            return stream_through_http(mode, stdin, stdout).await;
        }
        if mode == "principal" {
            let output_closed =
                !write_chunk(&mut stdout, principal_class(&principal).as_bytes().to_vec()).await;
            let _ = stdout.finish().await;
            return Ok(StreamSummary {
                chunks_read: 0,
                bytes_read: 0,
                output_closed,
            });
        }
        if mode == "nested-principal" {
            return run_nested_principal(&principal, stdout).await;
        }
        if mode == "nested" {
            return run_nested(stdin, stdout).await;
        }
        if mode == "nested-capable-parent-end" {
            return run_nested_capable_parent_end(stdin, stdout).await;
        }

        let mut summary = StreamSummary {
            chunks_read: 0,
            bytes_read: 0,
            output_closed: false,
        };

        if matches!(
            mode.as_str(),
            "marker-echo" | "trap" | "trap-after-clean-eof" | "declared-error"
        ) {
            summary.output_closed = !write_chunk(&mut stdout, MARKER.to_vec()).await;
        }

        match mode.as_str() {
            "empty" => {}
            "binary" => {
                summary.output_closed =
                    !write_chunk(&mut stdout, vec![0, 255, 1, 128, 0, 13, 10, 254]).await;
            }
            "fragmented" => {
                for byte in [b'f', b'r', b'a', b'g', 0, 255] {
                    if !write_chunk(&mut stdout, vec![byte]).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
            "large" => {
                for index in 0..512_u32 {
                    let chunk = vec![(index % 251) as u8; 4096];
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
            "backpressure" => {
                for index in 0..8192_u32 {
                    let chunk = vec![(index % 251) as u8; 4096];
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
            "early-stdin-close" => {
                summary.output_closed = !write_chunk(&mut stdout, b"stdin-ignored".to_vec()).await;
            }
            "early-stdout-close" => {
                let _ = stdout.finish().await;
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                return Ok(summary);
            }
            "hold-after-eof" => {
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.write(b"eof-observed".to_vec()).await;
                wait_at_crash_checkpoint(&stdout, "after-eof-before-terminal").await;
            }
            "hold-after-stdout-terminal" => {
                let _ = stdout.write(b"ready".to_vec()).await;
                if let Some(Ok(chunk)) = stdin.next().await {
                    summary.chunks_read += 1;
                    summary.bytes_read += chunk.len() as u64;
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                    }
                }
                let _ = stdout.finish().await;
                while stdin.next().await.is_some() {}
                return Ok(summary);
            }
            "hold-capable-terminal-child" => {
                let _ = golem_rust::generate_idempotency_key();
                wait_at_crash_checkpoint(&stdout, "capable-terminal-retained-child").await;
            }
            "historical-reconstruction-gate" => {
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.finish().await;
                wait_at_crash_checkpoint(&summary, "historical-reconstruction-body").await;
                return Ok(summary);
            }
            "historical-reconstruction-exclusive" => {
                while let Some(item) = stdin.next().await {
                    if let Ok(chunk) = item {
                        summary.chunks_read += 1;
                        summary.bytes_read += chunk.len() as u64;
                    }
                }
                let _ = stdout.finish().await;
                return Ok(summary);
            }
            "historical-reconstruction-backpressure" => {
                for expected in [vec![0x31; 64], vec![0x32; 64]] {
                    let chunk = stdin
                        .next()
                        .await
                        .expect("backpressured reconstruction stdin ended early")
                        .expect("backpressured reconstruction stdin failed");
                    assert_eq!(chunk, expected);
                    summary.chunks_read += 1;
                    summary.bytes_read += chunk.len() as u64;
                }
                drop(stdin);
                let _ = stdout.finish().await;
                return Ok(summary);
            }
            _ => {
                while let Some(item) = stdin.next().await {
                    let Ok(chunk) = item else {
                        break;
                    };
                    summary.chunks_read += 1;
                    summary.bytes_read += chunk.len() as u64;
                    if !write_chunk(&mut stdout, chunk).await {
                        summary.output_closed = true;
                        break;
                    }
                }
            }
        }

        if mode == "declared-error" {
            let _ = stdout.finish().await;
            return Err(StreamingError::Declared {
                bytes_read: summary.bytes_read,
            });
        }
        if mode == "trap" {
            panic!("deterministic streaming tool trap");
        }
        if mode == "trap-after-clean-eof" {
            let _ = stdout.finish().await;
            wait_at_crash_checkpoint(&(), "provider-clean-stdout-before-trap").await;
            panic!("deterministic streaming tool trap after clean stdout");
        }

        let _ = stdout.finish().await;
        Ok(summary)
    }

    async fn no_stream(&self, value: String) -> Result<String, StreamingError> {
        Ok(format!("no-stream:{value}"))
    }

    async fn optional_streams(
        &self,
        mut stdin: Option<InputStream>,
        mut stdout: Option<OutputStream>,
    ) -> Result<StreamSummary, StreamingError> {
        let mut summary = StreamSummary {
            chunks_read: 0,
            bytes_read: 0,
            output_closed: false,
        };
        if let Some(stdin) = stdin.as_mut() {
            while let Some(item) = stdin.next().await {
                let Ok(chunk) = item else {
                    break;
                };
                summary.chunks_read += 1;
                summary.bytes_read += chunk.len() as u64;
                if let Some(stdout) = stdout.as_mut()
                    && stdout.write(chunk).await.is_err()
                {
                    summary.output_closed = true;
                    break;
                }
            }
        }
        if let Some(stdout) = stdout {
            let _ = stdout.finish().await;
        }
        Ok(summary)
    }
}

struct CapableStreamingImpl;

#[tool_implementation]
impl CapableStreaming for CapableStreamingImpl {
    async fn run_capable(
        &self,
        path: String,
        mut stdin: InputStream,
        mut stdout: OutputStream,
    ) -> Result<StreamSummary, StreamingError> {
        let mut bytes = Vec::new();
        let mut chunks_read = 0;
        while let Some(item) = stdin.next().await {
            let Ok(chunk) = item else {
                break;
            };
            chunks_read += 1;
            bytes.extend(chunk);
        }

        let output = if let Some(rest) = path.strip_prefix("order:") {
            let (tag, path) = rest
                .split_once(':')
                .expect("ordered capable path contains a tag and file path");
            write_owner_file(path, &bytes).expect("capable tool must share the owner filesystem");
            append_owner_file("/capable-order.log", tag.as_bytes())
                .expect("append capable execution order");
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("nested-capable:") {
            let nested_output = run_nested_capable(bytes.clone()).await;
            assert_eq!(nested_output, bytes);
            write_owner_file(path, &nested_output)
                .expect("outer nested capable tool must share the owner filesystem");
            append_owner_file("/capable-order.log", b"O")
                .expect("append outer nested capable execution order");
            nested_output
        } else if let Some(path) = path.strip_prefix("stdout-exact:") {
            write_owner_file(path, &bytes).expect("capable tool must share the owner filesystem");
            vec![b'x'; 64]
        } else if let Some(path) = path.strip_prefix("stdout-over:") {
            write_owner_file(path, &bytes).expect("capable tool must share the owner filesystem");
            vec![b'x'; 65]
        } else if let Some(path) = path.strip_prefix("trap-once:") {
            write_owner_file(path, &bytes)
                .expect("capable trap marker must share the owner filesystem");
            let _effect = golem_rust::generate_idempotency_key();
            golem_rust::atomically_async(|| async {
                if is_first_trap_attempt().await {
                    panic!("deterministic capable streaming tool first-attempt trap");
                }
            })
            .await;
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("hold-body:") {
            write_owner_file(path, &bytes)
                .expect("capable body checkpoint must share the owner filesystem");
            let _ = golem_rust::get_oplog_index();
            stdout
                .write(b"body-checkpoint".to_vec())
                .await
                .expect("publish capable body checkpoint");
            wait_at_crash_checkpoint(&stdout, "capable-body").await;
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("hold-completion:") {
            write_owner_file(path, &bytes)
                .expect("capable completion checkpoint must share the owner filesystem");
            stdout
                .write(bytes.clone())
                .await
                .expect("buffer capable output before the checkpoint");
            append_owner_file(path, b":buffered")
                .expect("record buffered capable completion checkpoint");
            let _ = golem_rust::get_oplog_index();
            stdout
                .write(b"completion-checkpoint".to_vec())
                .await
                .expect("publish capable completion checkpoint");
            wait_at_crash_checkpoint(&stdout, "capable-completion").await;
            bytes.clone()
        } else if let Some(path) = path.strip_prefix("hold-publication:") {
            write_owner_file(path, &bytes)
                .expect("capable publication checkpoint must share the owner filesystem");
            stdout
                .write(bytes.clone())
                .await
                .expect("buffer capable output before lane return");
            wait_at_crash_checkpoint(&stdout, "provider-capable-before-publication").await;
            Vec::new()
        } else if let Some(path) = path.strip_prefix("hold-terminal:") {
            write_owner_file(path, &bytes)
                .expect("capable terminal checkpoint must share the owner filesystem");
            launch_retained_crash_child();
            bytes.clone()
        } else {
            write_owner_file(&path, &bytes).expect("capable tool must share the owner filesystem");
            bytes.clone()
        };
        let output_closed = stdout.write(output).await.is_err();
        let _ = stdout.finish().await;
        Ok(StreamSummary {
            chunks_read,
            bytes_read: bytes.len() as u64,
            output_closed,
        })
    }
}
