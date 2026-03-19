use golem_rust::{agent_definition, agent_implementation, WebSocketMessage, WebsocketConnection};

#[agent_definition]
pub trait WebsocketTest {
    fn new(name: String) -> Self;
    fn echo(&self, url: String, msg: String) -> String;
    fn receive_with_timeout_test(&self, url: String, timeout_ms: u64) -> Option<String>;
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
}
