use anyhow::{Context, ensure};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) struct Request {
    pub number: usize,
    pub socket: usize,
    pub idempotency_key: String,
}

pub(super) struct Peer {
    pub url: String,
    requests: mpsc::UnboundedReceiver<anyhow::Result<Request>>,
    closed: mpsc::UnboundedReceiver<anyhow::Result<usize>>,
    headers_sent: mpsc::UnboundedReceiver<anyhow::Result<usize>>,
    gates: Arc<Mutex<HashMap<usize, oneshot::Sender<()>>>>,
    connections: Arc<AtomicUsize>,
    request_count: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Peer {
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/body-wait", listener.local_addr()?);
        let (request_tx, requests) = mpsc::unbounded_channel();
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let (headers_tx, headers_sent) = mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::new(AtomicUsize::new(0));
        let gates = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let connections = connections.clone();
            let request_count = request_count.clone();
            let gates = gates.clone();
            let shutdown = shutdown.clone();
            async move {
                let mut handlers = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted?;
                            let socket = connections.fetch_add(1, Ordering::SeqCst) + 1;
                            let requests = request_tx.clone();
                            let closed = closed_tx.clone();
                            let headers = headers_tx.clone();
                            let count = request_count.clone();
                            let gates = gates.clone();
                            handlers.spawn(async move {
                                if let Err(error) = serve(stream, socket, &requests, &closed, &headers, &count, &gates).await {
                                    let _ = requests.send(Err(error));
                                }
                            });
                        }
                        result = handlers.join_next(), if !handlers.is_empty() => {
                            result.context("HTTP handler set closed")??;
                        }
                    }
                }
                handlers.abort_all();
                while handlers.join_next().await.is_some() {}
                Ok(())
            }
        });
        Ok(Self {
            url,
            requests,
            closed,
            headers_sent,
            gates,
            connections,
            request_count,
            shutdown,
            task,
        })
    }

    pub async fn request(&mut self) -> anyhow::Result<Request> {
        tokio::time::timeout(Duration::from_secs(10), self.requests.recv())
            .await
            .context("no HTTP request reached the peer")?
            .context("HTTP peer stopped")?
    }

    pub async fn wait_headers(&mut self, expected: usize) -> anyhow::Result<()> {
        let socket = tokio::time::timeout(Duration::from_secs(10), self.headers_sent.recv())
            .await
            .context("HTTP response headers not sent")?
            .context("HTTP header channel stopped")??;
        ensure!(
            socket == expected,
            "expected response headers on socket {expected}, got {socket}"
        );
        Ok(())
    }

    pub fn release(&self, number: usize) -> anyhow::Result<()> {
        self.gates
            .lock()
            .unwrap()
            .remove(&number)
            .context("HTTP peer gate missing")?
            .send(())
            .map_err(|_| anyhow::anyhow!("HTTP peer gate receiver stopped"))
    }

    pub fn release_all(&self) {
        for (_, gate) in self.gates.lock().unwrap().drain() {
            let _ = gate.send(());
        }
    }

    pub async fn wait_closed(&mut self, expected: usize) -> anyhow::Result<()> {
        let socket = tokio::time::timeout(Duration::from_secs(10), self.closed.recv())
            .await
            .context("HTTP socket did not close")?
            .context("HTTP closure channel stopped")??;
        ensure!(
            socket == expected,
            "expected socket {expected} to close, got {socket}"
        );
        Ok(())
    }

    pub fn assert_counts(&mut self, requests: usize, sockets: usize) -> anyhow::Result<()> {
        ensure!(
            self.request_count.load(Ordering::SeqCst) == requests,
            "expected {requests} requests, got {}",
            self.request_count.load(Ordering::SeqCst)
        );
        ensure!(
            self.connections.load(Ordering::SeqCst) == sockets,
            "expected {sockets} sockets, got {}",
            self.connections.load(Ordering::SeqCst)
        );
        ensure!(!self.task.is_finished(), "HTTP peer stopped early");
        ensure!(
            matches!(
                self.requests.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ),
            "unconsumed HTTP request or handler error"
        );
        Ok(())
    }

    pub async fn finish(&mut self) -> anyhow::Result<()> {
        self.release_all();
        self.shutdown.cancel();
        match tokio::time::timeout(Duration::from_secs(5), &mut self.task).await {
            Ok(result) => result?,
            Err(error) => {
                self.task.abort();
                match tokio::time::timeout(Duration::from_secs(5), &mut self.task)
                    .await
                    .context("HTTP peer abort join")?
                {
                    Ok(result) => result?,
                    Err(join_error) if join_error.is_cancelled() => {}
                    Err(join_error) => return Err(join_error.into()),
                }
                Err(error.into())
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.release_all();
        self.shutdown.cancel();
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    socket: usize,
    requests: &mpsc::UnboundedSender<anyhow::Result<Request>>,
    closed: &mpsc::UnboundedSender<anyhow::Result<usize>>,
    headers_sent: &mpsc::UnboundedSender<anyhow::Result<usize>>,
    request_count: &AtomicUsize,
    gates: &Mutex<HashMap<usize, oneshot::Sender<()>>>,
) -> anyhow::Result<()> {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        ensure!(headers.len() < 16 * 1024, "oversized HTTP request");
        headers.push(stream.read_u8().await?);
    }
    let headers = String::from_utf8(headers)?;
    ensure!(
        headers.starts_with("GET /body-wait HTTP/1.1\r\n"),
        "wrong request: {headers}"
    );
    let header_values = |name: &str| -> Vec<String> {
        headers
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_owned())
            })
            .collect()
    };
    let keys = header_values("idempotency-key");
    ensure!(
        keys.len() == 1 && !keys[0].is_empty(),
        "HTTP idempotency-key missing or duplicated"
    );
    let number = request_count.fetch_add(1, Ordering::SeqCst) + 1;
    let ranges = header_values("range");
    match number {
        1 | 3 | 4 => ensure!(ranges.is_empty(), "unexpected Range: {ranges:?}"),
        2 => ensure!(
            ranges == ["bytes=1-"],
            "wrong response resume Range: {ranges:?}"
        ),
        _ => anyhow::bail!("unexpected HTTP attempt {number}"),
    }
    let response = if number <= 2 {
        let (gate, rx) = oneshot::channel();
        gates.lock().unwrap().insert(number, gate);
        Some(rx)
    } else {
        None
    };
    requests.send(Ok(Request {
        number,
        socket,
        idempotency_key: keys[0].clone(),
    }))?;
    match number {
        1 => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nh")
                .await?;
            stream.flush().await?;
            let _ = response.unwrap().await;
            stream.shutdown().await?;
            closed.send(Ok(socket))?;
        }
        2 => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n")
                .await?;
            stream.flush().await?;
            headers_sent.send(Ok(socket))?;
            tokio::select! {
                _ = response.unwrap() => {
                    stream.write_all(b"hi").await?;
                    stream.shutdown().await?;
                }
                result = async {
                    let mut bytes = [0; 1024];
                    loop {
                        match stream.read(&mut bytes).await {
                            Ok(0) => break Ok(()),
                            Ok(_) => {},
                            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break Ok(()),
                            Err(error) => break Err(error),
                        }
                    }
                } => { result?; closed.send(Ok(socket))?; }
            }
        }
        _ => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi")
                .await?;
            stream.shutdown().await?;
        }
    }
    Ok(())
}
