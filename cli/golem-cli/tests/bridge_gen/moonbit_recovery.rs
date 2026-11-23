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

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::ErrorKind;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

const STEP: Duration = Duration::from_secs(15);
const KEY: &str = "00000000-0000-0000-0000-000000000123";

/// Owns the listener, accepted sockets and server thread, including during unwinding.
pub(super) struct RecoveryServer {
    pub port: u16,
    cancel: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<Result<()>>>,
    mode: &'static str,
}

impl RecoveryServer {
    pub fn start(mode: &'static str) -> Self {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (cancel, cancelled) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                let socket = TcpSocket::new_v4()?;
                // Bound the unread start before accept, so the accepted socket inherits it.
                if mode == "start-unread" {
                    socket.set_recv_buffer_size(4096)?;
                }
                socket.bind("127.0.0.1:0".parse()?)?;
                let listener = socket.listen(128)?;
                ready_tx.send(listener.local_addr()?.port())?;
                let server = Peer {
                    listener,
                    first: true,
                };
                tokio::select! {
                    _ = cancelled => Ok(()),
                    result = timeout(Duration::from_secs(660), server.run(mode)) => {
                        result.context("recovery server exceeded its total lifetime")?
                    }
                }
            })
        });
        let mut server = Self {
            port: 0,
            cancel: Some(cancel),
            thread: Some(thread),
            mode,
        };
        server.port = ready_rx
            .recv_timeout(STEP)
            .expect("recovery server did not bind");
        server
    }

    pub fn finish(mut self) {
        let result = self.thread.take().unwrap().join();
        match result {
            Ok(result) => result.unwrap_or_else(|error| panic!("{}: {error:#}", self.mode)),
            Err(error) => std::panic::resume_unwind(error),
        }
    }
}

impl Drop for RecoveryServer {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(thread) = self.thread.take() {
            // Cancellation drops the entire server future and all of its sockets.
            let _ = thread.join();
        }
    }
}

struct Peer {
    listener: TcpListener,
    first: bool,
}

async fn request(conn: &mut TcpStream) -> Result<String> {
    timeout(STEP, async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            anyhow::ensure!(bytes.len() < 65536, "oversized HTTP request");
            bytes.push(conn.read_u8().await?);
        }
        Ok(String::from_utf8(bytes)?)
    })
    .await
    .context("HTTP request timed out")?
}

async fn write(conn: &mut TcpStream, data: &[u8]) -> Result<()> {
    timeout(STEP, conn.write_all(data))
        .await
        .context("socket write timed out")??;
    Ok(())
}

