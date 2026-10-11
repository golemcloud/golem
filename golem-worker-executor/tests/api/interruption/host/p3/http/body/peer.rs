use anyhow::{Context, ensure};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) const FIRST: &[u8] = b"h";
pub(super) const BODY: &[u8] = b"hello-body";

pub(super) struct Request {
    pub number: usize,
    pub idempotency_key: String,
    pub finish_body: oneshot::Sender<()>,
}

#[derive(Default)]
struct Counts {
    connections: AtomicUsize,
    first_chunks: AtomicUsize,
    finished_bodies: AtomicUsize,
    closed: AtomicUsize,
}

pub(super) struct Peer {
    pub url: String,
    requests: mpsc::UnboundedReceiver<anyhow::Result<Request>>,
    closed: mpsc::UnboundedReceiver<anyhow::Result<usize>>,
    counts: Arc<Counts>,
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Peer {
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/body-wait", listener.local_addr()?);
        let (request_tx, requests) = mpsc::unbounded_channel();
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let counts = Arc::new(Counts::default());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let counts = counts.clone();
            let shutdown = shutdown.clone();
            async move {
                let mut handlers = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted?;
                            let number = counts.connections.fetch_add(1, Ordering::SeqCst) + 1;
                            let request_tx = request_tx.clone();
                            let closed_tx = closed_tx.clone();
                            let counts = counts.clone();
                            handlers.spawn(async move {
                                if let Err(error) = serve(stream, number, &request_tx, &closed_tx, &counts).await {
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
            counts,
            shutdown,
            task,
        })
    }

    pub async fn request(&mut self) -> anyhow::Result<Request> {
        tokio::time::timeout(Duration::from_secs(15), self.requests.recv())
            .await
            .context("peer did not send HTTP headers and first bounded chunk")?
            .context("peer stopped")?
    }

    pub async fn first_closed(&mut self) -> anyhow::Result<()> {
        let number = tokio::time::timeout(Duration::from_secs(10), self.closed.recv())
            .await
            .context("HTTP socket stayed open with the next body bytes withheld")?
            .context("peer stopped before socket closure")??;
        ensure!(number == 1, "wrong socket closed: {number}");
        Ok(())
    }

    pub fn assert_counts(
        &mut self,
        requests: usize,
        finished: usize,
        closed: usize,
    ) -> anyhow::Result<()> {
        ensure!(
            self.counts.connections.load(Ordering::SeqCst) == requests,
            "unexpected HTTP connection count"
        );
        ensure!(
            self.counts.first_chunks.load(Ordering::SeqCst) == requests,
            "unexpected HTTP first-chunk count"
        );
        ensure!(
            self.counts.finished_bodies.load(Ordering::SeqCst) == finished,
            "unexpected completed response count"
        );
        ensure!(
            self.counts.closed.load(Ordering::SeqCst) == closed,
            "unexpected peer close count"
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
    requests: &mpsc::UnboundedSender<anyhow::Result<Request>>,
    closed: &mpsc::UnboundedSender<anyhow::Result<usize>>,
    counts: &Counts,
) -> anyhow::Result<()> {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        ensure!(headers.len() < 16 * 1024, "oversized request headers");
        headers.push(stream.read_u8().await?);
    }
    let headers = String::from_utf8(headers)?;
    ensure!(
        headers.starts_with("GET /body-wait HTTP/1.1\r\n"),
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
    // One byte cannot be split into multiple nonempty native frames. Content-length
    // promises nine more bytes, but no peer timeout or EOF can supply them.
    stream
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\nconnection: close\r\n\r\nh")
        .await?;
    stream.flush().await?;
    counts.first_chunks.fetch_add(1, Ordering::SeqCst);
    let (finish_body, response) = oneshot::channel();
    requests.send(Ok(Request {
        number,
        idempotency_key: keys[0].clone(),
        finish_body,
    }))?;
    tokio::select! {
        result = response => {
            result.context("body gate dropped before peer closure")?;
            stream.write_all(&BODY[FIRST.len()..]).await?;
            stream.flush().await?;
            counts.finished_bodies.fetch_add(1, Ordering::SeqCst);
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
        } => {
            if result.is_ok() { counts.closed.fetch_add(1, Ordering::SeqCst); }
            let _ = closed.send(result);
        }
    }
    Ok(())
}
