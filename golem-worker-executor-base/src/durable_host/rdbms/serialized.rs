use crate::services::rdbms::RdbmsPoolKey;
use bincode::{Decode, Encode};

#[derive(Debug, Clone, Encode, Decode)]
pub struct RdbmsRequest<T: 'static> {
    pub pool_key: RdbmsPoolKey,
    pub statement: String,
    pub params: Vec<T>,
}

impl<T> RdbmsRequest<T> {
    pub fn new(pool_key: RdbmsPoolKey, statement: String, params: Vec<T>) -> Self {
        Self {
            pool_key,
            statement,
            params,
        }
    }
}
