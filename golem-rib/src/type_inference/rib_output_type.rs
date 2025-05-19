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

use crate::{InferredExpr, RibCompilationError};
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RibOutputTypeInfo {
    pub analysed_type: AnalysedType,
}

impl RibOutputTypeInfo {
    pub fn from_expr(
        inferred_expr: &InferredExpr,
    ) -> Result<RibOutputTypeInfo, RibCompilationError> {
        let inferred_type = inferred_expr.get_expr().inferred_type();
        let analysed_type = AnalysedType::try_from(&inferred_type).map_err(|e| {
            RibCompilationError::RibStaticAnalysisError(format!(
                "failed to convert inferred type to analysed type: {}",
                e
            ))
        })?;

        Ok(RibOutputTypeInfo { analysed_type })
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::RibOutputTypeInfo;
    use golem_api_grpc::proto::golem::rib::RibOutputType as ProtoRibOutputType;
    use golem_wasm_ast::analysis::AnalysedType;

    impl From<RibOutputTypeInfo> for ProtoRibOutputType {
        fn from(value: RibOutputTypeInfo) -> Self {
            ProtoRibOutputType {
                r#type: Some(golem_wasm_ast::analysis::protobuf::Type::from(
                    &value.analysed_type,
                )),
            }
        }
    }

    impl TryFrom<ProtoRibOutputType> for RibOutputTypeInfo {
        type Error = String;
        fn try_from(value: ProtoRibOutputType) -> Result<Self, String> {
            let proto_type = value.r#type.ok_or("Missing type")?;
            let analysed_type = AnalysedType::try_from(&proto_type)?;

            Ok(RibOutputTypeInfo { analysed_type })
        }
    }
}
