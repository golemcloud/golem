use golem_rust::{agent_definition, agent_implementation, WebSocketMessage, WebsocketConnection};
use std::cell::RefCell;
use wasi::io::poll;

#[agent_definition]
pub trait WebsocketTest {
    fn new(name: String) -> Self;
    fn echo(&self, url: String, msg: String) -> String;
    /// Like `echo`, but appends each echoed payload to agent-local history and returns `history.join("|")`.
    /// Used in tests to assert state survives replay across executor restarts.
    fn echo_and_record(&self, url: String, msg: String) -> String;
    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String>;

    // New polling methods
    fn poll_for_message(&self, url: String, timeout_ms: u64) -> Result<String, String>;
}

pub struct WebsocketTestImpl {
    _name: String,
    echo_history: RefCell<Vec<String>>,
}

#[agent_implementation]
impl WebsocketTest for WebsocketTestImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            echo_history: RefCell::new(Vec::new()),
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

    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String> {
        let ws = WebsocketConnection::connect(&url, None).expect("connect failed");

        match ws.receive_with_timeout(timeout_ms).expect("receive failed") {
            Some(WebSocketMessage::Text(t)) => Some(t),
            Some(WebSocketMessage::Binary(b)) => Some(format!("{} bytes", b.len())),
            None => None,
        }
    }

    fn poll_for_message(&self, url: String, timeout_ms: u64) -> Result<String, String> {
        // Connect to the WebSocket
        let ws = WebsocketConnection::connect(&url, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;

        // Get a pollable for the WebSocket connection
        let pollable = ws.subscribe();

        // Create a monotonic clock for timeout
        let clock = wasi::clocks::monotonic_clock::subscribe_duration(timeout_ms * 1_000_000); // Convert ms to ns

        // Poll for either data or timeout
        let ready_list = poll::poll(&[&pollable, &clock]);

        if ready_list.contains(&0) {
            // WebSocket is ready, try to receive
            match ws.receive() {
                Ok(WebSocketMessage::Text(text)) => Ok(text),
                Ok(WebSocketMessage::Binary(data)) => Ok(format!("Binary: {} bytes", data.len())),
                Err(e) => Err(format!("Receive error: {:?}", e)),
            }
        } else if ready_list.contains(&1) {
            // Timeout occurred
            Err("Timeout waiting for message".to_string())
        } else {
            Err("Unexpected poll result".to_string())
        }
    }
}
