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

use crate::services::rdbms::{RdbmsIntoValueAndType, RdbmsPoolKey, RdbmsType};
use desert_rust::BinaryCodec;
use golem_common::model::TransactionId;
use golem_wasm::analysis::{analysed_type, AnalysedType};
use golem_wasm::{IntoValue, Value, ValueAndType};

#[derive(Debug, Clone, BinaryCodec)]
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

    fn get_analysed_type(params_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("pool-key", RdbmsPoolKey::get_type()),
            analysed_type::field("statement", analysed_type::str()),
            analysed_type::field("params", params_type),
            analysed_type::field(
                "transaction-id",
                analysed_type::option(TransactionId::get_type()),
            ),
        ])
    }
}

impl<T> RdbmsIntoValueAndType for RdbmsRequest<T>
where
    T: RdbmsType + 'static,
    Vec<T::DbValue>: RdbmsIntoValueAndType,
{
    fn into_value_and_type_rdbms(self) -> ValueAndType {
        let v = self.params.into_value_and_type_rdbms();
        let t = RdbmsRequest::<T>::get_analysed_type(v.typ);
        let v = Value::Record(vec![
            self.pool_key.into_value(),
            self.statement.into_value(),
            v.value,
            self.transaction_id.into_value(),
        ]);
        ValueAndType::new(v, t)
    }

    fn get_base_type() -> AnalysedType {
        RdbmsRequest::<T>::get_analysed_type(<Vec<T::DbValue>>::get_base_type())
    }
}
