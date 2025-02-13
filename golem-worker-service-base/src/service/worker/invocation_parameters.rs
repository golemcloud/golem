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

use golem_wasm_rpc::json::OptionallyTypeAnnotatedValueJson;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub enum InvocationParameters {
    TypedProtoVals(Vec<TypeAnnotatedValue>),
    RawJsonStrings(Vec<String>),
}

impl InvocationParameters {
    pub fn from_optionally_type_annotated_value_jsons(
        values: Vec<OptionallyTypeAnnotatedValueJson>,
    ) -> Result<Self, Vec<String>> {
        let all_have_types = values.iter().all(|v| v.has_type());
        if all_have_types {
            let vals: Vec<TypeAnnotatedValue> = values
                .into_iter()
                .map(|param| param.try_into_type_annotated_value())
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|param| param.unwrap()) // This is expected to always succeed because of the `all_have_types` condition
                .collect();
            Ok(Self::TypedProtoVals(vals))
        } else {
            let vals: Vec<String> = values
                .into_iter()
                .map(|param| param.into_json_value().to_string())
                .collect();
            Ok(Self::RawJsonStrings(vals))
        }
    }
}
