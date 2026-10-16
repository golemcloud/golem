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

use std::{
    io::{Error as IoError, Result as IoResult},
    time::Duration,
};

use futures::{Sink, SinkExt, Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::{AgentEvent, AgentId};
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use poem::web::websocket::Message;
use tonic::Status;
use tracing::{Instrument, Level, error, info};

/// Proxies a worker connection, listening for either connection to close. Websocket sink will be closed at the end.
///
/// origin: the request that opened this connection, to link the proxy's span back to
/// keep_alive_interval: Interval at which Ping messages are sent
/// max_pong_timeout: Maximum time to wait for a Pong message before considering the connection dead;
/// zero keeps Pings enabled without requiring Pongs within a deadline
// The proxy runs for the connection's whole life - hours, for a `connect` websocket -
// so it is the root of its own trace, linked to the request that opened it rather than
// nested under it. The origin has to be captured by the caller: poem drives the upgrade
// callback from a bare `tokio::spawn`, so by the time this runs there is no current
// span left to capture.
pub async fn proxy_worker_connection(
    agent_id: AgentId,
    origin: TraceOrigin,
    mut worker_stream: impl Stream<Item = Result<LogEvent, Status>> + Unpin,
    websocket_sender: impl Sink<Message, Error = IoError> + Unpin,
    websocket_receiver: impl Stream<Item = IoResult<Message>> + Unpin,
    keep_alive_interval: Duration,
    max_pong_timeout: Duration,
) -> Result<(), ConnectProxyError> {
    let span = related_span!(
        origin,
        Level::INFO,
        "proxy_worker_connection",
        agent_id = %agent_id
    );

    async move {
        info!("Proxying worker connection");

        let mut websocket = keep_alive::WebSocketKeepAlive::from_sink_and_stream(
            websocket_receiver,
            websocket_sender,
            keep_alive_interval,
            max_pong_timeout,
        );

        let result = loop {
            tokio::select! {
                // WebSocket is cancellation safe.
                // https://github.com/snapview/tokio-tungstenite/issues/167
                websocket_message = websocket.next() => {
                    match websocket_message {
                        Some(Ok(Message::Close(payload))) => {
                            info!(
                                close_code=payload.as_ref().map(|p| u16::from(p.0)),
                                close_message=payload.as_ref().map(|p| &p.1),
                                "Client closed WebSocket connection",
                            );
                            break Ok(());
                        }
                        Some(Err(error)) => {
                            let error: ConnectProxyError = error.into();
                            info!(error=error.to_string(), "Received WebSocket Error");
                            break Err(error);
                        },
                        Some(Ok(_)) => {
                        }
                        None => {
                            info!("WebSocket connection closed");
                            break Ok(());
                        }
                    }
                },

                worker_message = worker_stream.next() => {
                    if let Some(message) = worker_message {
                        if let Err(error) = forward_worker_message(message, &mut websocket).await {
                            info!(error=error.to_string(), "Error forwarding message to WebSocket client");
                            break Err(error)

                        }
                    } else {
                        info!("Worker stream ended");
                        break Ok(());
                    }
                },
            }
        };

        info!("Closing websocket connection");
        if let Err(error) = websocket.close().await {
            error!(
                error = error.to_string(),
                "Error closing WebSocket connection"
            );
        } else {
            info!("WebSocket connection successfully closed");
        }

        result
    }
    .instrument(span)
    .await
}

async fn forward_worker_message<E>(
    message: Result<LogEvent, tonic::Status>,
    socket: &mut (impl Sink<Message, Error = E> + Unpin),
) -> Result<(), ConnectProxyError>
where
    ConnectProxyError: From<E>,
{
    let message: AgentEvent = message?.try_into().map_err(ConnectProxyError::Proto)?;
    let msg_json = serde_json::to_string(&message)?;
    socket.send(Message::Text(msg_json)).await?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectProxyError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("Internal protocol error: {0}")]
    Proto(String),

    #[error(transparent)]
    Tonic(#[from] tonic::Status),

    #[error("Ping confirmation timed out")]
    Timeout,
}

impl<A, B> From<keep_alive::KeepAliveError<A, B>> for ConnectProxyError
where
    A: Into<ConnectProxyError>,
    B: Into<ConnectProxyError>,
{
    fn from(error: keep_alive::KeepAliveError<A, B>) -> Self {
        match error {
            keep_alive::KeepAliveError::Sink(error) => error.into(),
            keep_alive::KeepAliveError::Stream(error) => error.into(),
            keep_alive::KeepAliveError::Timeout => ConnectProxyError::Timeout,
        }
    }
}

pub(crate) mod keep_alive {
    use std::{
        pin::Pin,
        sync::Arc,
        task::{Context, Poll},
        time::Duration,
    };

    use futures::task::{ArcWake, AtomicWaker, waker_ref};
    use futures::{Future, Sink, SinkExt, Stream, StreamExt};
    use poem::web::websocket::Message;
    use tokio::time::Instant;
    use tracing::debug;

    #[derive(Default)]
    struct WebSocketWakers {
        reader: AtomicWaker,
        writer: AtomicWaker,
    }

    impl ArcWake for WebSocketWakers {
        fn wake_by_ref(wakers: &Arc<Self>) {
            wakers.reader.wake();
            wakers.writer.wake();
        }
    }

    #[derive(PartialEq, Eq)]
    enum PingState {
        Idle,
        WaitingForReady,
        Flushing,
        WaitingForPong,
    }

    pub struct WebSocketKeepAlive<A, B> {
        sink: A,
        stream: B,
        max_pong_timeout: Duration,
        ping_state: PingState,
        // Timer that triggers sending of Ping messages at regular intervals
        ping_interval: Pin<Box<tokio::time::Interval>>,
        // Armed only after the Ping has been flushed.
        pong_timeout: Pin<Box<tokio::time::Sleep>>,
        // Reading sends Pings through the application sink. Either poller can register
        // the sink's readiness waker, so readiness must wake both pollers.
        wakers: Arc<WebSocketWakers>,
    }

    fn far_future() -> Instant {
        // Roughly 30 years from now.
        // API does not provide a way to obtain max `Instant`
        Instant::now() + Duration::from_secs(86400 * 365 * 30)
    }

    impl<A, B, StreamError> WebSocketKeepAlive<A, B>
    where
        A: Sink<Message> + Unpin,
        B: Stream<Item = Result<Message, StreamError>> + Unpin,
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
                ping_state: PingState::Idle,
                ping_interval,
                max_pong_timeout,
                pong_timeout: Box::pin(tokio::time::sleep_until(far_future())),
                wakers: Arc::default(),
            }
        }
    }

    #[derive(PartialEq, Debug)]
    pub enum KeepAliveError<A, B> {
        Sink(A),
        Stream(B),
        Timeout,
    }

    impl<A, B, StreamError> Stream for WebSocketKeepAlive<A, B>
    where
        A: Sink<Message> + Unpin,
        B: Stream<Item = Result<Message, StreamError>> + Unpin,
    {
        type Item = Result<Message, KeepAliveError<A::Error, StreamError>>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            let wakers = self.wakers.clone();
            wakers.reader.register(cx.waker());
            let waker = waker_ref(&wakers);
            let mut context = Context::from_waker(&waker);
            let cx = &mut context;
            if (self.pong_timeout.as_mut().poll(cx).is_ready() || self.pong_timeout.is_elapsed())
                && self.ping_state == PingState::WaitingForPong
            {
                debug!("Ping confirmation timed out");
                return Poll::Ready(Some(Err(KeepAliveError::Timeout)));
            }

            if self.ping_state == PingState::Idle && self.ping_interval.poll_tick(cx).is_ready() {
                self.ping_state = PingState::WaitingForReady;
            }

            if self.ping_state == PingState::WaitingForReady {
                debug!("Initiating WebSocket Ping");

                let sink_ready = self.sink.poll_ready_unpin(cx).map_err(|e| {
                    debug!("Error polling sink readiness");
                    KeepAliveError::Sink(e)
                })?;

                if sink_ready.is_pending() {
                    debug!("Waiting for sink to be ready");
                    return Poll::Pending;
                }

                self.sink
                    .start_send_unpin(Message::Ping(Vec::new()))
                    .map_err(|e| {
                        debug!("Error sending WebSocket Ping");
                        KeepAliveError::Sink(e)
                    })?;

                self.ping_state = PingState::Flushing;
            }

            if self.ping_state == PingState::Flushing {
                let flushed = self.sink.poll_flush_unpin(cx).map_err(|e| {
                    debug!("Error flushing WebSocket Ping");
                    KeepAliveError::Sink(e)
                })?;
                if flushed.is_pending() {
                    return Poll::Pending;
                }

                debug!("WebSocket Ping sent");

                if !self.max_pong_timeout.is_zero() {
                    let now = Instant::now();
                    let timeout = now + self.max_pong_timeout;

                    self.ping_state = PingState::WaitingForPong;
                    self.pong_timeout.as_mut().reset(timeout);
                } else {
                    self.ping_state = PingState::Idle;
                }
                // Poll again to register the interval's next tick even without incoming traffic.
                cx.waker().wake_by_ref();
            }

            match self.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(Message::Pong(pong)))) => {
                    debug!("Received WebSocket confirmation Pong");
                    self.ping_state = PingState::Idle;
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

    impl<A, B, StreamError> Sink<Message> for WebSocketKeepAlive<A, B>
    where
        A: Sink<Message> + Unpin,
        B: Stream<Item = Result<Message, StreamError>> + Unpin,
    {
        type Error = A::Error;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let wakers = self.wakers.clone();
            wakers.writer.register(cx.waker());
            let waker = waker_ref(&wakers);
            self.sink.poll_ready_unpin(&mut Context::from_waker(&waker))
        }

        fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.sink.start_send_unpin(item)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let wakers = self.wakers.clone();
            wakers.writer.register(cx.waker());
            let waker = waker_ref(&wakers);
            self.sink.poll_flush_unpin(&mut Context::from_waker(&waker))
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let wakers = self.wakers.clone();
            wakers.writer.register(cx.waker());
            let waker = waker_ref(&wakers);
            self.sink.poll_close_unpin(&mut Context::from_waker(&waker))
        }
    }

    #[cfg(test)]
    mod test {
        use test_r::test;

        use std::sync::Once;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        use super::*;
        use poem::web::websocket::Message;
        use tokio::sync::mpsc;
        use tokio::time::{Duration, timeout};
        use tokio_stream::wrappers::ReceiverStream;
        use tokio_util::sync::PollSender;
        use tracing::{Instrument, Level};

        #[derive(Default)]
        struct GatedSinkState {
            ready: AtomicBool,
            flushed: AtomicBool,
            sent: AtomicUsize,
            waker: AtomicWaker,
        }

        struct GatedSink(Arc<GatedSinkState>);

        impl Sink<Message> for GatedSink {
            type Error = ();

            fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
                self.0.waker.register(cx.waker());
                if self.0.ready.load(Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }

            fn start_send(self: Pin<&mut Self>, message: Message) -> Result<(), ()> {
                assert!(matches!(message, Message::Ping(_)));
                self.0.sent.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
                self.0.waker.register(cx.waker());
                if self.0.flushed.load(Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }

            fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
                self.poll_flush(cx)
            }
        }

        fn gated_websocket(
            state: Arc<GatedSinkState>,
        ) -> WebSocketKeepAlive<GatedSink, futures::stream::Pending<Result<Message, ()>>> {
            let mut websocket = WebSocketKeepAlive::from_sink_and_stream(
                futures::stream::pending(),
                GatedSink(state),
                Duration::from_secs(3600),
                Duration::from_millis(20),
            );
            websocket.ping_interval.as_mut().reset_immediately();
            websocket
        }

        #[test]
        #[test_r::timeout("5s")]
        async fn pending_ping_readiness_is_retried_without_another_interval_tick() {
            let state = Arc::new(GatedSinkState {
                flushed: AtomicBool::new(true),
                ..Default::default()
            });
            let mut websocket = gated_websocket(state.clone());
            tokio::time::sleep(Duration::from_millis(5)).await;
            assert!(futures::poll!(websocket.next()).is_pending());
            assert_eq!(state.sent.load(Ordering::SeqCst), 0);
            state.ready.store(true, Ordering::SeqCst);
            state.waker.wake();
            assert!(futures::poll!(websocket.next()).is_pending());
            assert_eq!(
                state.sent.load(Ordering::SeqCst),
                1,
                "the consumed interval tick must retain its pending Ping"
            );
        }

        #[test]
        #[test_r::timeout("5s")]
        async fn pong_deadline_starts_only_after_the_ping_flushes() {
            let state = Arc::new(GatedSinkState {
                ready: AtomicBool::new(true),
                ..Default::default()
            });
            let mut websocket = gated_websocket(state.clone());
            tokio::time::sleep(Duration::from_millis(5)).await;
            assert!(futures::poll!(websocket.next()).is_pending());
            assert_eq!(state.sent.load(Ordering::SeqCst), 1);
            tokio::time::sleep(Duration::from_millis(40)).await;
            assert!(
                futures::poll!(websocket.next()).is_pending(),
                "an unflushed Ping must not start a Pong deadline"
            );
            state.flushed.store(true, Ordering::SeqCst);
            state.waker.wake();
            assert!(futures::poll!(websocket.next()).is_pending());
            assert!(matches!(
                timeout(Duration::from_secs(1), websocket.next())
                    .await
                    .unwrap(),
                Some(Err(KeepAliveError::Timeout))
            ));
        }

        static TRACING_SETUP: Once = Once::new();

        fn setup_tracing() {
            TRACING_SETUP.call_once(|| {
                let subscriber = tracing_subscriber::FmtSubscriber::builder()
                    .with_max_level(Level::DEBUG)
                    .finish();

                tracing::subscriber::set_global_default(subscriber)
                    .expect("setting default subscriber failed");
            });
        }

        #[ignore]
        #[test]
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
            tokio::spawn(
                {
                    let pong_tx = pong_tx.clone();
                    let send_pong = send_pong.clone();
                    async move {
                        while ping_rx.recv().await.is_some() {
                            if send_pong.load(Ordering::Relaxed) {
                                pong_tx
                                    .send(Ok(Message::Pong(Vec::new())))
                                    .await
                                    .expect("Failed to send pong");
                            }
                        }
                    }
                }
                .in_current_span(),
            );

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
            assert!(next.is_err(), "Expected to receive a timeout error");

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
