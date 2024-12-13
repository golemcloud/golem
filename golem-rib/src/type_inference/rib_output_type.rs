use crate::InferredExpr;
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct RibOutputTypeInfo {
    pub analysed_type: AnalysedType,
}

impl RibOutputTypeInfo {
    pub fn from_expr(inferred_expr: &InferredExpr) -> Result<RibOutputTypeInfo, String> {
        let inferred_type = inferred_expr.get_expr().inferred_type();
        let analysed_type = AnalysedType::try_from(&inferred_type)?;

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