async fn upgrade(mut conn: TcpStream) -> Result<TcpStream> {
    let request = request(&mut conn).await?;
    assert!(
        request.starts_with("GET /v1/runtime/envs/env-id/invoke-agent-session "),
        "{request}"
    );
    let key = request
        .lines()
        .filter_map(|line| line.split_once(": "))
        .find(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key"))
        .context("missing WebSocket key")?
        .1;
    let accept = derive_accept_key(key.as_bytes());
    write(&mut conn, format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: golem.agent-invocation.v1\r\n\r\n").as_bytes()).await?;
    Ok(conn)
}

async fn frame(conn: &mut TcpStream) -> Result<Vec<u8>> {
    timeout(STEP, async {
        let mut header = [0; 2];
        conn.read_exact(&mut header).await?;
        let length = match header[1] & 127 {
            126 => u64::from(conn.read_u16().await?),
            127 => conn.read_u64().await?,
            length => u64::from(length),
        };
        anyhow::ensure!(header[1] & 128 != 0, "client frame is not masked");
        anyhow::ensure!(length <= 16 * 1024 * 1024, "oversized client frame");
        let mut mask = [0; 4];
        conn.read_exact(&mut mask).await?;
        let mut bytes = vec![0; length as usize];
        conn.read_exact(&mut bytes).await?;
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
        Ok(bytes)
    })
    .await
    .context("WebSocket read timed out")?
}

async fn receive(conn: &mut TcpStream) -> Result<Value> {
    Ok(serde_json::from_slice(&frame(conn).await?)?)
}

async fn send_frame(conn: &mut TcpStream, opcode: u8, data: &[u8]) -> Result<()> {
    let mut bytes = vec![0x80 | opcode];
    if data.len() < 126 {
        bytes.push(data.len() as u8);
    } else {
        bytes.push(126);
        bytes.extend_from_slice(&u16::try_from(data.len())?.to_be_bytes());
    }
    bytes.extend_from_slice(data);
    write(conn, &bytes).await
}

async fn send(conn: &mut TcpStream, mut message: Value) -> Result<()> {
    message["version"] = json!(1);
    send_frame(conn, 1, &serde_json::to_vec(&message)?).await
}

fn output(channel: u8) -> Value {
    json!([{"direction":"output", "channel":channel, "streamToken":"output"}])
}

async fn accepted(
    conn: &mut TcpStream,
    operation: &Value,
    session: &str,
    mappings: Value,
) -> Result<()> {
    send(conn, json!({"type":"invocationAccepted", "agentId":"00000000-0000-8000-8000-000000000abc",
        "attemptId":operation["attemptId"], "idempotencyKey":KEY, "sessionToken":session, "mappings":mappings})).await
}

async fn accept_operation(
    conn: &mut TcpStream,
    operation: &Value,
    key: &Value,
    mappings: Value,
) -> Result<()> {
    assert_ne!(key, KEY);
    accepted(conn, operation, "original-session", mappings).await
}

async fn result(conn: &mut TcpStream, mappings: Value) -> Result<()> {
    send(conn, json!({"type":"invocationResult", "mappings":mappings,
        "result":{"kind":"value", "graph":{"root":{"kind":"stream", "value":{"inner":{"kind":"u8", "value":{}}}}},
        "value":{"kind":"stream", "value":{"streamToken":"output"}}}})).await
}

async fn empty_result(conn: &mut TcpStream) -> Result<()> {
    send(conn, json!({"type":"invocationResult", "mappings":[], "result":{"kind":"value",
        "graph":{"root":{"kind":"tuple", "value":{"elements":[]}}}, "value":{"kind":"tuple", "value":{"elements":[]}}}})).await
}

async fn item(
    conn: &mut TcpStream,
    channel: u8,
    sequence: &str,
    cursor: &str,
    value: u8,
) -> Result<()> {
    send(
        conn,
        json!({"type":"outputStreamItem", "channel":channel, "sequence":sequence,
        "cursorToken":cursor, "mappings":[], "value":{"kind":"u8", "value":value}}),
    )
    .await
}

async fn finished(conn: &mut TcpStream) -> Result<()> {
    send(
        conn,
        json!({"type":"invocationFinished", "outcome":{"kind":"success"}}),
    )
    .await
}

async fn ack(conn: &mut TcpStream, channel: u8, terminal: bool) -> Result<()> {
    send(conn, json!({"type":"inputStreamAck", "channel":channel, "highestContiguousSequence":"1", "terminal":terminal, "mappings":[]})).await
}

fn packed_metadata(frame: &[u8]) -> (Value, &[u8]) {
    let size = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&frame[4..4 + size]).unwrap(),
        &frame[4 + size..],
    )
}

fn cancelled(channel: u8) -> Value {
    json!({"version":1, "type":"stream-end", "channel":channel, "direction":"output", "sequence":"0",
        "outcome":{"kind":"consumer-cancelled", "details":"cancelled"}})
}

impl Peer {
    async fn accept_for(&mut self, duration: Duration) -> Result<TcpStream> {
        let conn = timeout(duration, self.listener.accept())
            .await
            .context("accept timed out")??
            .0;
        self.first = false;
        Ok(conn)
    }

    async fn accept(&mut self) -> Result<TcpStream> {
        self.accept_for(if self.first {
            Duration::from_secs(600)
        } else {
            STEP
        })
        .await
    }

    async fn connect(&mut self) -> Result<TcpStream> {
        upgrade(self.accept().await?).await
    }

