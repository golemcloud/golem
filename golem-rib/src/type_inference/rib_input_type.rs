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

use crate::{Expr, InferredExpr};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

// RibInputTypeInfo refers to the required global inputs to a RibScript
// with its type information. Example: `request` variable which should be of the type `Record`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct RibInputTypeInfo {
    pub types: HashMap<String, AnalysedType>,
}
impl RibInputTypeInfo {
    pub fn empty() -> Self {
        RibInputTypeInfo {
            types: HashMap::new(),
        }
    }

    pub fn from_expr(inferred_expr: &InferredExpr) -> Result<RibInputTypeInfo, String> {
        let expr: &Expr = inferred_expr.get_expr();
        let mut queue = VecDeque::new();

        let mut global_variables = HashMap::new();

        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    if variable_id.is_global() {
                        let analysed_type = AnalysedType::try_from(inferred_type)?;
                        global_variables.insert(variable_id.name(), analysed_type);
                    }
                }
                _ => expr.visit_children_bottom_up(&mut queue),
            }
        }

        Ok(RibInputTypeInfo {
            types: global_variables,
        })
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::RibInputTypeInfo;
    use golem_api_grpc::proto::golem::rib::RibInputType as ProtoRibInputType;
    use golem_wasm_ast::analysis::AnalysedType;
    use std::collections::HashMap;

    impl TryFrom<ProtoRibInputType> for RibInputTypeInfo {
        type Error = String;
        fn try_from(value: ProtoRibInputType) -> Result<Self, String> {
            let mut types = HashMap::new();
            for (key, value) in value.types {
                types.insert(key, AnalysedType::try_from(&value)?);
            }
            Ok(RibInputTypeInfo { types })
        }
    }

    impl From<RibInputTypeInfo> for ProtoRibInputType {
        fn from(value: RibInputTypeInfo) -> Self {
            let mut types = HashMap::new();
            for (key, value) in value.types {
                types.insert(key, golem_wasm_ast::analysis::protobuf::Type::from(&value));
            }
            ProtoRibInputType { types }
        }
    }
}
