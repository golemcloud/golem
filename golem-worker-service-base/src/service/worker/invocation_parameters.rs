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
        let some_has_types = values.iter().any(|v| v.has_type());

        if all_have_types {
            let vals: Vec<TypeAnnotatedValue> = values
                .into_iter()
                .map(|param| param.try_into_type_annotated_value())
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|param| param.unwrap()) // This is expected to always succeed because of the `all_have_types` condition
                .collect();
            Ok(Self::TypedProtoVals(vals))
        } else if !some_has_types {
            let vals: Vec<String> = values
                .into_iter()
                .map(|param| param.into_json_value().to_string())
                .collect();
            Ok(Self::RawJsonStrings(vals))
        } else {
            Err(vec!["Some parameters have types specified, while others don't. Either all parameters must have types or none of them should.".to_string()])
        }
    }
}
