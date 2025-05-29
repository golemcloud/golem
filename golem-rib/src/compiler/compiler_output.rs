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

use crate::compiler::worker_functions_in_rib::WorkerFunctionsInRib;
use crate::{RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};

#[derive(Debug, Clone)]
pub struct CompilerOutput {
    pub worker_invoke_calls: Option<WorkerFunctionsInRib>,
    pub byte_code: RibByteCode,
    pub rib_input_type_info: RibInputTypeInfo,
    // Optional to keep backward compatible as compiler output information
    // for some existing Rib in persistence store doesn't have this info.
    // This is optional mainly to support the proto conversions.
    // At the API level, if we have access to expr, whenever this field is optional
    // we can compile the expression again and get the output type info
    pub rib_output_type_info: Option<RibOutputTypeInfo>,
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::{
        CompilerOutput, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo, WorkerFunctionsInRib,
    };
    use golem_api_grpc::proto::golem::rib::CompilerOutput as ProtoCompilerOutput;

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

            let rib_output_type_info = value
                .rib_output
                .map(RibOutputTypeInfo::try_from)
                .transpose()?;

            Ok(CompilerOutput {
                worker_invoke_calls,
                byte_code,
                rib_input_type_info: rib_input,
                rib_output_type_info,
            })
        }
    }

    impl TryFrom<CompilerOutput> for ProtoCompilerOutput {
        type Error = String;

        fn try_from(value: CompilerOutput) -> Result<Self, Self::Error> {
            Ok(ProtoCompilerOutput {
                byte_code: Some(golem_api_grpc::proto::golem::rib::RibByteCode::try_from(
                    value.byte_code,
                )?),
                rib_input: Some(golem_api_grpc::proto::golem::rib::RibInputType::from(
                    value.rib_input_type_info,
                )),
                worker_invoke_calls: value
                    .worker_invoke_calls
                    .map(golem_api_grpc::proto::golem::rib::WorkerFunctionsInRib::from),

                rib_output: value
                    .rib_output_type_info
                    .map(golem_api_grpc::proto::golem::rib::RibOutputType::from),
            })
        }
    }
}
