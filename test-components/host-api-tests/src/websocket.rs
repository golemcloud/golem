use futures_concurrency::future::Join;
use golem_rust::websocket::{WebSocketError, WebSocketReconstructionPolicy};
use golem_rust::{
    PromiseId, WebSocketMessage, WebsocketConnection, agent_definition, agent_implementation,
};
use std::cell::RefCell;

#[agent_definition]
pub trait WebsocketTest {
    fn new(name: String) -> Self;
    fn connect_report_loss(&self, url: String) -> String;
    fn replace_report_loss(&self, url: String) -> Result<String, String>;
    fn connect_both_policies(&self, report_url: String, automatic_url: String) -> (String, String);
    fn probe_both_policies(&self) -> (bool, String);
    fn receive_is_closed(&self) -> bool;
    fn live_disconnect_is_not_session_lost(&self) -> bool;
    fn non_idempotent_report_connect(&self, url: String);
    fn atomic_report_connect(&self, url: String) -> String;
    async fn probe_report_loss(&self) -> (bool, bool, bool, bool);
    async fn discard_report_loss(&self, cancel: PromiseId) -> bool;
    fn drop_persisted(&self);
    fn echo(&self, url: String, msg: String) -> String;
    /// Like `echo`, but appends each echoed payload to agent-local history and returns `history.join("|")`.
    /// Used in tests to assert state survives replay across executor restarts.
    fn echo_and_record(&self, url: String, msg: String) -> String;
    /// Connects once, stores the connection in agent state and receives one message.
    fn connect_and_receive_first(&self, url: String) -> String;
    /// Receives the next message from the connection stored in agent state.
    fn receive_next_from_persisted(&self) -> String;
    /// Like `receive_next_from_persisted`, but returns websocket errors to the caller.
    fn receive_next_from_persisted_result(&self) -> Result<String, String>;
    /// Like `receive_next_from_persisted_result`, but sends a text frame on the
    /// connection stored in agent state and returns any send error to the caller.
    fn send_persisted_result(&self, message: String) -> Result<(), String>;
    /// Concurrent `receive` and `receive-with-timeout` on the connection stored in
    /// agent state, so both calls contend on reconnecting the same reconstructed
    /// handle. Each result is the received text, `"timeout"` if the timed receive
    /// expired, or `"Receive error: ..."` on failure.
    async fn receive_lock_contention_from_persisted(&self, timeout_ms: u64) -> (String, String);
    /// Closes the persisted websocket and returns any close error to the caller.
    fn close_persisted_result(&self) -> Result<(), String>;

    /// Activates the agent without touching the persisted websocket.
    fn noop(&self) -> String;
    fn create_promise(&self) -> PromiseId;
    fn replay_reconnect_roundtrip(&self, url: String, barrier: PromiseId)
    -> Result<String, String>;
    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String>;
    async fn async_bidi_test(&self, url: String) -> Result<String, String>;
    fn connect_result(&self, url: String) -> Result<(), String>;

    fn poll_for_message(&self, url: String, timeout_ms: u64) -> Result<String, String>;
    fn poll_until_message_after_timeouts(
        &self,
        url: String,
        timeout_ms: u64,
        max_timeouts: u32,
    ) -> Result<String, String>;
}

pub struct WebsocketTestImpl {
    _name: String,
    echo_history: RefCell<Vec<String>>,
    persisted_ws: RefCell<Option<WebsocketConnection>>,
    automatic_ws: RefCell<Option<WebsocketConnection>>,
}

