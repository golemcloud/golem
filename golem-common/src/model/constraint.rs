use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};
use rib::{RegistryKey, WorkerFunctionInRibMetadata};
use golem_api_grpc::proto::golem::component::FunctionUsage as FunctionUsageProto;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct FunctionUsage {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>,
    pub usage_count: u32
}


impl From<FunctionUsage> for WorkerFunctionInRibMetadata {
    fn from(value: FunctionUsage) -> Self {
        WorkerFunctionInRibMetadata {
            function_key: value.function_key.clone(),
            parameter_types: value.parameter_types.clone(),
            return_types: value.return_types.clone(),
        }
    }
}

impl FunctionUsage {
    pub fn from_worker_function_in_rib(worker_function_rib: &WorkerFunctionInRibMetadata) -> FunctionUsage {
        FunctionUsage {
            function_key: worker_function_rib.function_key.clone(),
            parameter_types: worker_function_rib.parameter_types.clone(),
            return_types: worker_function_rib.return_types.clone(),
            usage_count: 1
        }
    }

    pub fn increment_usage(&self) -> FunctionUsage {
        FunctionUsage {
            function_key: self.function_key.clone(),
            parameter_types: self.parameter_types.clone(),
            return_types: self.return_types.clone(),
            usage_count: self.usage_count + 1
        }
    }
}


impl TryFrom<FunctionUsageProto> for FunctionUsage {
    type Error = String;

    fn try_from(value: FunctionUsageProto) -> Result<Self, Self::Error> {
        let return_types = value
            .return_types
            .iter()
            .map(AnalysedType::try_from)
            .collect::<Result<_, _>>()?;

        let parameter_types = value
            .parameter_types
            .iter()
            .map(AnalysedType::try_from)
            .collect::<Result<_, _>>()?;

        let registry_key_proto = value.function_key.ok_or("Function key missing")?;
        let function_key = RegistryKey::try_from(registry_key_proto)?;
        let usage_count = value.usage_count;

        Ok(Self {
            function_key,
            return_types,
            parameter_types,
            usage_count
        })
    }
}

impl From<FunctionUsage> for FunctionUsageProto {
    fn from(value: FunctionUsage) -> Self {
        let registry_key = value.function_key.into();

        FunctionUsageProto {
            function_key: Some(registry_key),
            parameter_types: value.parameter_types.iter().map(|x| x.into()).collect(),
            return_types: value.return_types.iter().map(|x| x.into()).collect(),
            usage_count: value.usage_count
        }
    }
}


