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

pub use crate::base_model::oplog::OplogCursor;
use crate::model::invocation_context::AttributeValue;
use crate::model::oplog::DurableFunctionType;
use crate::model::{Empty, RetryConfig};

pub use crate::base_model::oplog::public_types::*;

impl From<RetryConfig> for PublicRetryConfig {
    fn from(retry_config: RetryConfig) -> Self {
        PublicRetryConfig {
            max_attempts: retry_config.max_attempts,
            min_delay: retry_config.min_delay,
            max_delay: retry_config.max_delay,
            multiplier: retry_config.multiplier,
            max_jitter_factor: retry_config.max_jitter_factor,
        }
    }
}

impl From<DurableFunctionType> for PublicDurableFunctionType {
    fn from(function_type: DurableFunctionType) -> Self {
        match function_type {
            DurableFunctionType::ReadLocal => PublicDurableFunctionType::ReadLocal(Empty {}),
            DurableFunctionType::WriteLocal => PublicDurableFunctionType::WriteLocal(Empty {}),
            DurableFunctionType::ReadRemote => PublicDurableFunctionType::ReadRemote(Empty {}),
            DurableFunctionType::WriteRemote => PublicDurableFunctionType::WriteRemote(Empty {}),
            DurableFunctionType::WriteRemoteBatched(index) => {
                PublicDurableFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
                    index,
                })
            }
            DurableFunctionType::WriteRemoteTransaction(index) => {
                PublicDurableFunctionType::WriteRemoteTransaction(
                    WriteRemoteTransactionParameters { index },
                )
            }
        }
    }
}

impl From<AttributeValue> for PublicAttributeValue {
    fn from(value: AttributeValue) -> Self {
        match value {
            AttributeValue::String(value) => {
                PublicAttributeValue::String(StringAttributeValue { value })
            }
        }
    }
}

impl poem_openapi::types::ParseFromParameter for OplogCursor {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() != 2 {
            return Err("Invalid oplog cursor".into());
        }
        let next_oplog_index = parts[0]
            .parse()
            .map_err(|_| "Invalid index in the oplog cursor")?;
        let current_component_revision = parts[1]
            .parse()
            .map_err(|_| "Invalid component revision in the oplog cursor")?;
        Ok(OplogCursor {
            next_oplog_index,
            current_component_revision,
        })
    }
}
