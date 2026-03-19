use golem_rust::{agent_definition, agent_implementation};
use golem_rust::bindings::golem::websocket::client::{WebsocketConnection, Message};

#[agent_definition]
pub trait WebsocketTest {
    fn new(name: String) -> Self;
    fn echo(&self, url: String, msg: String) -> String;
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
        ws.send(&Message::Text(msg)).expect("send failed");
        match ws.receive().expect("receive failed") {
            Message::Text(t) => t,
            Message::Binary(b) => format!("{} bytes", b.len()),
        }
    }
}
