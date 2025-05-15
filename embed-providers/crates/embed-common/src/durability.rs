use golem_api_1_x::durability::LazyInitializedPollable;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
pub struct EmbeddingOperation {
    pub texts: Vec<String>,
    pub model: Option<String>,
    pub truncate: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RerankOperation {
    pub query: String,
    pub documents: Vec<String>,
    pub model: Option<String>,
    pub truncate: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Operation {
    Embed(EmbeddingOperation),
    Rerank(RerankOperation),
}

pub struct DurableRequest<T> {
    pub operation: Operation,
    pub pollable: Arc<LazyInitializedPollable>,
    pub result: Option<T>,
}

impl<T> DurableRequest<T> {
    pub fn new(operation: Operation, pollable: Arc<LazyInitializedPollable>) -> Self {
        Self {
            operation,
            pollable,
            result: None,
        }
    }

    pub fn with_result(operation: Operation, pollable: Arc<LazyInitializedPollable>, result: T) -> Self {
        Self {
            operation,
            pollable,
            result: Some(result),
        }
    }
}