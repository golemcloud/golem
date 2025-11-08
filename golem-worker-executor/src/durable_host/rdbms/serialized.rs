// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::services::rdbms::RdbmsType;
use golem_common::model::oplog::types::SerializableRdbmsRequest;
use golem_common::model::{RdbmsPoolKey, TransactionId};

#[derive(Debug, Clone)]
pub struct RdbmsRequest<T: RdbmsType + 'static> {
    pub pool_key: RdbmsPoolKey,
    pub statement: String,
    pub params: Vec<T::DbValue>,
    pub transaction_id: Option<TransactionId>,
}

impl<T: RdbmsType> RdbmsRequest<T> {
    pub fn new(
        pool_key: RdbmsPoolKey,
        statement: String,
        params: Vec<T::DbValue>,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        Self {
            pool_key,
            statement,
            params,
            transaction_id,
        }
    }
}

impl<T: RdbmsType + 'static> From<RdbmsRequest<T>> for SerializableRdbmsRequest {
    fn from(request: RdbmsRequest<T>) -> Self {
        SerializableRdbmsRequest {
            pool_key: request.pool_key,
            statement: request.statement,
            params: request.params.into_iter().map(|p| p.into()).collect(),
            transaction_id: request.transaction_id,
        }
    }
}

impl<T: RdbmsType + 'static> TryFrom<SerializableRdbmsRequest> for RdbmsRequest<T> {
    type Error = String;

    fn try_from(value: SerializableRdbmsRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            pool_key: value.pool_key,
            statement: value.statement,
            params: value
                .params
                .into_iter()
                .map(|p| p.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            transaction_id: value.transaction_id,
        })
    }
}
