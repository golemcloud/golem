// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::bindings::golem::websocket::client::{
    Error, Message, WebsocketConnection as RawWebsocketConnection,
};

pub use crate::bindings::golem::websocket::client::{
    CloseInfo as WebSocketCloseInfo, Error as WebSocketError, Message as WebSocketMessage,
};

/// A WebSocket connection with both blocking and async receive methods.
pub struct WebsocketConnection {
    inner: RawWebsocketConnection,
}

impl WebsocketConnection {
    /// Connect to a WebSocket server at the given URL (ws:// or wss://).
    /// Optional headers for auth, subprotocols, etc.
    pub fn connect(
        url: &str,
        headers: Option<Vec<(String, String)>>,
    ) -> Result<Self, Error> {
        RawWebsocketConnection::connect(url, headers.as_deref())
            .map(|inner| Self { inner })
    }

    /// Send a message (text or binary).
    pub fn send(&self, message: &Message) -> Result<(), Error> {
        self.inner.send(message)
    }

    /// Receive the next message, blocking until one is available.
    pub fn blocking_receive(&self) -> Result<Message, Error> {
        self.inner.receive()
    }

    /// Receive the next message, blocking with a timeout in milliseconds.
    /// Returns `None` if the timeout expires before a message arrives.
    pub fn blocking_receive_with_timeout(
        &self,
        timeout_ms: u64,
    ) -> Result<Option<Message>, Error> {
        self.inner.receive_with_timeout(timeout_ms)
    }

    /// Receive the next message asynchronously.
    /// Yields the current task until a message is available.
    pub async fn receive(&self) -> Result<Message, Error> {
        let pollable = self.inner.subscribe();
        wstd::io::AsyncPollable::new(pollable).wait_for().await;
        self.inner.receive()
    }

    /// Send a close frame with optional code and reason.
    pub fn close(&self, code: Option<u16>, reason: Option<String>) -> Result<(), Error> {
        self.inner.close(code, reason.as_deref())
    }
}
