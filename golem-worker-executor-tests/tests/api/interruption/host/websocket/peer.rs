use anyhow::{Context, ensure};
use futures::SinkExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::{handshake::derive_accept_key, protocol::Role};
use tokio_util::sync::CancellationToken;

pub(super) struct Handshake {
    pub number: usize,
    pub send: oneshot::Sender<String>,
    pub send_second: oneshot::Sender<String>,
}

pub(super) struct HeldHandshake {
    pub number: usize,
    pub release: oneshot::Sender<()>,
}

pub(super) struct Peer {
    pub url: String,
    handshakes: mpsc::UnboundedReceiver<anyhow::Result<Handshake>>,
    held: mpsc::UnboundedReceiver<anyhow::Result<HeldHandshake>>,
    closed: mpsc::UnboundedReceiver<anyhow::Result<usize>>,
    connections: Arc<AtomicUsize>,
    handshake_count: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Peer {
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_options(None, false).await
    }

    pub async fn start_with_held_handshake(hold_first: bool) -> anyhow::Result<Self> {
        Self::start_with_options(hold_first.then_some(1), false).await
    }

    pub async fn start_allowing_close() -> anyhow::Result<Self> {
        Self::start_with_options(None, true).await
    }

    pub async fn start_holding_third(allow_close: bool) -> anyhow::Result<Self> {
        Self::start_with_options(Some(3), allow_close).await
    }

