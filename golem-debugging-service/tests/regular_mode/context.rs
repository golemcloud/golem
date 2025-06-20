use crate::LastUniqueId;
use std::sync::atomic::Ordering;

#[derive(Clone)]
pub struct RegularExecutorTestContext {
    pub unique_id: u16,
}

impl RegularExecutorTestContext {
    pub fn new(last_unique_id: &LastUniqueId) -> Self {
        let unique_id = last_unique_id.id.fetch_add(1, Ordering::Relaxed);
        Self { unique_id }
    }

    pub fn redis_prefix(&self) -> String {
        format!("test-{}:", self.unique_id)
    }

    pub fn grpc_port(&self) -> u16 {
        9000 + (self.unique_id * 3)
    }

    pub fn http_port(&self) -> u16 {
        9001 + (self.unique_id * 3)
    }
}
