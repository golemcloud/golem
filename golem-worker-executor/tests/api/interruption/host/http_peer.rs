use anyhow::{Context, ensure};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) struct Request {
    pub number: usize,
    pub idempotency_key: String,
    pub respond: oneshot::Sender<()>,
}

pub(super) struct Peer {
    pub url: String,
    requests: mpsc::UnboundedReceiver<anyhow::Result<Request>>,
    closed: mpsc::UnboundedReceiver<anyhow::Result<usize>>,
    connections: Arc<AtomicUsize>,
    request_count: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Peer {
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_body_wait(false).await
    }

    pub async fn start_body_wait() -> anyhow::Result<Self> {
        Self::start_with_body_wait(true).await
    }

    async fn start_with_body_wait(body_wait: bool) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let path = if body_wait {
            "/body-wait"
        } else {
            "/header-wait"
        };
        let url = format!("http://{}{path}", listener.local_addr()?);
        let (request_tx, requests) = mpsc::unbounded_channel();
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let connections = connections.clone();
            let request_count = request_count.clone();
            let shutdown = shutdown.clone();
            async move {
                let mut handlers = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted?;
                            let number = connections.fetch_add(1, Ordering::SeqCst) + 1;
                            let request_tx = request_tx.clone();
                            let closed_tx = closed_tx.clone();
                            let request_count = request_count.clone();
                            handlers.spawn(async move {
                                if let Err(error) = serve(stream, number, body_wait, &request_tx, &closed_tx, &request_count).await {
                                    let _ = request_tx.send(Err(error));
                                }
                            });
                        }
                        result = handlers.join_next(), if !handlers.is_empty() => {
                            result.context("handler set closed")? ?;
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
            connections,
            request_count,
            shutdown,
            task,
        })
    }

    pub async fn request(&mut self) -> anyhow::Result<Request> {
        tokio::time::timeout(Duration::from_secs(15), self.requests.recv())
            .await
            .context("peer did not receive HTTP request headers")?
            .context("peer stopped")?
    }

    pub async fn first_closed(&mut self) -> anyhow::Result<()> {
        let number = tokio::time::timeout(Duration::from_secs(10), self.closed.recv())
            .await
            .context("HTTP socket stayed open without response headers")?
            .context("peer stopped before socket closure")??;
        ensure!(number == 1, "wrong socket closed: {number}");
        Ok(())
    }

    pub fn assert_counts(&mut self, expected: usize) -> anyhow::Result<()> {
        ensure!(
            self.connections.load(Ordering::SeqCst) == expected,
            "unexpected HTTP connection count: {}",
            self.connections.load(Ordering::SeqCst)
        );
        ensure!(
            self.request_count.load(Ordering::SeqCst) == expected,
            "unexpected HTTP request count"
        );
        ensure!(
            !self.task.is_finished(),
            "HTTP accept loop stopped unexpectedly"
        );
        ensure!(
            matches!(
                self.requests.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ),
            "unconsumed request, peer error or closed accept loop"
        );
        Ok(())
    }

    pub async fn finish(&mut self) -> anyhow::Result<()> {
        self.shutdown.cancel();
        match tokio::time::timeout(Duration::from_secs(5), &mut self.task).await {
            Ok(result) => result?,
            Err(error) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                Err(error.into())
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    number: usize,
    body_wait: bool,
    requests: &mpsc::UnboundedSender<anyhow::Result<Request>>,
    closed: &mpsc::UnboundedSender<anyhow::Result<usize>>,
    request_count: &AtomicUsize,
) -> anyhow::Result<()> {
    // Read only through the header boundary; the empty request body's framing may follow it.
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        ensure!(headers.len() < 16 * 1024, "oversized request headers");
        headers.push(stream.read_u8().await?);
    }
    let headers = String::from_utf8(headers)?;
    let path = if body_wait {
        "/body-wait"
    } else {
        "/header-wait"
    };
    ensure!(
        headers.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
        "unexpected request: {headers}"
    );
    let keys: Vec<_> = headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("idempotency-key")
                .then(|| value.trim().to_owned())
        })
        .collect();
    ensure!(
        keys.len() == 1 && !keys[0].is_empty(),
        "missing/duplicate idempotency key"
    );
    request_count.fetch_add(1, Ordering::SeqCst);
    let (respond, response) = oneshot::channel();
    requests.send(Ok(Request {
        number,
        idempotency_key: keys[0].clone(),
        respond,
    }))?;
    if body_wait {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nh")
            .await?;
        stream.flush().await?;
    }
    tokio::select! {
        result = response => {
            result.context("response gate dropped")?;
            if body_wait {
                stream.write_all(b"i").await?;
            } else {
                // Body bytes are required for mandatory body and transmission finalization.
                stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi").await?;
            }
            stream.shutdown().await?;
        }
        result = async {
            let mut bytes = [0; 1024];
            loop {
                match stream.read(&mut bytes).await {
                    Ok(0) => break Ok(number),
                    Ok(_) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break Ok(number),
                    Err(error) => break Err(error.into()),
                }
            }
        } => { let _ = closed.send(result); }
    }
    Ok(())
}
