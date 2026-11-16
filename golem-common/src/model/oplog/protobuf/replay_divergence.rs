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

use crate::base_model::OplogIndex;
use crate::base_model::oplog::{
    ReplayDivergence, ReplayHostCall, ReplayStep, ReplayStepDescription, StartIdentityField,
    UpdateFailureClass,
};
use golem_api_grpc::proto::golem::worker as proto;
use golem_schema::Empty;

impl TryFrom<ReplayDivergence> for proto::ReplayDivergence {
    type Error = String;

    fn try_from(value: ReplayDivergence) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: value.oplog_index.into(),
            expected: Some(Box::new(value.expected.try_into()?)),
            actual: Some(Box::new(value.actual.try_into()?)),
            mismatched: value
                .mismatched
                .into_iter()
                .map(|field| proto::StartIdentityField::from(field) as i32)
                .collect(),
        })
    }
}

impl TryFrom<proto::ReplayDivergence> for ReplayDivergence {
    type Error = String;

    fn try_from(value: proto::ReplayDivergence) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: OplogIndex::from_u64(value.oplog_index),
            expected: (*value.expected.ok_or("Missing field: expected")?).try_into()?,
            actual: (*value.actual.ok_or("Missing field: actual")?).try_into()?,
            mismatched: value
                .mismatched
                .into_iter()
                .map(|field| {
                    proto::StartIdentityField::try_from(field)
                        .map(StartIdentityField::from)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl TryFrom<ReplayStep> for proto::ReplayStep {
    type Error = String;

    fn try_from(value: ReplayStep) -> Result<Self, Self::Error> {
        use proto::replay_step::Step;
        let step = match value {
            ReplayStep::HostCall(call) => Step::HostCall(call.try_into()?),
            ReplayStep::InvocationEnd(_) => {
                Step::InvocationEnd(golem_schema::proto::golem::common::Empty {})
            }
            ReplayStep::EndOfHistory(_) => {
                Step::EndOfHistory(golem_schema::proto::golem::common::Empty {})
            }
            ReplayStep::Other(description) => Step::Other(description.description),
        };
        Ok(Self { step: Some(step) })
    }
}

impl TryFrom<proto::ReplayStep> for ReplayStep {
    type Error = String;

    fn try_from(value: proto::ReplayStep) -> Result<Self, Self::Error> {
        use proto::replay_step::Step;
        Ok(match value.step.ok_or("Missing field: step")? {
            Step::HostCall(call) => ReplayStep::HostCall(call.try_into()?),
            Step::InvocationEnd(_) => ReplayStep::InvocationEnd(Empty {}),
            Step::EndOfHistory(_) => ReplayStep::EndOfHistory(Empty {}),
            Step::Other(description) => ReplayStep::Other(ReplayStepDescription { description }),
        })
    }
}

impl TryFrom<ReplayHostCall> for proto::ReplayHostCall {
    type Error = String;

    fn try_from(value: ReplayHostCall) -> Result<Self, Self::Error> {
        Ok(Self {
            function_name: value.function_name,
            durable_function_type: Some(value.durable_function_type.into()),
            parent_start_index: value.parent_start_index.map(Into::into),
            invocation_id: value.invocation_id.map(Into::into),
            observational_owner: value.observational_owner.map(Into::into),
            carries_request: value.carries_request,
            request: value
                .request
                .map(|request| (*request).try_into())
                .transpose()?,
        })
    }
}

impl TryFrom<proto::ReplayHostCall> for ReplayHostCall {
    type Error = String;

    fn try_from(value: proto::ReplayHostCall) -> Result<Self, Self::Error> {
        Ok(Self {
            function_name: value.function_name,
            durable_function_type: value
                .durable_function_type
                .ok_or("Missing field: durable_function_type")?
                .try_into()?,
            parent_start_index: value.parent_start_index.map(OplogIndex::from_u64),
            invocation_id: value.invocation_id.map(Into::into),
            observational_owner: value.observational_owner.map(OplogIndex::from_u64),
            carries_request: value.carries_request,
            request: value
                .request
                .map(|request| crate::schema::TypedSchemaValue::try_from(request).map(Box::new))
                .transpose()
                .map_err(|error| format!("Invalid request: {error}"))?,
        })
    }
}

impl From<StartIdentityField> for proto::StartIdentityField {
    fn from(value: StartIdentityField) -> Self {
        match value {
            StartIdentityField::FunctionName => Self::FunctionName,
            StartIdentityField::FunctionType => Self::FunctionType,
            StartIdentityField::InvocationId => Self::InvocationId,
            StartIdentityField::ObservationalOwner => Self::ObservationalOwner,
            StartIdentityField::RequestPresence => Self::RequestPresence,
            StartIdentityField::Parent => Self::Parent,
        }
    }
}

impl From<proto::StartIdentityField> for StartIdentityField {
    fn from(value: proto::StartIdentityField) -> Self {
        match value {
            proto::StartIdentityField::FunctionName => Self::FunctionName,
            proto::StartIdentityField::FunctionType => Self::FunctionType,
            proto::StartIdentityField::InvocationId => Self::InvocationId,
            proto::StartIdentityField::ObservationalOwner => Self::ObservationalOwner,
            proto::StartIdentityField::RequestPresence => Self::RequestPresence,
            proto::StartIdentityField::Parent => Self::Parent,
        }
    }
}

impl From<UpdateFailureClass> for proto::UpdateFailureClass {
    fn from(value: UpdateFailureClass) -> Self {
        match value {
            UpdateFailureClass::Divergence => Self::Divergence,
            UpdateFailureClass::TargetTrap => Self::TargetTrap,
            UpdateFailureClass::NotReplayable => Self::NotReplayable,
            UpdateFailureClass::TargetRejected => Self::TargetRejected,
            UpdateFailureClass::Interrupted => Self::Interrupted,
            UpdateFailureClass::Operational => Self::Operational,
            UpdateFailureClass::Other => Self::Other,
        }
    }
}

impl From<proto::UpdateFailureClass> for UpdateFailureClass {
    fn from(value: proto::UpdateFailureClass) -> Self {
        match value {
            proto::UpdateFailureClass::Divergence => Self::Divergence,
            proto::UpdateFailureClass::TargetTrap => Self::TargetTrap,
            proto::UpdateFailureClass::NotReplayable => Self::NotReplayable,
            proto::UpdateFailureClass::TargetRejected => Self::TargetRejected,
            proto::UpdateFailureClass::Interrupted => Self::Interrupted,
            proto::UpdateFailureClass::Operational => Self::Operational,
            proto::UpdateFailureClass::Other => Self::Other,
        }
    }
}

impl UpdateFailureClass {
    /// The class that a protobuf enum number names.
    pub fn from_proto_number(value: i32) -> Result<Self, String> {
        proto::UpdateFailureClass::try_from(value)
            .map(Self::from)
            .map_err(|error| format!("Invalid update failure class {value}: {error}"))
    }

    /// The protobuf enum number of the class.
    pub fn proto_number(self) -> i32 {
        proto::UpdateFailureClass::from(self) as i32
    }
}
