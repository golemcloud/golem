use golem_rust::{WebSocketMessage, WebsocketConnection, agent_definition, agent_implementation};
use std::cell::RefCell;
use wasi::io::poll;

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

        match ws.receive().expect("receive failed") {
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
        let first = match ws.receive().expect("receive failed") {
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
        match ws.receive().expect("receive failed") {
            WebSocketMessage::Text(t) => t,
            WebSocketMessage::Binary(b) => format!("{} bytes", b.len()),
        }
    }

    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String> {
        let ws = WebsocketConnection::connect(&url, None).expect("connect failed");

        match ws.receive_with_timeout(timeout_ms).expect("receive failed") {
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

            // Convert websocket pollable to an async future via wstd.
            let pollable = ws.subscribe();
            wstd::io::AsyncPollable::new(pollable).wait_for().await;

            let msg = ws
                .receive()
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
        let pollable = ws.subscribe();
        let clock = wasi::clocks::monotonic_clock::subscribe_duration(timeout_ms * 1_000_000);
        let ready_list = poll::poll(&[&pollable, &clock]);

        if ready_list.contains(&0) {
            match ws.receive() {
                Ok(WebSocketMessage::Text(text)) => Ok(text),
                Ok(WebSocketMessage::Binary(data)) => Ok(format!("Binary: {} bytes", data.len())),
                Err(e) => Err(format!("Receive error: {:?}", e)),
            }
        } else if ready_list.contains(&1) {
            Err("Timeout waiting for message".to_string())
        } else {
            Err("Unexpected poll result".to_string())
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
            let pollable = ws.subscribe();
            let clock = wasi::clocks::monotonic_clock::subscribe_duration(timeout_ms * 1_000_000);
            let ready_list = poll::poll(&[&pollable, &clock]);

            if ready_list.contains(&0) {
                return match ws.receive() {
                    Ok(WebSocketMessage::Text(text)) => Ok(text),
                    Ok(WebSocketMessage::Binary(data)) => {
                        Ok(format!("Binary: {} bytes", data.len()))
                    }
                    Err(e) => Err(format!("Receive error: {:?}", e)),
                };
            }

            if !ready_list.contains(&1) {
                return Err("Unexpected poll result".to_string());
            }
        }

        Err(format!(
            "Timed out after {max_timeouts} polling attempts without receiving a message"
        ))
    }
}
