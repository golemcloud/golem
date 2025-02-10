use crate::services::rdbms::{RdbmsPoolKey, RdbmsType};
use bincode::{Decode, Encode};

#[derive(Debug, Clone, Encode, Decode)]
pub struct RdbmsRequest<T: RdbmsType + 'static> {
    pub pool_key: RdbmsPoolKey,
    pub statement: String,
    pub params: Vec<T::DbValue>,
}

impl<T: RdbmsType> RdbmsRequest<T> {
    pub fn new(pool_key: RdbmsPoolKey, statement: String, params: Vec<T::DbValue>) -> Self {
        Self {
            pool_key,
            statement,
            params,
        }
    }
}
