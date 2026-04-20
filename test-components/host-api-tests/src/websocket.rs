use golem_rust::{
    PromiseId, WebSocketMessage, WebsocketConnection, agent_definition, agent_implementation,
};
use std::cell::RefCell;

#[agent_definition]
pub trait WebsocketTest {
    fn new(name: String) -> Self;
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
    /// Closes the persisted websocket and returns any close error to the caller.
    fn close_persisted_result(&self) -> Result<(), String>;

    /// Activates the agent without touching the persisted websocket.
    fn noop(&self) -> String;
    fn create_promise(&self) -> PromiseId;
    fn replay_reconnect_roundtrip(&self, url: String, barrier: PromiseId)
    -> Result<String, String>;
    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String>;
    async fn async_bidi_test(&self, url: String) -> Result<String, String>;

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
}

#[agent_implementation]
impl WebsocketTest for WebsocketTestImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            echo_history: RefCell::new(Vec::new()),
            persisted_ws: RefCell::new(None),
        }
    }

    fn echo(&self, url: String, msg: String) -> String {
        let ws = WebsocketConnection::connect(&url, None).expect("connect failed");

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
        let ws = WebsocketConnection::connect(&url, None).expect("connect failed");
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

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn replay_reconnect_roundtrip(
        &self,
        url: String,
        barrier: PromiseId,
    ) -> Result<String, String> {
        let ws = WebsocketConnection::connect(&url, None)
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
        let ws = WebsocketConnection::connect(&url, None).expect("connect failed");

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
        let ws = WebsocketConnection::connect(&url, None)
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

    fn poll_for_message(&self, url: String, timeout_ms: u64) -> Result<String, String> {
        let ws = WebsocketConnection::connect(&url, None)
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
        let ws = WebsocketConnection::connect(&url, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;

        for _ in 0..max_timeouts {
            match ws
                .blocking_receive_with_timeout(timeout_ms)
                .map_err(|e| format!("Receive error: {:?}", e))?
            {
                Some(WebSocketMessage::Text(text)) => return Ok(text),
                Some(WebSocketMessage::Binary(data)) => {
                    return Ok(format!("Binary: {} bytes", data.len()))
                }
                None => continue,
            }
        }

        Err(format!(
            "Timed out after {max_timeouts} polling attempts without receiving a message"
        ))
    }
}
