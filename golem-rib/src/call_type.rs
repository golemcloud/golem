// Copyright 2024 Golem Cloud
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

use crate::ParsedFunctionName;
use bincode::{Decode, Encode};
use std::convert::TryFrom;
use std::fmt::Display;

#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub enum CallType {
    Function(ParsedFunctionName),
    VariantConstructor(String),
    EnumConstructor(String),
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

impl TryFrom<golem_api_grpc::proto::golem::rib::InvocationName> for CallType {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::rib::InvocationName,
    ) -> Result<Self, Self::Error> {
        let invocation = value.name.ok_or("Missing name of invocation")?;
        match invocation {
            golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(name) => {
                Ok(CallType::Function(ParsedFunctionName::try_from(name)?))
            }
            golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(name) => {
                Ok(CallType::VariantConstructor(name))
            }
            golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(name) => {
                Ok(CallType::EnumConstructor(name))
            }
        }
    }
}

impl From<CallType> for golem_api_grpc::proto::golem::rib::InvocationName {
    fn from(value: CallType) -> Self {
        match value {
            CallType::Function(parsed_name) => {
                golem_api_grpc::proto::golem::rib::InvocationName {
                    name: Some(golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(
                        parsed_name.into(),
                    )),
                }
            }
            CallType::VariantConstructor(name) => {
                golem_api_grpc::proto::golem::rib::InvocationName {
                    name: Some(golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(
                        name,
                    )),
                }
            }
            CallType::EnumConstructor(name) => {
                golem_api_grpc::proto::golem::rib::InvocationName {
                    name: Some(golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(
                        name,
                    )),
                }
            }
        }
    }
}