    async fn start_with_options(
        held_number: Option<usize>,
        allow_close: bool,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("ws://{}/receive-wait", listener.local_addr()?);
        let (handshake_tx, handshakes) = mpsc::unbounded_channel();
        let (held_tx, held) = mpsc::unbounded_channel();
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let handshake_count = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let connections = connections.clone();
            let handshake_count = handshake_count.clone();
            let frames = frames.clone();
            let shutdown = shutdown.clone();
            async move {
                let mut handlers = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted?;
                            let number = connections.fetch_add(1, Ordering::SeqCst) + 1;
                            let handshake_tx = handshake_tx.clone();
                            let held_tx = held_tx.clone();
                            let closed_tx = closed_tx.clone();
                            let handshake_count = handshake_count.clone();
                            let frames = frames.clone();
                            handlers.spawn(async move {
                                if let Err(error) = serve(stream, number, held_number == Some(number), allow_close, &held_tx, &handshake_tx, &closed_tx, &handshake_count, &frames).await {
                                    let _ = handshake_tx.send(Err(error));
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
            handshakes,
            held,
            closed,
            connections,
            handshake_count,
            frames,
            shutdown,
            task,
        })
    }

    pub async fn handshake(&mut self) -> anyhow::Result<Handshake> {
        tokio::time::timeout(Duration::from_secs(15), self.handshakes.recv())
            .await
            .context("peer did not complete WebSocket handshake")?
            .context("peer stopped")?
    }

    pub async fn held(&mut self) -> anyhow::Result<HeldHandshake> {
        tokio::time::timeout(Duration::from_secs(15), self.held.recv())
            .await
            .context("peer did not read the TCP Upgrade request")?
            .context("peer stopped before Upgrade request")?
    }

    pub async fn closed(&mut self, expected: usize) -> anyhow::Result<()> {
        let number = tokio::time::timeout(Duration::from_secs(10), self.closed.recv())
            .await
            .context("WebSocket stayed open without a peer frame or close")?
            .context("peer stopped before socket closure")??;
        ensure!(number == expected, "wrong socket closed: {number}");
        Ok(())
    }

    pub fn assert_counts(&mut self, handshakes: usize, frames: usize) -> anyhow::Result<()> {
        self.assert_effects(handshakes, handshakes, frames)
    }

    pub fn assert_effects(
        &mut self,
        accepts: usize,
        handshakes: usize,
        frames: usize,
    ) -> anyhow::Result<()> {
        ensure!(
            self.connections.load(Ordering::SeqCst) == accepts,
            "unexpected WebSocket connection count: {}",
            self.connections.load(Ordering::SeqCst)
        );
        ensure!(
            self.handshake_count.load(Ordering::SeqCst) == handshakes,
            "unexpected WebSocket handshake count"
        );
        ensure!(
            self.frames.load(Ordering::SeqCst) == frames,
            "unexpected peer frame count"
        );
        ensure!(
            !self.task.is_finished(),
            "WebSocket accept loop stopped unexpectedly"
        );
        ensure!(
            matches!(self.held.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "unconsumed held handshake or closed accept loop"
        );
        ensure!(
            matches!(
                self.handshakes.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ),
            "unconsumed handshake, peer error or closed accept loop"
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
    stream: TcpStream,
    number: usize,
    hold: bool,
    allow_close: bool,
    held: &mpsc::UnboundedSender<anyhow::Result<HeldHandshake>>,
    handshakes: &mpsc::UnboundedSender<anyhow::Result<Handshake>>,
    closed: &mpsc::UnboundedSender<anyhow::Result<usize>>,
    handshake_count: &AtomicUsize,
    frames: &AtomicUsize,
) -> anyhow::Result<()> {
    let mut ws = if hold {
        let mut stream = stream;
        let mut request = Vec::new();
        loop {
            ensure!(request.len() < 8192, "Upgrade request exceeds peer limit");
            let mut buf = [0; 1024];
            let count = stream.read(&mut buf).await?;
            ensure!(count > 0, "peer closed before sending Upgrade request");
            request.extend_from_slice(&buf[..count]);
            if request.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
        }
        let text = std::str::from_utf8(&request).context("non-UTF-8 Upgrade request")?;
        ensure!(
            text.starts_with("GET /receive-wait HTTP/1.1\r\n"),
            "wrong Upgrade request: {text}"
        );
        let key = text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("sec-websocket-key")
                    .then(|| value.trim())
            })
            .context("Upgrade request lacks Sec-WebSocket-Key")?;
        ensure!(
            text.lines()
                .any(|line| line.eq_ignore_ascii_case("upgrade: websocket")),
            "no WebSocket Upgrade header"
        );
        let accept = derive_accept_key(key.as_bytes());
        let (release, gate) = oneshot::channel();
        held.send(Ok(HeldHandshake { number, release }))?;
        tokio::select! {
            result = gate => result.context("handshake release gate dropped")?,
            result = wait_for_eof(&mut stream) => {
                let _ = closed.send(result.map(|()| number));
                return Ok(());
            }
        }
        stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await?;
        tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Server, None).await
    } else {
        tokio_tungstenite::accept_async(stream).await?
    };
    handshake_count.fetch_add(1, Ordering::SeqCst);
    let (send, frame) = oneshot::channel();
    let (send_second, second_frame) = oneshot::channel();
    handshakes.send(Ok(Handshake {
        number,
        send,
        send_second,
    }))?;
    // Observe transport EOF directly so the peer cannot auto-reply with pong or close.
    tokio::select! {
        payload = frame => {
            ws.send(Message::Text(payload.context("frame gate dropped")?.into())).await?;
            frames.fetch_add(1, Ordering::SeqCst);
            tokio::select! {
                payload = second_frame => {
                    if let Ok(payload) = payload {
                        ws.send(Message::Text(payload.into())).await?;
                        frames.fetch_add(1, Ordering::SeqCst);
                    }
                    let result = wait_for_eof_or_close(ws.get_mut(), allow_close).await.map(|()| number);
                    let _ = closed.send(result);
                }
                result = wait_for_eof_or_close(ws.get_mut(), allow_close) => {
                    let _ = closed.send(result.map(|()| number));
                }
            }
        }
        result = wait_for_eof_or_close(ws.get_mut(), allow_close) => {
            let _ = closed.send(result.map(|()| number));
        }
    }
    Ok(())
}

async fn wait_for_eof(stream: &mut TcpStream) -> anyhow::Result<()> {
    wait_for_eof_or_close(stream, false).await
}

async fn wait_for_eof_or_close(stream: &mut TcpStream, allow_close: bool) -> anyhow::Result<()> {
    let mut first = [0];
    match stream.read(&mut first).await {
        Ok(0) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) if allow_close && first[0] == 0x88 => {
            let mut length = [0];
            stream.read_exact(&mut length).await?;
            ensure!(
                length[0] & 0x80 != 0 && length[0] & 0x7f <= 125,
                "invalid client close frame"
            );
            let mut payload = vec![0; 4 + usize::from(length[0] & 0x7f)];
            stream.read_exact(&mut payload).await?;
            let mut end = [0];
            ensure!(
                stream.read(&mut end).await? == 0,
                "unexpected data after client close frame"
            );
            Ok(())
        }
        Ok(_) => anyhow::bail!("receive-only guest unexpectedly sent a WebSocket frame"),
    }
}
