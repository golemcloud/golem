use golem_rust::{agent_definition, agent_implementation, WebSocketMessage, WebsocketConnection};
use wasi::io::poll;

#[agent_definition]
pub trait WebsocketTest {
    fn new(name: String) -> Self;
    fn echo(&self, url: String, msg: String) -> String;
    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String>;

    // New polling methods
    fn poll_for_message(&self, url: String, timeout_ms: u64) -> Result<String, String>;
    fn poll_multiple_messages(
        &self,
        url: String,
        count: u32,
        timeout_ms: u64,
    ) -> Result<Vec<String>, String>;
}

pub struct WebsocketTestImpl {
    _name: String,
}

#[agent_implementation]
impl WebsocketTest for WebsocketTestImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn echo(&self, url: String, msg: String) -> String {
        let ws = WebsocketConnection::connect(&url, None).expect("connect failed");

        ws.send(&WebSocketMessage::Text(msg)).expect("send failed");

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

    fn poll_multiple_messages(
        &self,
        url: String,
        count: u32,
        timeout_ms: u64,
    ) -> Result<Vec<String>, String> {
        // Connect to the WebSocket
        let ws = WebsocketConnection::connect(&url, None)
            .map_err(|e| format!("Failed to connect: {:?}", e))?;

        let mut messages = Vec::new();

        for i in 0..count {
            println!("Waiting for message {}/{}", i + 1, count);

            // Get a pollable for the WebSocket connection
            let pollable = ws.subscribe();

            // Create a timeout for each message
            let clock = wasi::clocks::monotonic_clock::subscribe_duration(timeout_ms * 1_000_000);

            // Poll for either data or timeout
            let ready_list = poll::poll(&[&pollable, &clock]);

            if ready_list.contains(&0) {
                // WebSocket is ready
                match ws.receive() {
                    Ok(WebSocketMessage::Text(text)) => {
                        println!("Received: {}", text);
                        messages.push(text);
                    }
                    Ok(WebSocketMessage::Binary(data)) => {
                        let msg = format!("Binary: {} bytes", data.len());
                        println!("Received: {}", msg);
                        messages.push(msg);
                    }
                    Err(e) => {
                        return Err(format!("Receive error on message {}: {:?}", i + 1, e));
                    }
                }
            } else if ready_list.contains(&1) {
                // Timeout occurred
                return Err(format!("Timeout waiting for message {}", i + 1));
            } else {
                return Err("Unexpected poll result".to_string());
            }
        }

        // Close the connection gracefully
        ws.close(Some(1000), Some("Normal closure"))
            .map_err(|e| format!("Failed to close: {:?}", e))?;

        Ok(messages)
    }
}
