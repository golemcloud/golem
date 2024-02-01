use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::TemplateId;
use poem::web::websocket::{CloseCode, Message, WebSocket, WebSocketStream};
use poem::web::{Data, Path};
use poem::*;
use tap::TapFallible;
use tonic::Status;

use crate::api::auth::AuthData;
use crate::api::connect::keep_alive::{KeepAliveError, WebSocketKeepAlive};
use crate::auth::AccountAuthorisation;
use crate::service::worker::ConnectWorkerStream;
use crate::service::worker::WorkerService;

#[derive(Clone)]
pub struct ConnectService {
    worker_service: Arc<dyn WorkerService + Send + Sync>,
}

impl ConnectService {
    pub fn new(worker_service: Arc<dyn WorkerService + Send + Sync>) -> Self {
        Self { worker_service }
    }
}

#[handler]
pub fn ws(
    Path((template_id, worker_name)): Path<(TemplateId, String)>,
    ws: WebSocket,
    Data(service): Data<&ConnectService>,
    auth: AuthData,
) -> Response {
    let service = service.clone();
    let auth = auth.auth;

    ws.on_upgrade(move |mut socket| async move {
        tokio::spawn(async move {
            let _ =
                try_proxy_worker_connection(&service, template_id, worker_name, auth, socket).await;
        })
    })
    .into_response()
}

async fn try_proxy_worker_connection(
    service: &ConnectService,
    template_id: TemplateId,
    worker_name: String,
    auth: AccountAuthorisation,
    mut websocket: WebSocketStream,
) -> Result<(), ConnectError> {
    let worker_id = make_worker_id(template_id.clone(), worker_name.clone())?;

    let worker_stream = service
        .worker_service
        .connect(&worker_id, &auth)
        .await
        .tap_err(|e| tracing::info!("Error connecting to worker {e}"));

    match worker_stream {
        Ok(worker_stream) => proxy_worker_connection(worker_id, worker_stream, websocket).await,
        Err(err) => {
            let close_message = format!(
                "Error connecting to worker: Template-Id: {}, Worker Name: {},  {}",
                &template_id, &worker_name, err
            );
            let message = Message::Close(Some((CloseCode::Error, close_message)));

            websocket
                .send(message)
                .await
                .map_err(|err| ConnectError(format!("Failed to send closing frame: {}", err)))
        }
    }
}

#[tracing::instrument(skip_all, fields(worker_id = worker_id.to_string()))]
async fn proxy_worker_connection(
    worker_id: golem_service_base::model::WorkerId,
    mut worker_stream: ConnectWorkerStream,
    websocket: WebSocketStream,
) -> Result<(), ConnectError> {
    tracing::info!("Connecting to worker: {worker_id}");

    let mut websocket = WebSocketKeepAlive::from_websocket(
        websocket,
        Duration::from_secs(20),
        Duration::from_secs(10),
    );

    let result = loop {
        tokio::select! {
            // WebSocket is cancellation safe.
            // https://github.com/snapview/tokio-tungstenite/issues/167
            websocket_message = websocket.next() => {
                match websocket_message {
                    Some(Ok(Message::Close(payload))) => {
                        tracing::info!("Client closed WebSocket connection: {payload:?}");
                        break Ok(());
                    }

                    Some(Ok(Message::Ping(_))) => {
                        tracing::info!("Received PING");
                        if let Err(error) = websocket.send(Message::Pong(Vec::new())).await{
                            tracing::info!("Error sending PONG: {error}");
                            break Err(error.into());
                        }
                    }
                    Some(Err(error)) => {
                        let error: ConnectError = error.into();
                        tracing::info!("Received WebSocket Error: {error}");
                        break Err(error);
                    },
                    Some(Ok(_)) => {}
                    None => {
                        tracing::info!("WebSocket connection closed");
                        break Ok(());
                    }
                }
            },

            worker_message = worker_stream.next() => {
                if let Some(message) = worker_message {
                    if let Err(error) = forward_worker_message(message, &mut websocket).await {
                        tracing::info!("Error forwarding message to WebSocket client: {error}");
                        break(Err(error))
                    }
                } else {
                    tracing::info!("Worker Stream ended");
                    break Ok(());
                }
            },
        }
    };

    if let Err(ref error) = result {
        tracing::info!("Error proxying worker: {worker_id} {error}");
    }

    if let Err(error) = websocket.close().await {
        tracing::info!("Error closing WebSocket connection: {error}");
    } else {
        tracing::info!("WebSocket connection successfully closed");
    }

    result
}

async fn forward_worker_message<S, E>(
    message: Result<LogEvent, tonic::Status>,
    socket: &mut S,
) -> Result<(), ConnectError>
where
    S: futures_util::Sink<Message, Error = E> + Unpin,
    ConnectError: From<E>,
{
    let message = message?;
    let msg_json = serde_json::to_string(&message)?;
    socket.send(Message::Text(msg_json)).await?;
    Ok(())
}

