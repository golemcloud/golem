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

#[cfg(test)]
test_r::enable!();

#[allow(clippy::large_enum_variant)]
pub mod proto {
    use crate::proto::golem::worker::UpdateMode;
    use bincode::de::Decoder;
    use bincode::enc::Encoder;
    use bincode::error::{DecodeError, EncodeError};
    use bincode::{Decode, Encode};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance,
    };
    use uuid::Uuid;

    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    impl From<Uuid> for golem::common::Uuid {
        fn from(value: Uuid) -> Self {
            let (high_bits, low_bits) = value.as_u64_pair();
            golem::common::Uuid {
                high_bits,
                low_bits,
            }
        }
    }

    impl From<golem::common::Uuid> for Uuid {
        fn from(value: golem::common::Uuid) -> Self {
            let high_bits = value.high_bits;
            let low_bits = value.low_bits;
            Uuid::from_u64_pair(high_bits, low_bits)
        }
    }

    impl TryFrom<crate::proto::golem::component::FunctionResult> for AnalysedFunctionResult {
        type Error = String;

        fn try_from(
            value: crate::proto::golem::component::FunctionResult,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                name: value.name,
                typ: (&value.typ.ok_or("Missing typ")?).try_into()?,
            })
        }
    }

    impl From<AnalysedFunctionResult> for crate::proto::golem::component::FunctionResult {
        fn from(value: AnalysedFunctionResult) -> Self {
            Self {
                name: value.name,
                typ: Some((&value.typ).into()),
            }
        }
    }

    impl TryFrom<crate::proto::golem::component::ExportInstance> for AnalysedInstance {
        type Error = String;

        fn try_from(
            value: crate::proto::golem::component::ExportInstance,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                name: value.name,
                functions: value
                    .functions
                    .into_iter()
                    .map(|function| function.try_into())
                    .collect::<Result<_, _>>()?,
            })
        }
    }

    impl From<AnalysedInstance> for crate::proto::golem::component::ExportInstance {
        fn from(value: AnalysedInstance) -> Self {
            Self {
                name: value.name,
                functions: value
                    .functions
                    .into_iter()
                    .map(|function| function.into())
                    .collect(),
            }
        }
    }

    impl TryFrom<crate::proto::golem::component::ExportFunction> for AnalysedFunction {
        type Error = String;

        fn try_from(
            value: crate::proto::golem::component::ExportFunction,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                name: value.name,
                parameters: value
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.try_into())
                    .collect::<Result<_, _>>()?,
                results: value
                    .results
                    .into_iter()
                    .map(|result| result.try_into())
                    .collect::<Result<_, _>>()?,
            })
        }
    }

    impl From<AnalysedFunction> for crate::proto::golem::component::ExportFunction {
        fn from(value: AnalysedFunction) -> Self {
            Self {
                name: value.name,
                parameters: value
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                results: value
                    .results
                    .into_iter()
                    .map(|result| result.into())
                    .collect(),
            }
        }
    }

    impl TryFrom<crate::proto::golem::component::Export> for AnalysedExport {
        type Error = String;

        fn try_from(value: crate::proto::golem::component::Export) -> Result<Self, Self::Error> {
            match value.export {
                None => Err("Missing export".to_string()),
                Some(crate::proto::golem::component::export::Export::Instance(instance)) => {
                    Ok(Self::Instance(instance.try_into()?))
                }
                Some(crate::proto::golem::component::export::Export::Function(function)) => {
                    Ok(Self::Function(function.try_into()?))
                }
            }
        }
    }

    impl From<AnalysedExport> for crate::proto::golem::component::Export {
        fn from(value: AnalysedExport) -> Self {
            match value {
                AnalysedExport::Instance(instance) => Self {
                    export: Some(crate::proto::golem::component::export::Export::Instance(
                        instance.into(),
                    )),
                },
                AnalysedExport::Function(function) => Self {
                    export: Some(crate::proto::golem::component::export::Export::Function(
                        function.into(),
                    )),
                },
            }
        }
    }

    impl TryFrom<golem::component::FunctionParameter> for AnalysedFunctionParameter {
        type Error = String;

        fn try_from(
            value: crate::proto::golem::component::FunctionParameter,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                name: value.name,
                typ: (&value.typ.ok_or("Missing typ")?).try_into()?,
            })
        }
    }

    impl From<AnalysedFunctionParameter> for crate::proto::golem::component::FunctionParameter {
        fn from(value: AnalysedFunctionParameter) -> Self {
            Self {
                name: value.name,
                typ: Some((&value.typ).into()),
            }
        }
    }

    impl Encode for UpdateMode {
        fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
            match self {
                UpdateMode::Automatic => 0u8.encode(encoder),
                UpdateMode::Manual => 1u8.encode(encoder),
            }
        }
    }

    impl Decode for UpdateMode {
        fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
            match Decode::decode(decoder)? {
                0u8 => Ok(UpdateMode::Automatic),
                1u8 => Ok(UpdateMode::Manual),
                _ => Err(DecodeError::Other("Invalid UpdateMode")),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::proto::golem;
        use prost::Message;
        use test_r::test;

        #[test]
        fn target_worker_id_and_worker_id_are_bin_compatible() {
            let component_id_uuid = uuid::Uuid::new_v4();
            let component_id_uuid: golem::common::Uuid = component_id_uuid.into();
            let component_id = golem::component::ComponentId {
                value: Some(component_id_uuid),
            };
            let target_worker_id = golem::worker::TargetWorkerId {
                component_id: Some(component_id),
                name: Some("hello".to_string()),
            };
            let worker_id = golem::worker::WorkerId {
                component_id: Some(component_id),
                name: "hello".to_string(),
            };

            let target_worker_id_bytes = target_worker_id.encode_to_vec();
            let worker_id_bytes = worker_id.encode_to_vec();

            assert_eq!(target_worker_id_bytes, worker_id_bytes);
        }
    }
}
