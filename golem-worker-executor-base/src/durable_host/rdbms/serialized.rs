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
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value, ValueAndType};

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

    fn get_analysed_type(params_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("pool-key", RdbmsPoolKey::get_type()),
            analysed_type::field("statement", analysed_type::str()),
            analysed_type::field("params", params_type),
        ])
    }
}

impl<T> RdbmsIntoValueAndType for RdbmsRequest<T>
where
    T: RdbmsType + 'static,
    Vec<T::DbValue>: RdbmsIntoValueAndType,
{
    fn into_value_and_type(self) -> ValueAndType {
        let v = self.params.into_value_and_type();
        let t = RdbmsRequest::<T>::get_analysed_type(v.typ);
        let v = Value::Record(vec![
            self.pool_key.into_value(),
            self.statement.into_value(),
            v.value,
        ]);
        ValueAndType::new(v, t)
    }

    fn get_base_type() -> AnalysedType {
        RdbmsRequest::<T>::get_analysed_type(<Vec<T::DbValue>>::get_base_type())
    }
}
