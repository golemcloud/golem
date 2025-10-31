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

use crate::value_and_type::FromValueAndType;
use crate::value_and_type::IntoValue;
use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
use golem_wasm::golem_rpc_0_2_x::types::WitType;
use golem_wasm::golem_rpc_0_2_x::types::WitValue;

pub trait Schema: IntoValue + FromValueAndType {
    fn from_wit_value_and_type(wit_value: WitValue, wit_type: WitType) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value_and_type = ValueAndType {
            value: wit_value,
            typ: wit_type,
        };
        Self::from_value_and_type(value_and_type)
    }
}

impl<T: IntoValue + FromValueAndType> Schema for T {}
