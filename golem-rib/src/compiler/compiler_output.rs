use std::convert::TryFrom;
use crate::compiler::worker_invoke_calls::WorkerFunctionsInRib;
use crate::{RibByteCode, RibInputTypeInfo};
use golem_api_grpc::proto::golem::rib::CompilerOutput as ProtoCompilerOutput;


#[derive(Debug, Clone)]
pub struct CompilerOutput {
    pub worker_invoke_calls: Option<WorkerFunctionsInRib>,
    pub byte_code: RibByteCode,
    pub global_input_type_info: RibInputTypeInfo,
}

impl TryFrom<ProtoCompilerOutput> for CompilerOutput {
    type Error = String;

    fn try_from(value: ProtoCompilerOutput) -> Result<Self, Self::Error> {
        let proto_rib_input = value.rib_input.ok_or("Missing rib_input")?;
        let proto_byte_code = value.byte_code.ok_or("Missing byte_code")?;
        let rib_input = RibInputTypeInfo::try_from(proto_rib_input)?;
        let byte_code = RibByteCode::try_from(proto_byte_code)?;
        let worker_invoke_calls = if let Some(value) = value.worker_invoke_calls {
            Some(WorkerFunctionsInRib::try_from(value)?)
        } else {
            None
        };

        Ok(CompilerOutput {
            worker_invoke_calls,
            byte_code,
            global_input_type_info: rib_input,
        })
    }
}

impl From<CompilerOutput> for ProtoCompilerOutput {
    fn from(value: CompilerOutput) -> Self {
        ProtoCompilerOutput {
            byte_code: Some(golem_api_grpc::proto::golem::rib::RibByteCode::from(
                value.byte_code,
            )),
            rib_input: Some(golem_api_grpc::proto::golem::rib::RibInputType::from(
                value.global_input_type_info,
            )),
            worker_invoke_calls: value
                .worker_invoke_calls
                .map(golem_api_grpc::proto::golem::rib::WorkerFunctionsInRib::from),
        }
    }
}