#[derive(Debug)]
struct ConnectError(String);

impl Display for ConnectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Status> for ConnectError {
    fn from(value: Status) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<std::io::Error> for ConnectError {
    fn from(value: std::io::Error) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<serde_json::Error> for ConnectError {
    fn from(value: serde_json::Error) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<crate::service::worker::WorkerError> for ConnectError {
    fn from(error: crate::service::worker::WorkerError) -> Self {
        ConnectError(error.to_string())
    }
}

impl<A, B> From<KeepAliveError<A, B>> for ConnectError
where
    A: Display,
    B: Display,
{
    fn from(error: KeepAliveError<A, B>) -> Self {
        match error {
            KeepAliveError::Sink(error) => ConnectError(error.to_string()),
            KeepAliveError::Stream(error) => ConnectError(error.to_string()),
            KeepAliveError::Timeout => ConnectError("Ping confirmation timed out".to_string()),
        }
    }
}

fn make_worker_id(
    template_id: TemplateId,
    worker_name: String,
) -> std::result::Result<golem_service_base::model::WorkerId, ConnectError> {
    golem_service_base::model::WorkerId::new(template_id, worker_name)
        .map_err(|error| ConnectError(format!("Invalid worker name: {error}")))
}

mod keep_alive {
    use std::{
        pin::Pin,
        task::{Context, Poll},
        time::Duration,
    };

    use futures_util::{Future, SinkExt, StreamExt};
    use poem::web::websocket::{Message, WebSocketStream};
    use tokio::time::Instant;

    pub struct WebSocketKeepAlive<A, B> {
        sink: A,
        stream: B,
        max_pong_timeout: Duration,
        // When last ping was sent
        last_ping: Option<Instant>,
        // Timer that triggers sending of Ping messages at regular intervals
        ping_interval: Pin<Box<tokio::time::Interval>>,
        // Is reset when a Ping message is sent
        pong_timeout: Pin<Box<tokio::time::Sleep>>,
    }

    fn far_future() -> Instant {
        // Roughly 30 years from now.
        // API does not provide a way to obtain max `Instant`
        Instant::now() + Duration::from_secs(86400 * 365 * 30)
    }

    impl
        WebSocketKeepAlive<
            futures_util::stream::SplitSink<WebSocketStream, Message>,
            futures_util::stream::SplitStream<WebSocketStream>,
        >
    {
        pub fn from_websocket(
            websocket: WebSocketStream,
            keep_alive_interval: Duration,
            max_confirmation: Duration,
        ) -> Self {
            let (sink, stream) = websocket.split();

            WebSocketKeepAlive::from_sink_and_stream(
                stream,
                sink,
                keep_alive_interval,
                max_confirmation,
            )
        }
    }

    impl<A, B, StreamError> WebSocketKeepAlive<A, B>
    where
        A: futures_util::Sink<Message> + Unpin,
        B: futures_util::Stream<Item = Result<Message, StreamError>> + Unpin,
    {
        pub fn from_sink_and_stream(
            stream: B,
            sink: A,
            keep_alive_interval: Duration,
            max_pong_timeout: Duration,
        ) -> Self {
            let mut ping_interval =
                tokio::time::interval_at(Instant::now() + keep_alive_interval, keep_alive_interval);

            ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            let ping_interval = Box::pin(ping_interval);

            WebSocketKeepAlive {
                sink,
                stream,
                last_ping: None,
                ping_interval,
                max_pong_timeout,
                pong_timeout: Box::pin(tokio::time::sleep_until(far_future())),
            }
        }
    }

    #[derive(PartialEq, Debug)]
    pub enum KeepAliveError<A, B> {
        Sink(A),
        Stream(B),
        Timeout,
    }

    impl<A, B, StreamError> futures_util::Stream for WebSocketKeepAlive<A, B>
    where
        A: futures_util::Sink<Message> + Unpin,
        B: futures_util::Stream<Item = Result<Message, StreamError>> + Unpin,
    {
        type Item = Result<Message, KeepAliveError<A::Error, StreamError>>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            if (self.pong_timeout.as_mut().poll(cx).is_ready() || self.pong_timeout.is_elapsed())
                && self.last_ping.is_some()
            {
                tracing::debug!("Ping confirmation timed out");
                return Poll::Ready(Some(Err(KeepAliveError::Timeout)));
            }

            if self.ping_interval.poll_tick(cx).is_ready() && self.last_ping.is_none() {
                tracing::debug!("Initiating WebSocket Ping");

                let sink_ready = self.sink.poll_ready_unpin(cx).map_err(|e| {
                    tracing::debug!("Error polling sink readiness");
                    KeepAliveError::Sink(e)
                })?;

                if sink_ready.is_pending() {
                    tracing::debug!("Waiting for sink to be ready");
                    return Poll::Pending;
                }

                self.sink
                    .start_send_unpin(Message::Ping(Vec::new()))
                    .map_err(|e| {
                        tracing::debug!("Error sending WebSocket Ping");
                        KeepAliveError::Sink(e)
                    })?;

                let _ = self.sink.poll_flush_unpin(cx).map_err(|e| {
                    tracing::debug!("Error flushing WebSocket Ping");
                    KeepAliveError::Sink(e)
                })?;

                tracing::debug!("WebSocket Ping sent");

                let now = Instant::now();
                let timeout = now + self.max_pong_timeout;

                self.last_ping = Some(now);
                self.pong_timeout.as_mut().reset(timeout);
            }

            // Ensure to return Poll::Pending when waiting for events and register the task's waker
            match self.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(Message::Pong(pong)))) => {
                    tracing::debug!("Received WebSocket confirmation Pong");
                    self.last_ping = None;
                    self.ping_interval.as_mut().reset();

                    // Expiration that should never fire.
                    self.pong_timeout.as_mut().reset(far_future());

                    Poll::Ready(Some(Ok(Message::Pong(pong))))
                }
                Poll::Ready(Some(msg)) => Poll::Ready(Some(msg.map_err(KeepAliveError::Stream))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    impl<A, B, StreamError> futures_util::Sink<Message> for WebSocketKeepAlive<A, B>
    where
        A: futures_util::Sink<Message> + Unpin,
        B: futures_util::Stream<Item = Result<Message, StreamError>> + Unpin,
    {
        type Error = A::Error;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.sink.poll_ready_unpin(cx)
        }

        fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.sink.start_send_unpin(item)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.sink.poll_flush_unpin(cx)
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.sink.poll_close_unpin(cx)
        }
    }

    #[cfg(test)]
    mod test {
        use poem::web::websocket::Message;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Once;
        use tokio::sync::mpsc;
        use tokio::time::{timeout, Duration};
        use tokio_stream::wrappers::ReceiverStream;
        use tokio_util::sync::PollSender;

        use super::*;

        static TRACING_SETUP: Once = Once::new();

        fn setup_tracing() {
            TRACING_SETUP.call_once(|| {
                let subscriber = tracing_subscriber::FmtSubscriber::builder()
                    .with_max_level(tracing::Level::DEBUG)
                    .finish();

                tracing::subscriber::set_global_default(subscriber)
                    .expect("setting default subscriber failed");
            });
        }

        #[tokio::test]
        async fn test_websocket_keep_alive() {
            setup_tracing();

            // Create a channel for simulating incoming WebSocket messages (pong responses)
            let (pong_tx, pong_rx) = mpsc::channel::<Result<Message, ()>>(32);

            // Create a channel for capturing outgoing WebSocket messages (ping requests)
            let (ping_tx, mut ping_rx) = mpsc::channel(32);

            // convert to stream and sink.
            let pong_stream = ReceiverStream::new(pong_rx);
            let ping_sink = PollSender::new(ping_tx);

            // Initialize WebSocketKeepAlive with the ping sender and pong receiver
            let mut websocket_keep_alive = WebSocketKeepAlive::from_sink_and_stream(
                pong_stream,
                ping_sink,
                Duration::from_millis(10),
                Duration::from_millis(50),
            );

            // Task and flag to simulate a response message.
            let send_pong = std::sync::Arc::new(AtomicBool::new(true));
            tokio::spawn({
                let pong_tx = pong_tx.clone();
                let send_pong = send_pong.clone();
                async move {
                    while let Some(_) = ping_rx.recv().await {
                        if send_pong.load(Ordering::Relaxed) {
                            pong_tx
                                .send(Ok(Message::Pong(Vec::new())))
                                .await
                                .expect("Failed to send pong");
                        }
                    }
                }
            });

            // Poll the WebSocketKeepAlive stream to trigger the keep-alive mechanism
            let next = websocket_keep_alive.next().await;

            // Verify the pong message was received and processed by the WebSocketKeepAlive stream
            assert!(
                matches!(next, Some(Ok(Message::Pong(_)))),
                "Expected to receive a pong message"
            );

            // send an arbitrary message.
            pong_tx
                .send(Ok(Message::Text("test".to_string())))
                .await
                .expect("Failed to send msg");

            let next = websocket_keep_alive.next().await;

            assert!(
                matches!(next, Some(Ok(Message::Text(_)))),
                "Expected to receive a text message"
            );

            // Make sure that ping/pong isn't executed
            let next = timeout(Duration::ZERO, websocket_keep_alive.next()).await;
            assert!(
                matches!(next, Err(_)),
                "Expected to receive a timeout error"
            );

            let next = websocket_keep_alive.next().await;

            assert!(
                matches!(next, Some(Ok(Message::Pong(_)))),
                "Expected to receive a pong message"
            );

            send_pong.store(false, Ordering::Relaxed);
            let next = websocket_keep_alive.next().await;

            assert!(
                matches!(next, Some(Err(KeepAliveError::Timeout))),
                "Expected to receive a timeout error"
            );
        }
    }
}
