use crate::regular_mode::context::RegularExecutorTestContext;

pub struct DebugExecutorTestContext {
    regular_worker_executor_context: RegularExecutorTestContext,
}

impl DebugExecutorTestContext {
    pub fn from(regular_context: &RegularExecutorTestContext) -> Self {
        Self {
            regular_worker_executor_context: regular_context.clone(),
        }
    }

    pub fn redis_prefix(&self) -> String {
        self.regular_worker_executor_context.redis_prefix()
    }

    pub fn regular_worker_executor_context(&self) -> RegularExecutorTestContext {
        self.regular_worker_executor_context.clone()
    }

    pub fn debug_server_port(&self) -> u16 {
        8050 + (self.regular_worker_executor_context.unique_id * 3)
    }

    pub fn http_port(&self) -> u16 {
        8051 + (self.regular_worker_executor_context.unique_id * 3)
    }
}
