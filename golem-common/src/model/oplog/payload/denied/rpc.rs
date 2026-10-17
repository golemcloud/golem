// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::model::oplog::payload::{
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcInvokeGet,
    HostResponseGolemRpcScheduledInvocationCompat, HostResponseGolemRpcUnitOrFailure,
};
use crate::model::oplog::types::{SerializableInvokeResult, SerializableRpcError};

fn denied(denial: &str) -> SerializableRpcError {
    SerializableRpcError::Denied {
        details: denial.to_string(),
    }
}

pub fn invoke_and_await<R>(_request: &R, denial: &str) -> HostResponseGolemRpcInvokeAndAwait {
    HostResponseGolemRpcInvokeAndAwait {
        result: Err(denied(denial)),
    }
}

pub fn invoke<R>(_request: &R, denial: &str) -> HostResponseGolemRpcUnitOrFailure {
    HostResponseGolemRpcUnitOrFailure {
        result: Err(denied(denial)),
    }
}

pub fn future_invoke_result_get<R>(_request: &R, denial: &str) -> HostResponseGolemRpcInvokeGet {
    HostResponseGolemRpcInvokeGet {
        result: SerializableInvokeResult::Completed(Err(denied(denial))),
    }
}

pub fn schedule_invocation<R>(
    _request: &R,
    denial: &str,
) -> HostResponseGolemRpcScheduledInvocationCompat {
    HostResponseGolemRpcScheduledInvocationCompat {
        result: Err(denied(denial)),
    }
}