#[agent_implementation]
impl WebsocketTest for WebsocketTestImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            echo_history: RefCell::new(Vec::new()),
            persisted_ws: RefCell::new(None),
            automatic_ws: RefCell::new(None),
        }
    }

    fn echo(&self, url: String, msg: String) -> String {
        let ws = WebsocketConnection::connect(&url, None, None).expect("connect failed");

        ws.send(&WebSocketMessage::Text(msg)).expect("send failed");

        match ws.blocking_receive().expect("receive failed") {
            WebSocketMessage::Text(t) => t,
            WebSocketMessage::Binary(b) => format!("{} bytes", b.len()),
        }
    }

    fn echo_and_record(&self, url: String, msg: String) -> String {
        let echoed = self.echo(url, msg);
        self.echo_history.borrow_mut().push(echoed);
        self.echo_history.borrow().join("|")
    }

    fn connect_and_receive_first(&self, url: String) -> String {
        let ws = WebsocketConnection::connect(&url, None, None).expect("connect failed");
        let first = match ws.blocking_receive().expect("receive failed") {
            WebSocketMessage::Text(t) => t,
            WebSocketMessage::Binary(b) => format!("{} bytes", b.len()),
        };
        *self.persisted_ws.borrow_mut() = Some(ws);
        first
    }

    fn receive_next_from_persisted(&self) -> String {
        let mut ws_ref = self.persisted_ws.borrow_mut();
        let ws = ws_ref
            .as_mut()
            .expect("persisted websocket was not initialized");
        match ws.blocking_receive().expect("receive failed") {
            WebSocketMessage::Text(t) => t,
            WebSocketMessage::Binary(b) => format!("{} bytes", b.len()),
        }
    }

    fn receive_next_from_persisted_result(&self) -> Result<String, String> {
        let mut ws_ref = self.persisted_ws.borrow_mut();
        let ws = ws_ref
            .as_mut()
            .expect("persisted websocket was not initialized");
        match ws
            .blocking_receive()
            .map_err(|e| format!("Receive error: {:?}", e))?
        {
            WebSocketMessage::Text(t) => Ok(t),
            WebSocketMessage::Binary(b) => Ok(format!("{} bytes", b.len())),
        }
    }

    fn send_persisted_result(&self, message: String) -> Result<(), String> {
        let mut ws_ref = self.persisted_ws.borrow_mut();
        let ws = ws_ref
            .as_mut()
            .expect("persisted websocket was not initialized");
        ws.send(&WebSocketMessage::Text(message))
            .map_err(|e| format!("Send error: {:?}", e))
    }

    async fn receive_lock_contention_from_persisted(&self, timeout_ms: u64) -> (String, String) {
        let ws = self.persisted_ws.borrow();
        let ws = ws
            .as_ref()
            .expect("persisted websocket was not initialized");
        fn text(message: WebSocketMessage) -> String {
            match message {
                WebSocketMessage::Text(t) => t,
                WebSocketMessage::Binary(b) => format!("{} bytes", b.len()),
            }
        }
        let (received, timed) = (ws.receive(), ws.receive_with_timeout(timeout_ms))
            .join()
            .await;
        let received = received.map(text).unwrap_or_else(|e| match e {
            WebSocketError::SessionLost => "session-lost".to_string(),
            e => format!("Receive error: {e:?}"),
        });
        let timed = match timed {
            Ok(Some(message)) => text(message),
            Ok(None) => "timeout".to_string(),
            Err(WebSocketError::SessionLost) => "session-lost".to_string(),
            Err(e) => format!("Receive error: {e:?}"),
        };
        (received, timed)
    }

    fn close_persisted_result(&self) -> Result<(), String> {
        let mut ws_ref = self.persisted_ws.borrow_mut();
        let ws = ws_ref
            .as_mut()
            .expect("persisted websocket was not initialized");
        ws.close(None, None)
            .map_err(|e| format!("Close error: {:?}", e))
    }

    fn noop(&self) -> String {
        "ok".to_string()
    }

    fn connect_report_loss(&self, url: String) -> String {
        let ws = WebsocketConnection::connect(
            &url,
            None,
            Some(WebSocketReconstructionPolicy::ReportConnectionLoss),
        )
        .expect("connect failed");
        let WebSocketMessage::Text(first) = ws.blocking_receive().expect("receive failed") else {
            panic!("expected text greeting");
        };
        ws.send(&WebSocketMessage::Text("completed-before-crash".into()))
            .expect("send failed");
        assert!(
            ws.blocking_receive_with_timeout(0)
                .expect("timeout failed")
                .is_none()
        );
        *self.persisted_ws.borrow_mut() = Some(ws);
        first
    }

    fn replace_report_loss(&self, url: String) -> Result<String, String> {
        let ws = WebsocketConnection::connect(
            &url,
            None,
            Some(WebSocketReconstructionPolicy::ReportConnectionLoss),
        )
        .map_err(|error| format!("Connect error: {error:?}"))?;
        let first = ws
            .blocking_receive()
            .map_err(|error| format!("Initialize error: {error:?}"));
        *self.persisted_ws.borrow_mut() = Some(ws);
        match first? {
            WebSocketMessage::Text(text) => Ok(text),
            WebSocketMessage::Binary(_) => panic!("expected text greeting"),
        }
    }

    fn connect_both_policies(&self, report_url: String, automatic_url: String) -> (String, String) {
        let report = self.connect_report_loss(report_url);
        let automatic = WebsocketConnection::connect(
            &automatic_url,
            None,
            Some(WebSocketReconstructionPolicy::ReconnectAutomatically),
        )
        .expect("connect failed");
        let WebSocketMessage::Text(first) = automatic.blocking_receive().expect("receive failed")
        else {
            panic!("expected text greeting");
        };
        *self.automatic_ws.borrow_mut() = Some(automatic);
        (report, first)
    }

    fn probe_both_policies(&self) -> (bool, String) {
        let report = self.persisted_ws.borrow();
        let lost = matches!(
            report
                .as_ref()
                .expect("report websocket missing")
                .blocking_receive_with_timeout(0),
            Err(WebSocketError::SessionLost)
        );
        let automatic = self.automatic_ws.borrow();
        let WebSocketMessage::Text(message) = automatic
            .as_ref()
            .expect("automatic websocket missing")
            .blocking_receive()
            .expect("automatic reconnect failed")
        else {
            panic!("expected text greeting");
        };
        (lost, message)
    }

    async fn probe_report_loss(&self) -> (bool, bool, bool, bool) {
        let ws = self.persisted_ws.borrow();
        let ws = ws
            .as_ref()
            .expect("persisted websocket was not initialized");
        let (received, timed) = (ws.receive(), ws.receive_with_timeout(0)).join().await;
        (
            matches!(received, Err(WebSocketError::SessionLost)),
            matches!(timed, Err(WebSocketError::SessionLost)),
            matches!(
                ws.send(&WebSocketMessage::Text("must-not-send".into())),
                Err(WebSocketError::SessionLost)
            ),
            matches!(ws.close(None, None), Err(WebSocketError::SessionLost)),
        )
    }

    async fn discard_report_loss(&self, cancel: PromiseId) -> bool {
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        let ws = self.persisted_ws.borrow();
        let ws = ws.as_ref().expect("persisted websocket missing");
        let mut receive = Box::pin(ws.receive());
        poll_fn(|cx| match receive.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(result) => panic!("discarded receive was redelivered: {result:?}"),
        })
        .await;
        golem_rust::await_promise(&cancel).await;
        poll_fn(|cx| match receive.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(result) => panic!("discarded receive was redelivered: {result:?}"),
        })
        .await;
        drop(receive);
        // The exclusive clock call acknowledges that guest cancellation has run.
        let _ = wasi::clocks::wall_clock::now();
        matches!(
            ws.blocking_receive_with_timeout(0),
            Err(WebSocketError::SessionLost)
        )
    }

    fn drop_persisted(&self) {
        self.persisted_ws.borrow_mut().take();
    }

    fn receive_is_closed(&self) -> bool {
        let ws = self.persisted_ws.borrow();
        matches!(
            ws.as_ref()
                .expect("persisted websocket missing")
                .blocking_receive(),
            Err(WebSocketError::Closed(_))
        )
    }

    fn live_disconnect_is_not_session_lost(&self) -> bool {
        let ws = self.persisted_ws.borrow();
        match ws
            .as_ref()
            .expect("persisted websocket missing")
            .blocking_receive()
        {
            Err(WebSocketError::SessionLost) | Ok(_) => false,
            Err(_) => true,
        }
    }

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn non_idempotent_report_connect(&self, url: String) {
        let _guard = golem_rust::use_idempotence_mode(false);
        let _ws = WebsocketConnection::connect(
            &url,
            None,
            Some(WebSocketReconstructionPolicy::ReportConnectionLoss),
        )
        .expect("connect failed");
    }

    fn atomic_report_connect(&self, url: String) -> String {
        let _guard = golem_rust::use_idempotence_mode(false);
        golem_rust::atomically(|| {
            let ws = WebsocketConnection::connect(
                &url,
                None,
                Some(WebSocketReconstructionPolicy::ReportConnectionLoss),
            )
            .expect("connect failed");
            let WebSocketMessage::Text(first) = ws.blocking_receive().expect("receive failed")
            else {
                panic!("expected text greeting");
            };
            first
        })
    }

    fn replay_reconnect_roundtrip(
        &self,
        url: String,
        barrier: PromiseId,
    ) -> Result<String, String> {
        let ws = WebsocketConnection::connect(&url, None, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;
        let mut received = Vec::new();

        for payload in ["msg-1", "msg-2"] {
            ws.send(&WebSocketMessage::Text(payload.to_string()))
                .map_err(|e| format!("Send error: {:?}", e))?;
            let message = ws
                .blocking_receive()
                .map_err(|e| format!("Receive error: {:?}", e))?;
            match message {
                WebSocketMessage::Text(text) => received.push(text),
                WebSocketMessage::Binary(data) => {
                    received.push(format!("Binary: {} bytes", data.len()))
                }
            }
        }

        // Suspending on a promise gives the recovery test a deterministic crash boundary.
        let _ = golem_rust::blocking_await_promise(&barrier);

        for payload in ["msg-3", "msg-4"] {
            ws.send(&WebSocketMessage::Text(payload.to_string()))
                .map_err(|e| format!("Send error: {:?}", e))?;
            let message = ws
                .blocking_receive()
                .map_err(|e| format!("Receive error: {:?}", e))?;
            match message {
                WebSocketMessage::Text(text) => received.push(text),
                WebSocketMessage::Binary(data) => {
                    received.push(format!("Binary: {} bytes", data.len()))
                }
            }
        }

        Ok(received.join("|"))
    }

    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String> {
        let ws = WebsocketConnection::connect(&url, None, None).expect("connect failed");

        match ws
            .blocking_receive_with_timeout(timeout_ms)
            .expect("receive failed")
        {
            Some(WebSocketMessage::Text(t)) => Some(t),
            Some(WebSocketMessage::Binary(b)) => Some(format!("{} bytes", b.len())),
            None => None,
        }
    }

    async fn async_bidi_test(&self, url: String) -> Result<String, String> {
        let ws = WebsocketConnection::connect(&url, None, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;

        let payloads = ["msg-a", "msg-b", "msg-c"];
        let mut received = Vec::new();

        for payload in payloads {
            ws.send(&WebSocketMessage::Text(payload.to_string()))
                .map_err(|e| format!("Send error: {:?}", e))?;

            let msg = ws
                .receive()
                .await
                .map_err(|e| format!("Receive error: {:?}", e))?;
            match msg {
                WebSocketMessage::Text(text) => received.push(text),
                WebSocketMessage::Binary(data) => {
                    received.push(format!("Binary: {} bytes", data.len()))
                }
            }
        }

        Ok(received.join("|"))
    }

    fn connect_result(&self, url: String) -> Result<(), String> {
        WebsocketConnection::connect(&url, None, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn poll_for_message(&self, url: String, timeout_ms: u64) -> Result<String, String> {
        let ws = WebsocketConnection::connect(&url, None, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;
        match ws
            .blocking_receive_with_timeout(timeout_ms)
            .map_err(|e| format!("Receive error: {:?}", e))?
        {
            Some(WebSocketMessage::Text(text)) => Ok(text),
            Some(WebSocketMessage::Binary(data)) => Ok(format!("Binary: {} bytes", data.len())),
            None => Err("Timeout waiting for message".to_string()),
        }
    }

    fn poll_until_message_after_timeouts(
        &self,
        url: String,
        timeout_ms: u64,
        max_timeouts: u32,
    ) -> Result<String, String> {
        let ws = WebsocketConnection::connect(&url, None, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;

        for _ in 0..max_timeouts {
            match ws
                .blocking_receive_with_timeout(timeout_ms)
                .map_err(|e| format!("Receive error: {:?}", e))?
            {
                Some(WebSocketMessage::Text(text)) => return Ok(text),
                Some(WebSocketMessage::Binary(data)) => {
                    return Ok(format!("Binary: {} bytes", data.len()));
                }
                None => continue,
            }
        }

        Err(format!(
            "Timed out after {max_timeouts} polling attempts without receiving a message"
        ))
    }
}
