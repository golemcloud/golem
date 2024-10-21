use std::convert::TryFrom;
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};
use rib::ParsedFunctionName;

// A trimmed down version of component metadata that just includes enough details
// that act as constraints that services should adhere to for compatibility purposes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionConstraint {
    pub function_name: ParsedFunctionName,
    pub argument_types: Vec<AnalysedType>,
    pub result_types: Vec<AnalysedType>,
}

impl From<FunctionConstraint> for golem_api_grpc::proto::golem::component::FunctionConstraint {
    fn from(value: FunctionConstraint) -> Self {
        Self {
            function_name: value.function_name.to_string(),
            argument_types: value
                .argument_types
                .iter()
                .map(|analysed_type| analysed_type.into())
                .collect(),
            result_types: value
                .result_types
                .iter()
                .map(|analysed_type| analysed_type.into())
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraint> for FunctionConstraint {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionConstraint,
    ) -> Result<Self, Self::Error> {
        let result = FunctionConstraint {
            function_name: ParsedFunctionName::parse(value.function_name)?,
            argument_types: value
                .argument_types
                .into_iter()
                .map(|typ| AnalysedType::try_from(&typ))
                .collect::<Result<_, _>>()?,
            result_types: value
                .result_types
                .into_iter()
                .map(|typ| AnalysedType::try_from(&typ))
                .collect::<Result<_, _>>()?,
        };

        Ok(result)
    }
}