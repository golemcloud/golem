use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::TemplateId;
use poem::web::websocket::{Message, WebSocket, WebSocketStream};
use poem::web::{Data, Path};
use poem::*;
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
    let auth = auth.auth.clone();

    ws.on_upgrade(move |mut socket| async move {
        tokio::spawn(async move {
            match try_proxy_worker_connection(&service, template_id, worker_name, auth, socket)
                .await
            {
                Ok(()) => {
                    tracing::info!("Worker connection closed");
                }
                Err(err) => {
                    tracing::error!("Error connecting to worker: {err}");
                }
            }
        })
    })
    .into_response()
}

async fn try_proxy_worker_connection(
    service: &ConnectService,
    template_id: TemplateId,
    worker_name: String,
    auth: AccountAuthorisation,
    websocket: WebSocketStream,
) -> Result<(), ConnectError> {
    let worker_id = make_worker_id(template_id, worker_name)?;

    tracing::info!("Connecting to worker");

    let worker_stream = service.worker_service.connect(&worker_id, &auth).await?;

    proxy_worker_connection(worker_id, worker_stream, websocket).await
}

#[tracing::instrument(skip_all, fields(worker_id = worker_id.to_string()))]
async fn proxy_worker_connection(
    worker_id: golem_service_base::model::WorkerId,
    mut worker_stream: ConnectWorkerStream,
    websocket: WebSocketStream,
) -> Result<(), ConnectError> {
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
                        if let Err(error) = websocket.close().await {
                            break Err(error.into());
                        }
                    }
                } else {
                    break Ok(());
                }
            },
        }
    };

    if result.is_err() {
        if let Err(error) = websocket.close().await {
            tracing::info!("Error closing WebSocket connection: {error}");
        } else {
            tracing::info!("WebSocket connection successfully closed");
        }
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

    use futures_util::{SinkExt, StreamExt};
    use poem::web::websocket::{Message, WebSocketStream};
    use tokio::time::Instant;

    pub struct WebSocketKeepAlive<A, B> {
        sink: A,
        stream: B,
        // Interval between sending Ping messages
        keep_alive_interval: Duration,
        // Maximum time to wait for a Pong response before closing the connection
        max_confirmation: Duration,
        // The start time for keepalive. Is reset when a pong is received.
        start_time: Option<Instant>,
        last_ping: Option<Instant>,
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
        A::Error: std::fmt::Debug,
        B: futures_util::Stream<Item = Result<Message, StreamError>> + Unpin,
    {
        pub fn from_sink_and_stream(
            stream: B,
            sink: A,
            keep_alive_interval: Duration,
            max_confirmation: Duration,
        ) -> Self {
            WebSocketKeepAlive {
                last_ping: None,
                start_time: None,
                keep_alive_interval,
                max_confirmation,
                sink,
                stream,
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
            // Set start_time on first poll.
            let start_time = {
                if let Some(start_time) = self.start_time {
                    start_time
                } else {
                    tracing::debug!("Starting WebSocket KeepAlive");
                    let start_time = Instant::now();
                    self.start_time = Some(start_time);
                    start_time
                }
            };

            // Close the sink if the last ping was not confirmed
            if let Some(last_ping) = self.last_ping {
                let elapsed = last_ping.elapsed();
                if elapsed > self.max_confirmation {
                    tracing::debug!("Ping confirmation timed out: {}", elapsed.as_millis());
                    return Poll::Ready(Some(Err(KeepAliveError::Timeout)));
                }
            // Send a ping if needed
            } else if start_time.elapsed() > self.keep_alive_interval {
                tracing::debug!("Initiating WebSocket Ping");

                let sink_ready = self.sink.poll_ready_unpin(cx).map_err(|e| {
                    tracing::debug!("Error polling sink readiness");
                    KeepAliveError::Sink(e)
                })?;

                if sink_ready.is_pending() {
                    tracing::debug!("Waiting for sink to be ready");
                    return Poll::Pending;
                }

                let send_result = self.sink.start_send_unpin(Message::Ping(Vec::new()));

                if let Err(error) = send_result {
                    tracing::debug!("Error sending WebSocket Ping");
                    return Poll::Ready(Some(Err(KeepAliveError::Sink(error))));
                } else {
                    tracing::debug!("WebSocket Ping sent");
                    self.last_ping = Some(Instant::now());
                }
            }

            match self.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(Message::Pong(pong)))) => {
                    tracing::debug!("Received WebSocket confirmation Pong");
                    self.last_ping = None;
                    self.start_time = Some(Instant::now());
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
        use futures_util::stream::Stream;
        use futures_util::Sink;
        use poem::web::websocket::Message;
        use std::pin::Pin;
        use std::sync::Once;
        use std::task::{Context, Poll};
        use tokio::sync::mpsc;
        use tokio::time::{timeout, Duration};
        use tokio_stream::wrappers::ReceiverStream;
        use tokio_util::sync::PollSender;

        use super::*;

        // Mock sink that accepts messages and records them
        struct MockSink {
            closed: bool,
            sent_messages: Vec<Message>,
        }

        impl Sink<Message> for MockSink {
            type Error = ();

            fn poll_ready(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }

            fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
                let self_mut = self.get_mut();
                self_mut.sent_messages.push(item);
                Ok(())
            }

            fn poll_flush(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }

            fn poll_close(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                if self.closed {
                    Poll::Ready(Err(()))
                } else {
                    let self_mut = self.get_mut();
                    self_mut.closed = true;
                    Poll::Ready(Ok(()))
                }
            }
        }

        // Mock stream that can yield predefined messages
        struct MockStream {
            messages_to_send: Vec<Result<Message, ()>>,
        }

        impl Stream for MockStream {
            type Item = Result<Message, ()>;

            fn poll_next(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if let Some(msg) = self.messages_to_send.pop() {
                    Poll::Ready(Some(msg))
                } else {
                    Poll::Pending
                }
            }
        }

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
        async fn test_ping_pong_cycle() {
            setup_tracing();

            let mock_sink = MockSink {
                closed: false,
                sent_messages: Vec::new(),
            };
            // Simulate a Pong response
            let mock_stream = MockStream {
                messages_to_send: vec![],
            };

            let ping_interval = Duration::from_millis(50);

            let mut keep_alive = WebSocketKeepAlive::from_sink_and_stream(
                mock_stream,
                mock_sink,
                ping_interval,
                Duration::from_millis(100),
            );

            let waker = futures::task::noop_waker_ref();
            let mut cx = Context::from_waker(waker);

            let poll_result = keep_alive.poll_next_unpin(&mut cx);
            assert!(poll_result.is_pending());
            assert!(keep_alive.start_time.is_some());
            assert!(keep_alive.last_ping.is_none());
            assert!(keep_alive.sink.sent_messages.first().is_none());

            // This sleep is necessary for the interval to fire
            tokio::time::sleep(ping_interval).await;

            // Check Ping was sent
            let poll_result = keep_alive.poll_next_unpin(&mut cx);
            assert!(poll_result.is_pending());
            assert!(matches!(
                keep_alive.sink.sent_messages.first(),
                Some(Message::Ping(_))
            ));
            assert!(keep_alive.last_ping.is_some());

            // Set sink message to respond with a pong on next poll.
            keep_alive
                .stream
                .messages_to_send
                .push(Ok(Message::Pong(Vec::new())));

            // Poll again to process the Pong
            let poll_result = Pin::new(&mut keep_alive).poll_next(&mut cx);
            assert!(matches!(
                poll_result,
                Poll::Ready(Some(Ok(Message::Pong(_))))
            ));
            // Ensure the Pong was processed and last_ping is reset
            assert!(keep_alive.last_ping.is_none());
        }

        #[tokio::test]
        async fn test_pong_timeout() {
            setup_tracing();

            let mock_sink = MockSink {
                sent_messages: Vec::new(),
                closed: false,
            };

            // No Pong response is simulated by leaving messages_to_send empty
            let mock_stream = MockStream {
                messages_to_send: vec![],
            };

            let ping_interval = Duration::from_millis(50);

            let mut keep_alive = WebSocketKeepAlive::from_sink_and_stream(
                mock_stream,
                mock_sink,
                ping_interval,
                ping_interval + ping_interval,
            );

            // Simulate polling the Stream to trigger a Ping send
            let waker = futures::task::noop_waker_ref();
            let mut cx = Context::from_waker(waker);

            let poll_result = keep_alive.poll_next_unpin(&mut cx);
            assert!(poll_result.is_pending());
            assert!(keep_alive.start_time.is_some());
            assert!(keep_alive.last_ping.is_none());
            assert!(keep_alive.sink.sent_messages.first().is_none());

            // This sleep is necessary for the interval to fire
            tokio::time::sleep(ping_interval).await;

            let poll_result = keep_alive.poll_next_unpin(&mut cx);

            // Assert that a Ping was sent
            assert!(poll_result.is_pending());
            assert_eq!(keep_alive.sink.sent_messages.len(), 1);
            assert!(matches!(
                keep_alive.sink.sent_messages.first(),
                Some(Message::Ping(_))
            ));
            assert!(keep_alive.last_ping.is_some());

            // Sleep but not enough for timeout
            tokio::time::sleep(ping_interval).await;

            let poll_result = keep_alive.poll_next_unpin(&mut cx);
            assert_eq!(
                poll_result,
                Poll::Pending,
                "Timeout hasn't expired, so no error should be returned"
            );

            // Simulate enough time passing without receiving a Pong response
            tokio::time::sleep(ping_interval).await;

            // Poll again to trigger the timeout behavior
            let poll_result = keep_alive.poll_next_unpin(&mut cx);
            assert_eq!(
                poll_result,
                Poll::Ready(Some(Err(KeepAliveError::Timeout))),
                "Timeout error should be returned"
            );
        }

        #[tokio::test]
        async fn test_websocket_keep_alive_with_mpsc() {
            // Setup tracing if necessary
            setup_tracing();

            // Create a channel for simulating incoming WebSocket messages (pong responses)
            let (pong_tx, pong_rx) = mpsc::channel::<Result<Message, ()>>(32);

            // Create a channel for capturing outgoing WebSocket messages (ping requests)
            let (ping_tx, mut ping_rx) = mpsc::channel(32);

            // convert to stream and sink.
            let pong_stream = ReceiverStream::new(pong_rx);
            let ping_sink = PollSender::new(ping_tx);

            let ping_interval = Duration::from_millis(50);

            // Initialize WebSocketKeepAlive with the ping sender and pong receiver
            let mut websocket_keep_alive = WebSocketKeepAlive::from_sink_and_stream(
                pong_stream,
                ping_sink,
                ping_interval,
                Duration::from_millis(1000),
            );

            // Test that ping/pong response works
            // Simulate receiving a pong response after a delay
            tokio::spawn({
                let pong_tx = pong_tx.clone();
                async move {
                    while let Some(_) = ping_rx.recv().await {
                        pong_tx
                            .send(Ok(Message::Pong(Vec::new())))
                            .await
                            .expect("Failed to send pong");
                    }
                }
            });

            // Initiate polling.
            let _ = timeout(Duration::ZERO, websocket_keep_alive.next()).await;

            tokio::time::sleep(ping_interval).await;

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

            // Sleep the rest of the interval, so that the next ping/pong executes.
            tokio::time::sleep(ping_interval).await;

            let next = websocket_keep_alive.next().await;

            assert!(
                matches!(next, Some(Ok(Message::Pong(_)))),
                "Expected to receive a pong message"
            );
        }
    }
}