    async fn refuse(&mut self, status: u16) -> Result<()> {
        let mut conn = self.accept().await?;
        request(&mut conn).await?;
        write(
            &mut conn,
            format!("HTTP/1.1 {status} Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
    }

    async fn terminal(&mut self) -> Result<()> {
        match timeout(Duration::from_millis(500), self.listener.accept()).await {
            Err(_) => Ok(()),
            Ok(Err(error)) => Err(error.into()),
            Ok(Ok(_)) => bail!("terminal rejection caused another attachment attempt"),
        }
    }

    async fn signal(&mut self, path: Option<&str>) -> Result<()> {
        let mut conn = self.accept().await?;
        let request = request(&mut conn).await?;
        if let Some(path) = path {
            assert!(request.starts_with(&format!("GET /{path} ")), "{request}");
        }
        write(
            &mut conn,
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
    }

    async fn finish_cancelled_output(
        &mut self,
        conn: &mut TcpStream,
        channel: u8,
        mode: &str,
    ) -> Result<()> {
        assert_eq!(receive(conn).await?, cancelled(channel));
        if mode == "output-cancel-race" {
            item(conn, channel, "1", "cursor-2", 43).await?;
        }
        if mode != "output-cancel-after-resume-head" {
            send(conn, json!({"type":"stream-end", "channel":channel, "direction":"output",
                "sequence":if mode == "output-cancel-partial-packed" {"3"} else {"2"}, "outcome":{"kind":"finished"}})).await?;
        }
        send(conn, json!({"type":"stream-end", "channel":4, "direction":"output", "sequence":"0", "outcome":{"kind":"finished"}})).await?;
        finished(conn).await?;
        self.signal(Some("cancelled")).await
    }

    async fn run(mut self, mode: &str) -> Result<()> {
        if mode == "start-closed" {
            return Ok(());
        }
        if mode == "start-blackhole" {
            let mut held = vec![self.accept().await?];
            while let Ok(conn) = timeout(Duration::from_secs(3), self.listener.accept()).await {
                held.push(conn?.0);
            }
            assert!(held.len() >= 2, "the client dialed {} times", held.len());
            return Ok(());
        }
        if mode == "start-unanswered" {
            let mut conn = self.connect().await?;
            receive(&mut conn).await?;
            let close = async {
                frame(&mut conn).await?;
                write(&mut conn, b"\x88\x02\x03\xe8").await
            }
            .await;
            match close {
                Ok(()) => {}
                Err(error)
                    if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                        matches!(
                            error.kind(),
                            ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                        )
                    }) => {}
                Err(error) => return Err(error),
            }
            drop(conn);
            return self.terminal().await;
        }
        if mode == "start-unread" {
            let mut conn = self.connect().await?;
            sleep(Duration::from_secs(3)).await;
            let mut received = 0;
            let mut bytes = [0; 65536];
            loop {
                match timeout(STEP, conn.read(&mut bytes)).await {
                    Ok(Ok(0)) | Err(_) => break,
                    Ok(Ok(size)) => received += size,
                    Ok(Err(error)) if error.kind() == ErrorKind::ConnectionReset => break,
                    Ok(Err(error)) => return Err(error.into()),
                }
            }
            assert!(
                received < 8 * 1024 * 1024,
                "the client sent the whole start ({received} bytes)"
            );
            drop(conn);
            return self.terminal().await;
        }
        if mode == "accept-drop" {
            let mut connected = Vec::new();
            let mut key = None;
            loop {
                let duration = if self.first {
                    Duration::from_secs(600)
                } else {
                    Duration::from_secs(3)
                };
                // Only an idle accept ends this scenario; protocol errors must fail it.
                let mut conn = match timeout(duration, self.listener.accept()).await {
                    Err(_) => break,
                    Ok(conn) => conn?.0,
                };
                self.first = false;
                conn = upgrade(conn).await?;
                connected.push(Instant::now());
                let operation = receive(&mut conn).await?;
                let key = key.get_or_insert_with(|| operation["idempotencyKey"].clone());
                accept_operation(&mut conn, &operation, key, json!([])).await?;
            }
            assert!(
                (3..=6).contains(&connected.len()),
                "the client connected {} times",
                connected.len()
            );
            let last_wait = connected[connected.len() - 1] - connected[connected.len() - 2];
            assert!(
                last_wait >= Duration::from_millis(150),
                "the last reconnect waited only {last_wait:?}"
            );
            return Ok(());
        }
        if mode == "stable-reset" {
            let mut conn = self.connect().await?;
            let start = receive(&mut conn).await?;
            let key = &start["idempotencyKey"];
            accept_operation(&mut conn, &start, key, json!([])).await?;
            result(&mut conn, output(2)).await?;
            item(&mut conn, 2, "0", "cursor-1", 42).await?;
            self.signal(None).await?;
            sleep(Duration::from_millis(500)).await;
            drop(conn);
            let started = Instant::now();
            for _ in 0..3 {
                let lost = Instant::now();
                let mut conn = self.connect().await?;
                let wait = lost.elapsed();
                assert!(
                    wait < Duration::from_millis(100),
                    "a reconnect after a stable connection waited {wait:?}"
                );
                let resume = receive(&mut conn).await?;
                assert_eq!(resume["operation"], "resume");
                accept_operation(&mut conn, &resume, key, output(2)).await?;
                sleep(Duration::from_millis(500)).await;
            }
            let lost = Instant::now();
            let mut conn = self.connect().await?;
            let wait = lost.elapsed();
            assert!(
                wait < Duration::from_millis(100),
                "a reconnect after a stable connection waited {wait:?}"
            );
            let resume = receive(&mut conn).await?;
            accept_operation(&mut conn, &resume, key, output(2)).await?;
            item(&mut conn, 2, "1", "cursor-2", 43).await?;
            send(&mut conn, json!({"type":"stream-end", "channel":2, "direction":"output", "sequence":"2", "cursorToken":"cursor-end", "outcome":{"kind":"finished"}})).await?;
            finished(&mut conn).await?;
            assert!(started.elapsed() >= Duration::from_secs(1));
            return Ok(());
        }
        if matches!(
            mode,
            "input-prefetch-delayed" | "input-prefetch-before-attachment"
        ) {
            let mut conn = self.connect().await?;
            let start = receive(&mut conn).await?;
            accepted(&mut conn, &start, "prefetch-session", json!([{"direction":"input", "channel":1, "streamToken":"input",
                "provisionalRef":start["methodParameters"]["value"]["provisionalRef"], "inputHighWater":{"sequence":"0", "terminal":false}}])).await?;
            let bytes = frame(&mut conn).await?;
            let (metadata, payload) = packed_metadata(&bytes);
            assert_eq!(metadata["kind"], "input-u8");
            assert_eq!(metadata["sequence"], "0");
            assert_eq!(payload, [42]);
            empty_result(&mut conn).await?;
            // Input cannot advance until the prefix receives its ACK.
            self.signal(Some("diagnostic-admitted")).await?;
            ack(&mut conn, 1, false).await?;
            let terminal = receive(&mut conn).await?;
            assert_eq!(terminal["type"], "stream-end");
            assert_eq!(terminal["sequence"], "1");
            assert_eq!(terminal["outcome"]["error"]["kind"], "interrupted");
            assert!(
                terminal["outcome"]["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("delayed 界🦄 diagnostic")
            );
            ack(&mut conn, 1, true).await?;
            return finished(&mut conn).await;
        }
        if matches!(mode, "input-failure-replay" | "input-eof-replay") {
            let mut conn = self.connect().await?;
            let start = receive(&mut conn).await?;
            accepted(&mut conn, &start, "input-session", json!([{"direction":"input", "channel":1, "streamToken":"input",
                "provisionalRef":start["methodParameters"]["value"]["provisionalRef"], "inputHighWater":{"sequence":"0", "terminal":false}}])).await?;
            let item = receive(&mut conn).await?;
            assert_eq!(item["type"], "inputStreamItem");
            assert_eq!(item["sequence"], "0");
            ack(&mut conn, 1, false).await?;
            let terminal = receive(&mut conn).await?;
            assert_eq!(terminal["type"], "stream-end");
            assert_eq!(terminal["direction"], "input");
            assert_eq!(terminal["sequence"], "1");
            if mode == "input-failure-replay" {
                assert_eq!(terminal["outcome"]["kind"], "failed");
                assert_eq!(terminal["outcome"]["error"]["kind"], "interrupted");
                assert!(
                    terminal["outcome"]["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("source diagnostic")
                );
            } else {
                assert_eq!(terminal["outcome"], json!({"kind":"finished"}));
            }
            drop(conn);
            for channel in [5, 6] {
                let mut conn = self.connect().await?;
                let resume = receive(&mut conn).await?;
                assert_eq!(resume["type"], "resumeAttach");
                assert_eq!(resume["operation"], "resume");
                accepted(
                    &mut conn,
                    &resume,
                    "input-session",
                    json!([{"direction":"input", "channel":channel, "streamToken":"input",
                    "inputHighWater":{"sequence":"1", "terminal":false}}]),
                )
                .await?;
                let mut expected = terminal.clone();
                expected["channel"] = json!(channel);
                assert_eq!(receive(&mut conn).await?, expected);
                ack(&mut conn, channel, channel == 6).await?;
                if channel == 6 {
                    empty_result(&mut conn).await?;
                    finished(&mut conn).await?;
                }
            }
            return Ok(());
        }
        if mode == "start-503" {
            self.refuse(503).await?;
            self.refuse(429).await?;
        }
        if mode == "start-no-limit" {
            for _ in 0..5 {
                self.refuse(503).await?;
            }
        }
        let mut first_start = None;
        if mode == "start-retryable" {
            let mut conn = self.connect().await?;
            let start = receive(&mut conn).await?;
            send(&mut conn, json!({"type":"invocationRejected", "attemptId":start["attemptId"], "code":"routing-miss", "message":"moving", "retryable":true})).await?;
            first_start = Some(start);
        }
        let mut conn = self.connect().await?;
        let start = receive(&mut conn).await?;
        assert_eq!(start["type"], "invocationStart");
        if let Some(first) = first_start {
            assert_ne!(start["attemptId"], first["attemptId"]);
            assert_eq!(start["idempotencyKey"], first["idempotencyKey"]);
            assert_eq!(start["methodParameters"], first["methodParameters"]);
        }
        if mode == "initial" {
            send(&mut conn, json!({"type":"invocationRejected", "attemptId":start["attemptId"], "code":"invalid-attachment-state", "message":"terminal rejection", "retryable":false})).await?;
            return self.terminal().await;
        }
        accepted(&mut conn, &start, "original-session", json!([])).await?;
        let cancelling = matches!(
            mode,
            "output-cancel-race"
                | "output-cancel-partial-packed"
                | "output-cancel-after-resume-head"
        );
        let mut mappings = output(2);
        if cancelling {
            mappings
                .as_array_mut()
                .unwrap()
                .push(json!({"direction":"output", "channel":4, "streamToken":"sibling"}));
        }
        result(&mut conn, mappings).await?;
        if mode == "output-cancel-partial-packed" {
            let metadata = serde_json::to_vec(
                &json!({"version":1, "kind":"output-u8", "channel":2, "sequence":"0", "itemCount":"3", "cursorToken":"cursor-packed"}),
            )?;
            let mut data = (metadata.len() as u32).to_be_bytes().to_vec();
            data.extend_from_slice(&metadata);
            data.extend_from_slice(&[42, 43, 44]);
            send_frame(&mut conn, 2, &data).await?;
        } else {
            item(&mut conn, 2, "0", "cursor-1", 42).await?;
        }
        // The client signals only after consuming the item and storing its cursor.
        self.signal(Some("consumed")).await?;
        if mode == "output-admission-cancel" {
            for channel in [11, 12] {
                assert_eq!(receive(&mut conn).await?, cancelled(channel));
            }
            return self.signal(Some("audit-done")).await;
        }
        if mode == "terminal-accounting" {
            for prefix in [true, false, true, false, true] {
                let bytes = frame(&mut conn).await?;
                if prefix {
                    let (metadata, payload) = packed_metadata(&bytes);
                    assert_eq!(metadata["kind"], "input-u8");
                    assert_eq!(payload, [42]);
                } else {
                    let message: Value = serde_json::from_slice(&bytes)?;
                    assert_eq!(message["type"], "stream-end");
                    assert_eq!(message["outcome"]["error"]["kind"], "interrupted");
                    assert!(
                        message["outcome"]["error"]["message"]
                            .as_str()
                            .unwrap()
                            .contains('界')
                    );
                }
            }
            return self.signal(Some("audit-done")).await;
        }
        if cancelling {
            if mode == "output-cancel-after-resume-head" {
                drop(conn);
                let mut resumed = self.connect().await?;
                let resume = receive(&mut resumed).await?;
                assert_eq!(resume["operation"], "resume");
                assert_eq!(resume["outputCursors"], json!(["cursor-1"]));
                accepted(&mut resumed, &resume, "new-session", json!([{"direction":"output", "channel":3, "streamToken":"output"}, {"direction":"output", "channel":4, "streamToken":"sibling"}])).await?;
                return self.finish_cancelled_output(&mut resumed, 3, mode).await;
            }
            return self.finish_cancelled_output(&mut conn, 2, mode).await;
        }
        if mode == "window-reset" {
            sleep(Duration::from_millis(1500)).await;
        }
        drop(conn);
        if mode == "resume-unreachable" {
            return Ok(());
        }
        let mut rejected_resume = None;
        if mode == "resume-routing-miss" {
            let mut conn = self.connect().await?;
            let resume = receive(&mut conn).await?;
            assert_eq!(resume["operation"], "resume");
            send(&mut conn, json!({"type":"invocationRejected", "attemptId":resume["attemptId"], "code":"routing-miss", "message":"moving", "retryable":true})).await?;
            rejected_resume = Some(resume);
        }
        let mut conn = self.connect().await?;
        let frozen_resume = frame(&mut conn).await?;
        let resume: Value = serde_json::from_slice(&frozen_resume)?;
        assert_eq!(resume["operation"], "resume");
        if let Some(rejected) = rejected_resume {
            assert_ne!(resume["attemptId"], rejected["attemptId"]);
        }
        assert_eq!(resume["sessionToken"], "original-session");
        assert_eq!(resume["outputCursors"], json!(["cursor-1"]));
        drop(conn);
        let mut conn = self.connect().await?;
        assert_eq!(frame(&mut conn).await?, frozen_resume);
        send(&mut conn, json!({"type":"invocationRejected",
            "attemptId":if mode == "mismatch" {json!("wrong-attempt")} else {resume["attemptId"].clone()},
            "code":if mode == "other" {"token-invalid"} else {"invalid-attachment-state"},
            "message":"terminal rejection", "retryable":mode == "resume-retryable"})).await?;
        if matches!(mode, "mismatch" | "other") {
            return self.terminal().await;
        }
        drop(conn);
        let mut conn = self.connect().await?;
        let frozen_takeover = frame(&mut conn).await?;
        let takeover: Value = serde_json::from_slice(&frozen_takeover)?;
        let mut expected = resume.clone();
        expected["operation"] = json!("takeover");
        expected["attemptId"] = takeover["attemptId"].clone();
        assert_eq!(takeover, expected);
        assert_ne!(takeover["attemptId"], resume["attemptId"]);
        if matches!(mode, "second" | "second-retryable" | "takeover-mismatch") {
            send(&mut conn, json!({"type":"invocationRejected",
                "attemptId":if mode == "takeover-mismatch" {json!("wrong-attempt")} else {takeover["attemptId"].clone()},
                "code":"invalid-attachment-state", "message":"terminal rejection", "retryable":mode == "second-retryable"})).await?;
            return self.terminal().await;
        }
        drop(conn);
        let mut conn = self.connect().await?;
        assert_eq!(frame(&mut conn).await?, frozen_takeover);
        accepted(&mut conn, &takeover, "new-session", output(3)).await?;
        item(&mut conn, 3, "1", "cursor-2", 43).await?;
        let outcome = match mode {
            "output-failed" => {
                json!({"kind":"failed", "error":{"kind":"failed", "value":{"graph":{"root":{"kind":"string", "value":{}}}, "value":{"kind":"string", "value":"application failure"}}}})
            }
            "output-consumer-cancelled" => {
                json!({"kind":"consumer-cancelled", "details":"server consumer stopped"})
            }
            _ => json!({"kind":"finished"}),
        };
        send(&mut conn, json!({"type":"stream-end", "channel":3, "direction":"output", "sequence":"2", "cursorToken":"cursor-end", "outcome":outcome})).await?;
        finished(&mut conn).await
    }
}

#[test_r::test]
#[test_r::timeout("5s")]
async fn recovery_server_drop_cancels_pending_accept() {
    tokio::task::spawn_blocking(|| {
        let server = RecoveryServer::start("success");
        let port = server.port;
        drop(server);
        assert!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err());
    })
    .await
    .unwrap();
}

#[test_r::test]
#[test_r::timeout("5s")]
async fn recovery_server_unwind_closes_pending_frame_read() {
    use std::io::{Read, Write};

    tokio::task::spawn_blocking(|| {
        let server = RecoveryServer::start("success");
        let port = server.port;
        let mut conn = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        conn.set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        conn.write_all(b"GET /v1/runtime/envs/env-id/invoke-agent-session HTTP/1.1\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            conn.read_exact(&mut byte).unwrap();
            response.push(byte[0]);
        }
        assert!(response.starts_with(b"HTTP/1.1 101 "));
        conn.write_all(&[0x81]).unwrap();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _server = server;
            panic!("native client failure");
        }));
        assert!(failure.is_err());
        match conn.read(&mut [0]) {
            Ok(0) => {}
            Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
            other => panic!("server did not close the pending frame read: {other:?}"),
        }
        assert!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err());
    })
    .await
    .unwrap();
}
