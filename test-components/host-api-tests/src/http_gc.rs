//! Snapshot restoration changes only resource release timing, as a restored GC heap can.

use crate::raw_wasi_http::send_http_request;
use golem_rust::wasip3::http::types;
use golem_rust::{PromiseId, agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
pub trait HttpGc {
    fn new(name: String) -> Self;
    fn configure(&mut self, live: u8, restored: u8);
    fn create_promise(&self) -> PromiseId;
    async fn run(&mut self, promise: PromiseId, partial_body: bool) -> u16;
    fn inspect(&self) -> (bool, u16);
    fn release(&mut self);
}

struct HttpGcImpl {
    drop_at: u8,
    after_restore: u8,
    restored: bool,
    status: u16,
    retained: Option<Box<dyn std::any::Any>>,
}

#[agent_implementation]
impl HttpGc for HttpGcImpl {
    fn new(_name: String) -> Self {
        Self {
            drop_at: 0,
            after_restore: 0,
            restored: false,
            status: 0,
            retained: None,
        }
    }

    fn configure(&mut self, live: u8, restored: u8) {
        self.drop_at = live;
        self.after_restore = restored;
    }

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    async fn run(&mut self, promise: PromiseId, partial_body: bool) -> u16 {
        let response = send_http_request("/drop-unread").await;
        self.status = response.get_status_code();
        self.retained = if partial_body {
            let (ack, ack_reader) =
                golem_rust::wasip3::wit_future::new::<Result<(), types::ErrorCode>>(|| Ok(()));
            let (mut body, trailers) = types::Response::consume_body(response, ack_reader);
            assert_eq!(body.next().await, Some(b'u'));
            Some(Box::new((body, trailers, ack)))
        } else {
            Some(Box::new(response))
        };
        if self.drop_at == 0 {
            self.retained.take();
        }
        golem_rust::retry::remove_retry_policy("http-gc-sentinel");
        if self.drop_at == 1 {
            self.retained.take();
        }
        golem_rust::blocking_await_promise(&promise);
        self.status
    }

    fn inspect(&self) -> (bool, u16) {
        (self.restored, self.status)
    }

    fn release(&mut self) {
        self.retained.take();
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&(self.after_restore, self.status)).map_err(|e| e.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let (after_restore, status) = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        Ok(Self {
            drop_at: after_restore,
            after_restore,
            restored: true,
            status,
            retained: None,
        })
    }
}
