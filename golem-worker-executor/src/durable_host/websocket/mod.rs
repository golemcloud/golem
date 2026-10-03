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

pub mod client;

#[cfg(feature = "test-utils")]
pub mod reconnect_test;

#[cfg(feature = "test-utils")]
pub use reconnect_test::{
    WebSocketReconnectEventForTest, WebSocketReconnectObservationForTest,
    WebSocketReconnectObservationInnerForTest,
};

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[cfg(feature = "test-utils")]
use self::reconnect_test::{
    WebSocketReconnectHandshakeObserverForTest, WebSocketReconnectOutcomeForTest,
    WebSocketReconnectPathForTest, WebSocketReconnectWaitForTest,
    WebSocketReconnectWaitObserverForTest,
};

#[cfg(feature = "test-utils")]
use golem_common::model::{IdempotencyKey, OplogIndex};

/// A per-executor connection pool that limits the number of concurrent
/// WebSocket connections, preventing socket exhaustion under load.
///
/// Modeled after `wasmtime_wasi_http::HttpConnectionPool` — callers acquire
/// a permit before establishing a connection. The permit is held for the
/// lifetime of the connection and released when the connection is dropped.
#[derive(Clone)]
pub struct WebSocketConnectionPool {
    semaphore: Arc<Semaphore>,
    #[cfg(feature = "test-utils")]
    reconnect_observation: Arc<std::sync::Mutex<Option<WebSocketReconnectObservationInnerForTest>>>,
}

impl WebSocketConnectionPool {
    pub fn new(max_connections: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_connections)),
            #[cfg(feature = "test-utils")]
            reconnect_observation: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Acquires a permit, blocking if the pool is at capacity.
    /// The returned permit must be held for the lifetime of the connection.
    pub async fn acquire(&self) -> anyhow::Result<OwnedSemaphorePermit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("WebSocket connection pool closed unexpectedly"))
    }

    /// Installs a test observation of the websocket reconnect path. Only one
    /// observation may be installed per pool at a time.
    #[cfg(feature = "test-utils")]
    pub fn install_websocket_reconnect_observation_for_test(
        &self,
        inner: WebSocketReconnectObservationInnerForTest,
    ) {
        assert!(
            self.reconnect_observation
                .lock()
                .unwrap()
                .replace(inner)
                .is_none(),
            "a websocket reconnect observation is already installed"
        );
    }

    #[cfg(feature = "test-utils")]
    fn reconnect_observation(&self) -> Option<WebSocketReconnectObservationInnerForTest> {
        self.reconnect_observation.lock().unwrap().clone()
    }

    #[cfg(feature = "test-utils")]
    fn reconnect_event(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
        path: WebSocketReconnectPathForTest,
        kind: self::reconnect_test::WebSocketReconnectEventKindForTest,
        wait_state: Option<Arc<std::sync::atomic::AtomicU8>>,
    ) -> WebSocketReconnectEventForTest {
        WebSocketReconnectEventForTest {
            invocation_key,
            start,
            path,
            kind,
            wait_state,
        }
    }

    /// Emits a `Decided` event for a call that, holding the per-resource
    /// reconnect coordination, decided to perform the live reconnect.
    #[cfg(feature = "test-utils")]
    pub(crate) fn emit_websocket_reconnect_decided_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
        path: WebSocketReconnectPathForTest,
    ) {
        if let Some(observation) = self.reconnect_observation() {
            let _ = observation.notify.send(self.reconnect_event(
                invocation_key,
                start,
                path,
                self::reconnect_test::WebSocketReconnectEventKindForTest::Decided,
                None,
            ));
        }
    }

    /// Emits a `Published` event for the outcome a reconnect published into
    /// the connection table.
    #[cfg(feature = "test-utils")]
    pub(crate) fn emit_websocket_reconnect_published_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
        path: WebSocketReconnectPathForTest,
        outcome: WebSocketReconnectOutcomeForTest,
    ) {
        if let Some(observation) = self.reconnect_observation() {
            let _ = observation.notify.send(self.reconnect_event(
                invocation_key,
                start,
                path,
                self::reconnect_test::WebSocketReconnectEventKindForTest::Published { outcome },
                None,
            ));
        }
    }

    /// Returns an observer for a reconnect-path wait (gate or pool permit),
    /// or `None` when no observation is installed.
    #[cfg(feature = "test-utils")]
    pub(crate) fn websocket_reconnect_wait_observer_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
        path: WebSocketReconnectPathForTest,
        wait: WebSocketReconnectWaitForTest,
    ) -> Option<WebSocketReconnectWaitObserverForTest> {
        let observation = self.reconnect_observation()?;
        let state = Arc::new(std::sync::atomic::AtomicU8::new(0));
        Some(WebSocketReconnectWaitObserverForTest {
            notify: observation.notify,
            event: Some(self.reconnect_event(
                invocation_key,
                start,
                path,
                self::reconnect_test::WebSocketReconnectEventKindForTest::WaitPending { wait },
                Some(state.clone()),
            )),
            state,
        })
    }

    /// Returns an observer for the reconnect handshake that first parks on a
    /// test-held admission gate, or `None` when no observation is installed.
    #[cfg(feature = "test-utils")]
    pub(crate) fn websocket_reconnect_handshake_observer_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
        path: WebSocketReconnectPathForTest,
    ) -> Option<WebSocketReconnectHandshakeObserverForTest> {
        let observation = self.reconnect_observation()?;
        let state = Arc::new(std::sync::atomic::AtomicU8::new(0));
        Some(WebSocketReconnectHandshakeObserverForTest {
            wait: WebSocketReconnectWaitObserverForTest {
                notify: observation.notify,
                event: Some(self.reconnect_event(
                    invocation_key,
                    start,
                    path,
                    self::reconnect_test::WebSocketReconnectEventKindForTest::WaitPending {
                        wait: WebSocketReconnectWaitForTest::Handshake,
                    },
                    Some(state.clone()),
                )),
                state,
            },
            admission: observation.handshake_release,
        })
    }
}
