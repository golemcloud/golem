// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::services::rdbms::{RdbmsPoolKey, RdbmsType};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value};

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

impl<T: RdbmsType + 'static> IntoValue for RdbmsRequest<T> {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.pool_key.into_value(),
            self.statement.into_value(),
            self.params.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("pool-key", RdbmsPoolKey::get_type()),
            analysed_type::field("statement", analysed_type::str()),
            analysed_type::field("params", analysed_type::list(T::DbValue::get_type())),
        ])
    }
}
