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

use crate::{Expr, ExprVisitor, InferredExpr, RibCompilationError};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// RibInputTypeInfo refers to the required global inputs to a RibScript
// with its type information. Example: `request` variable which should be of the type `Record`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RibInputTypeInfo {
    pub types: HashMap<String, AnalysedType>,
}
impl RibInputTypeInfo {
    pub fn get(&self, key: &str) -> Option<&AnalysedType> {
        self.types.get(key)
    }

    pub fn empty() -> Self {
        RibInputTypeInfo {
            types: HashMap::new(),
        }
    }

    pub fn from_expr(
        inferred_expr: &InferredExpr,
    ) -> Result<RibInputTypeInfo, RibCompilationError> {
        let mut expr = inferred_expr.get_expr().clone();
        let mut queue = ExprVisitor::bottom_up(&mut expr);

        let mut global_variables = HashMap::new();

        while let Some(expr) = queue.pop_back() {
            if let Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } = &expr
            {
                if variable_id.is_global() {
                    let analysed_type = AnalysedType::try_from(inferred_type).map_err(|e| {
                        RibCompilationError::RibStaticAnalysisError(format!(
                            "failed to convert inferred type to analysed type: {}",
                            e
                        ))
                    })?;

                    global_variables.insert(variable_id.name(), analysed_type);
                }
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
