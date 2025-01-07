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

use crate::DynamicParsedFunctionName;
use std::fmt::Display;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CallType {
    Function(DynamicParsedFunctionName),
    VariantConstructor(String),
    EnumConstructor(String),
}

impl CallType {
    pub fn is_resource_method(&self) -> bool {
        match self {
            CallType::Function(parsed_fn_name) => parsed_fn_name
                .to_parsed_function_name()
                .function
                .resource_method_name()
                .is_some(),
            _ => false,
        }
    }
}

impl Display for CallType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallType::Function(parsed_fn_name) => write!(f, "{}", parsed_fn_name),
            CallType::VariantConstructor(name) => write!(f, "{}", name),
            CallType::EnumConstructor(name) => write!(f, "{}", name),
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::call_type::CallType;
    use crate::{DynamicParsedFunctionName, ParsedFunctionName};

    impl TryFrom<golem_api_grpc::proto::golem::rib::CallType> for CallType {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::rib::CallType,
        ) -> Result<Self, Self::Error> {
            let invocation = value.name.ok_or("Missing name of invocation")?;
            match invocation {
                golem_api_grpc::proto::golem::rib::call_type::Name::Parsed(name) => Ok(
                    CallType::Function(DynamicParsedFunctionName::try_from(name)?),
                ),
                golem_api_grpc::proto::golem::rib::call_type::Name::VariantConstructor(name) => {
                    Ok(CallType::VariantConstructor(name))
                }
                golem_api_grpc::proto::golem::rib::call_type::Name::EnumConstructor(name) => {
                    Ok(CallType::EnumConstructor(name))
                }
            }
        }
    }

    impl From<CallType> for golem_api_grpc::proto::golem::rib::CallType {
        fn from(value: CallType) -> Self {
            match value {
                CallType::Function(parsed_name) => golem_api_grpc::proto::golem::rib::CallType {
                    name: Some(golem_api_grpc::proto::golem::rib::call_type::Name::Parsed(
                        parsed_name.into(),
                    )),
                },
                CallType::VariantConstructor(name) => golem_api_grpc::proto::golem::rib::CallType {
                    name: Some(
                        golem_api_grpc::proto::golem::rib::call_type::Name::VariantConstructor(
                            name,
                        ),
                    ),
                },
                CallType::EnumConstructor(name) => golem_api_grpc::proto::golem::rib::CallType {
                    name: Some(
                        golem_api_grpc::proto::golem::rib::call_type::Name::EnumConstructor(name),
                    ),
                },
            }
        }
    }

    // InvocationName is a legacy structure to keep the backward compatibility.
    // InvocationName is corresponding to the new CallType and the difference here is,
    // InvocationName::Function will always hold a static function name and not a dynamic one
    // with Expr representing resource construction parameters
    impl TryFrom<golem_api_grpc::proto::golem::rib::InvocationName> for CallType {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::rib::InvocationName,
        ) -> Result<Self, Self::Error> {
            let invocation = value.name.ok_or("Missing name of invocation")?;
            match invocation {
                golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(name) => {
                    Ok(CallType::Function(DynamicParsedFunctionName::parse(
                        ParsedFunctionName::try_from(name)?.to_string(),
                    )?))
                }
                golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(
                    name,
                ) => Ok(CallType::VariantConstructor(name)),
                golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(name) => {
                    Ok(CallType::EnumConstructor(name))
                }
            }
        }
    }
}
